//! 音频引擎控制：解码线程生命周期与跨线程播放调度。

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use ringbuf::traits::{Observer, Split};
use ringbuf::{HeapProd, HeapRb};

use crate::decoder::Decoder;
use crate::output;
use crate::resample::Resampler;
use crate::ring::{map_channels, push_slice, ring_capacity};
use crate::state::Shared;
use crate::{Error, Event, Fetcher, Snapshot, State, TrackRef};

/// 音频引擎配置参数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    /// 环形缓冲区目标时长（毫秒，默认 200ms）
    pub ring_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { ring_ms: 200 }
    }
}

/// 解码线程控制命令
enum Command {
    Play(TrackRef),
    Toggle,
    Seek(u64),
    Shutdown,
}

/// 音频引擎控制句柄（非阻塞、线程安全）
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    shared: Arc<Shared>,
    cmd_tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    /// 输出流句柄（随引擎销毁而停止）
    _stream: cpal::Stream,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        let Some(handle) = self.thread.take() else {
            return;
        };
        // 最多等待解码线程 2 秒，防退出挂起
        let deadline = Instant::now() + Duration::from_secs(2);
        while !handle.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if handle.is_finished() {
            let _ = handle.join();
        } else {
            tracing::warn!("解码线程退出超时，已放弃等待");
        }
    }
}

impl Engine {
    /// 播放指定曲目
    pub fn play(&self, track: TrackRef) {
        self.send(Command::Play(track));
    }

    /// 切换播放与暂停状态
    pub fn toggle(&self) {
        self.send(Command::Toggle);
    }

    /// 定位到指定毫秒位置
    pub fn seek(&self, ms: u64) {
        self.send(Command::Seek(ms));
    }

    /// 设置音量（0.0..=1.0）
    pub fn set_volume(&self, v: f32) {
        self.inner.shared.set_volume(v.clamp(0.0, 1.0));
    }

    /// 获取当前状态快照（非阻塞）
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let shared = &self.inner.shared;
        Snapshot {
            state: shared.state(),
            track: shared.track(),
            position_ms: shared.position_ms(),
            duration_ms: shared.duration_ms(),
            volume: shared.volume(),
        }
    }

    fn send(&self, cmd: Command) {
        let _ = self.inner.cmd_tx.send(cmd);
    }
}

/// 使用默认配置启动音频引擎
pub fn spawn(fetcher: Arc<dyn Fetcher>, events: Box<dyn Fn(Event) + Send + Sync>) -> Result<Engine, Error> {
    spawn_with(fetcher, events, AudioConfig::default())
}

/// 启动音频引擎（配置输出设备、缓冲区与解码线程）
pub fn spawn_with(
    fetcher: Arc<dyn Fetcher>,
    events: Box<dyn Fn(Event) + Send + Sync>,
    config: AudioConfig,
) -> Result<Engine, Error> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(Error::NoOutputDevice)?;
    let supported = device
        .default_output_config()
        .map_err(|e| Error::OutputConfig(e.to_string()))?;

    let sample_rate = supported.sample_rate().0;
    let channels = usize::from(supported.channels());
    let sample_format = supported.sample_format();
    let stream_config = supported.config();

    let capacity = ring_capacity(sample_rate, channels, config.ring_ms);
    let ring = HeapRb::<f32>::new(capacity);
    let (prod, cons) = ring.split();

    let shared = Shared::new();
    let stream = output::build_stream(&device, &stream_config, sample_format, &shared, cons)?;
    output::start(&stream)?;

    let (cmd_tx, cmd_rx) = unbounded();
    let ctx = RunCtx {
        fetcher,
        events,
        cmd_rx,
        prod,
        shared: shared.clone(),
        sample_rate,
        channels,
    };
    let thread = thread::Builder::new()
        .name("lyra-audio".into())
        .spawn(move || Player::new(ctx).run())
        .map_err(|e| Error::SpawnThread(e.to_string()))?;

    Ok(Engine {
        inner: Arc::new(EngineInner {
            shared,
            cmd_tx,
            thread: Some(thread),
            _stream: stream,
        }),
    })
}

/// 解码线程运行上下文
struct RunCtx {
    fetcher: Arc<dyn Fetcher>,
    events: Box<dyn Fn(Event) + Send + Sync>,
    cmd_rx: Receiver<Command>,
    prod: HeapProd<f32>,
    shared: Arc<Shared>,
    sample_rate: u32,
    channels: usize,
}

/// 单曲播放会话
struct Session {
    decoder: Decoder,
    /// 当前曲目信息
    track: TrackRef,
    resampler: Option<Resampler>,
    src_channels: usize,
    dst_channels: usize,
    dst_rate: u32,
    duration_ms: u64,
    /// 待推入缓冲区的转换后样本
    pending: Vec<f32>,
    pending_off: usize,
    /// 已推入缓冲区的总帧数
    pushed_frames: u64,
}

impl Session {
    fn open(ctx: &RunCtx, track: &TrackRef) -> Result<Self, Error> {
        let reader = ctx.fetcher.open(track).map_err(|e| Error::Decode(e.to_string()))?;
        let decoder = Decoder::open_with_hint(reader, Some(&track.ext))?;
        let src_channels = decoder.channels();
        let src_rate = decoder.sample_rate();
        // 采样率不一致时启用重采样
        let resampler = if src_rate == ctx.sample_rate {
            None
        } else {
            Some(Resampler::new(src_rate, ctx.sample_rate, src_channels)?)
        };
        Ok(Self {
            decoder,
            track: track.clone(),
            resampler,
            src_channels,
            dst_channels: ctx.channels,
            dst_rate: ctx.sample_rate,
            duration_ms: track.duration_ms,
            pending: Vec::new(),
            pending_off: 0,
            pushed_frames: 0,
        })
    }

    /// 转换源 PCM 样本并加入待推送队列
    fn stage(&mut self, pcm: &[f32]) -> Result<(), Error> {
        let resampled = match &mut self.resampler {
            Some(r) => r.process(pcm)?,
            None => pcm.to_vec(),
        };
        let mapped = map_channels(&resampled, self.src_channels, self.dst_channels);
        self.pending.extend_from_slice(&mapped);
        Ok(())
    }

    /// 推送样本到环形缓冲区（返回是否已全部写入）
    fn flush(&mut self, prod: &mut HeapProd<f32>) -> bool {
        let pushed = push_slice(prod, &self.pending[self.pending_off..]);
        self.pending_off += pushed;
        self.pushed_frames += (pushed / self.dst_channels) as u64;
        if self.pending_off >= self.pending.len() {
            self.pending.clear();
            self.pending_off = 0;
            true
        } else {
            false
        }
    }

    /// 顺序解码并丢弃样本直到指定位置（避免未定长流 seek 导致 panic）
    fn advance_to(&mut self, ms: u64) -> Result<(), Error> {
        let target = ms.saturating_mul(u64::from(self.decoder.sample_rate())) / 1000;
        let mut skipped = 0_u64;
        while skipped < target {
            let Some(pcm) = self.decoder.next_chunk()? else {
                break;
            };
            skipped += (pcm.len() / self.src_channels) as u64;
        }
        Ok(())
    }

    /// 计算当前播放进度（毫秒）
    fn position_ms(&self, prod: &HeapProd<f32>) -> u64 {
        if self.dst_rate == 0 {
            return 0;
        }
        let in_ring = (prod.occupied_len() / self.dst_channels) as u64;
        let played = self.pushed_frames.saturating_sub(in_ring);
        played * 1000 / u64::from(self.dst_rate)
    }
}

/// 单次数据泵状态
enum Pump {
    Progress,
    Full,
    Ended,
    Failed(String),
}

/// 播放器主状态机
struct Player {
    ctx: RunCtx,
    state: State,
    session: Option<Session>,
    last_report: Instant,
}

impl Player {
    fn new(ctx: RunCtx) -> Self {
        Self {
            ctx,
            state: State::Idle,
            session: None,
            last_report: Instant::now(),
        }
    }

    fn run(mut self) {
        while self.tick() {}
    }

    /// 执行单轮事件处理与解码调度（返回 false 表示退出线程）
    fn tick(&mut self) -> bool {
        let busy = self.state == State::Playing && self.session.is_some();
        if busy {
            while let Ok(cmd) = self.ctx.cmd_rx.try_recv() {
                if !self.handle(cmd) {
                    return false;
                }
            }
        } else {
            match self.ctx.cmd_rx.recv_timeout(Duration::from_millis(20)) {
                Ok(cmd) => {
                    if !self.handle(cmd) {
                        return false;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }

        if self.state == State::Playing {
            match self.pump() {
                Pump::Progress => {}
                Pump::Full => thread::sleep(Duration::from_millis(2)),
                Pump::Ended => {
                    // 等待缓冲区尾音播放完毕（超时 2 秒防挂死）
                    let mut waited = 0_u32;
                    while self.ctx.prod.occupied_len() > 0 && waited < 400 {
                        thread::sleep(Duration::from_millis(5));
                        waited += 1;
                    }
                    let dur = self.session.as_ref().map_or(0, |s| s.duration_ms);
                    self.session = None;
                    self.ctx.shared.set_position(dur, dur);
                    self.emit(Event::Ended);
                    self.set_state(State::Idle);
                }
                Pump::Failed(message) => {
                    self.session = None;
                    self.emit(Event::Failed { message });
                    self.set_state(State::Idle);
                }
            }
        }

        self.report_position();
        true
    }

    fn handle(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Play(track) => self.start(&track),
            Command::Toggle => match self.state {
                State::Playing => self.set_state(State::Paused),
                State::Paused => self.set_state(State::Playing),
                _ => {}
            },
            Command::Seek(ms) => self.seek(ms),
            Command::Shutdown => return false,
        }
        true
    }

    fn start(&mut self, track: &TrackRef) {
        // 更新加载态与当前曲目
        self.set_state(State::Loading);
        self.ctx.shared.set_track(track.clone());

        match Session::open(&self.ctx, track) {
            Ok(session) => {
                self.session = Some(session);
                self.ctx.shared.set_position(0, track.duration_ms);
                // 切歌请求刷新缓冲区，避免混入前一首尾音
                self.ctx.shared.request_flush();
                self.set_state(State::Playing);
            }
            Err(e) => {
                self.session = None;
                self.emit(Event::Failed { message: e.to_string() });
                self.set_state(State::Idle);
            }
        }
    }

    fn seek(&mut self, ms: u64) {
        let dst_rate = self.ctx.sample_rate;
        let Some(track) = self.session.as_ref().map(|session| session.track.clone()) else {
            return;
        };
        // 请求清空缓冲区，停止输出旧位置样本
        self.ctx.shared.request_flush();
        // 重新打开解码器并快进至目标位置
        let mut session = match Session::open(&self.ctx, &track) {
            Ok(session) => session,
            Err(error) => {
                self.emit(Event::Failed {
                    message: format!("定位失败: {error}"),
                });
                return;
            }
        };
        if let Err(error) = session.advance_to(ms) {
            self.emit(Event::Failed {
                message: format!("定位失败: {error}"),
            });
            return;
        }
        session.pushed_frames = ms.saturating_mul(u64::from(dst_rate)) / 1000;
        self.session = Some(session);
        // 立即更新进度位置
        self.ctx.shared.set_position(ms, track.duration_ms);
    }

    /// 解码并推送样本（缓冲区满时返回 Full）
    fn pump(&mut self) -> Pump {
        let Self { ctx, session, .. } = self;
        let Some(session) = session.as_mut() else {
            return Pump::Progress;
        };
        if !session.flush(&mut ctx.prod) {
            return Pump::Full;
        }
        match session.decoder.next_chunk() {
            Ok(Some(pcm)) => {
                if let Err(e) = session.stage(&pcm) {
                    return Pump::Failed(e.to_string());
                }
                session.flush(&mut ctx.prod);
                Pump::Progress
            }
            Ok(None) => Pump::Ended,
            Err(e) => Pump::Failed(e.to_string()),
        }
    }

    fn emit(&self, event: Event) {
        (self.ctx.events)(event);
    }

    fn set_state(&mut self, state: State) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.ctx.shared.set_state(state);
    }

    /// 定期上报播放进度（约 150ms 间隔）
    fn report_position(&mut self) {
        if self.last_report.elapsed() < Duration::from_millis(150) {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let position = session.position_ms(&self.ctx.prod);
        let duration = session.duration_ms;
        self.last_report = Instant::now();
        self.ctx.shared.set_position(position, duration);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]
    use super::*;

    /// 无声卡环境直接跳过；有设备时验证引擎可启动并返回快照。
    #[test]
    fn spawn_smoke_skips_without_device() {
        if cpal::default_host().default_output_device().is_none() {
            return;
        }
        struct NoFetcher;
        impl Fetcher for NoFetcher {
            fn open(&self, _track: &TrackRef) -> crate::FetchResult<Box<dyn crate::ReadSeek + Send>> {
                Err(crate::FetchError::Message("测试不抓取".into()))
            }
        }

        let engine = spawn(Arc::new(NoFetcher), Box::new(|_event| {})).unwrap();
        let snap = engine.snapshot();
        assert_eq!(snap.state, State::Idle);
        assert!(snap.track.is_none());
    }
}
