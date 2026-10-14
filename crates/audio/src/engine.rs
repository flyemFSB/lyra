//! 音频引擎装配与控制：管理解码线程、输出流生命周期及跨线程命令调度。

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use parking_lot::Mutex;
use ringbuf::traits::{Observer, Split};
use ringbuf::{HeapProd, HeapRb};

use crate::decoder::Decoder;
use crate::output;
use crate::resample::Resampler;
use crate::ring::{map_channels, push_slice, ring_capacity};
use crate::state::Shared;
use crate::{Error, Event, Fetcher, Snapshot, State, TrackRef};

/// 引擎可调参数，[`Default`] 给出推荐值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    /// 环形缓冲目标时长（毫秒）。越大越抗抖动，越大切歌 / 暂停延迟越明显。
    pub ring_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { ring_ms: 200 }
    }
}

/// UI → 解码线程的控制命令。
enum Command {
    Play(TrackRef),
    Pause,
    Resume,
    Toggle,
    Stop,
    Seek(u64),
    Shutdown,
}

/// UI 侧句柄。所有方法非阻塞、幂等；`Clone` 共享同一底层引擎。
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    shared: Arc<Shared>,
    cmd_tx: Sender<Command>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// 输出流必须存活；`Stream` 在 drop 时停止播放。
    _stream: cpal::Stream,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        if let Some(handle) = self.thread.lock().take() {
            let _ = handle.join();
        }
    }
}

impl Engine {
    /// 播放一首曲目（会替换当前曲目）。
    pub fn play(&self, track: TrackRef) {
        self.send(Command::Play(track));
    }

    /// 暂停当前播放。空闲时无效果。
    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    /// 从暂停恢复播放。
    pub fn resume(&self) {
        self.send(Command::Resume);
    }

    /// 播放 / 暂停切换。
    pub fn toggle(&self) {
        self.send(Command::Toggle);
    }

    /// 停止并清空当前曲目。
    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    /// 定位到指定毫秒。无当前曲目时无效果。
    pub fn seek(&self, ms: u64) {
        self.send(Command::Seek(ms));
    }

    /// 设置音量，范围外的值会被夹紧到 `0.0..=1.0`。
    pub fn set_volume(&self, v: f32) {
        self.inner.shared.set_volume(v.clamp(0.0, 1.0));
    }

    /// 读取当前状态快照。只读原子字段；曲目字段仅短暂加锁，不做任何 IO 或等待。
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

/// 用默认 [`AudioConfig`] 启动引擎。
///
/// # Errors
/// 见 [`spawn_with`]。
pub fn spawn(fetcher: Arc<dyn Fetcher>, events: Box<dyn Fn(Event) + Send + Sync>) -> Result<Engine, Error> {
    spawn_with(fetcher, events, AudioConfig::default())
}

/// 打开默认输出设备、构建环形缓冲与 cpal 输出流，并启动解码线程。
///
/// # Errors
/// - 无默认输出设备 → [`Error::NoOutputDevice`]
/// - 设备默认配置读取失败 → [`Error::OutputConfig`]
/// - 采样格式不支持 / 输出流构建失败 → [`Error::UnsupportedSampleFormat`] / [`Error::BuildStream`]
/// - 输出流无法播放 → [`Error::PlayStream`]
/// - 解码线程启动失败 → [`Error::SpawnThread`]
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
            thread: Mutex::new(Some(thread)),
            _stream: stream,
        }),
    })
}

/// 解码线程持有的上下文。
struct RunCtx {
    fetcher: Arc<dyn Fetcher>,
    events: Box<dyn Fn(Event) + Send + Sync>,
    cmd_rx: Receiver<Command>,
    prod: HeapProd<f32>,
    shared: Arc<Shared>,
    sample_rate: u32,
    channels: usize,
}

/// 一首正在播放的曲目：解码器 + 可选重采样器 + 待推送样本。
struct Session {
    decoder: Decoder,
    resampler: Option<Resampler>,
    src_channels: usize,
    dst_channels: usize,
    dst_rate: u32,
    duration_ms: u64,
    /// 已转换、尚未推入 ring 的交织样本（目标采样率 / 声道）。
    pending: Vec<f32>,
    pending_off: usize,
    /// 已推入 ring 的帧数（目标采样率），用于推算播放进度。
    pushed_frames: u64,
}

impl Session {
    fn open(ctx: &RunCtx, track: &TrackRef) -> Result<Self, Error> {
        let reader = ctx.fetcher.open(track).map_err(|e| Error::Decode(e.to_string()))?;
        let decoder = Decoder::open_with_hint(reader, Some(&track.ext))?;
        let src_channels = decoder.channels();
        let src_rate = decoder.sample_rate();
        // 仅当设备采样率与音频源采样率不一致时启用重采样
        let resampler = if src_rate == ctx.sample_rate {
            None
        } else {
            Some(Resampler::new(src_rate, ctx.sample_rate, src_channels)?)
        };
        Ok(Self {
            decoder,
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

    /// 把一段源 PCM 转换（重采样 + 声道映射）进入 pending 队列。
    fn stage(&mut self, pcm: &[f32]) -> Result<(), Error> {
        let resampled = match &mut self.resampler {
            Some(r) => r.process(pcm)?,
            None => pcm.to_vec(),
        };
        let mapped = map_channels(&resampled, self.src_channels, self.dst_channels);
        self.pending.extend_from_slice(&mapped);
        Ok(())
    }

    /// 尽量把 pending 推入 ring；返回是否已全部推入（false 表示 ring 已满）。
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

    /// 当前播放进度（毫秒）：已推送帧数减去仍在 ring 中未播的帧数。
    fn position_ms(&self, prod: &HeapProd<f32>) -> u64 {
        if self.dst_rate == 0 {
            return 0;
        }
        let in_ring = (prod.occupied_len() / self.dst_channels) as u64;
        let played = self.pushed_frames.saturating_sub(in_ring);
        played * 1000 / u64::from(self.dst_rate)
    }
}

/// 一次 `pump` 的结果。
enum Pump {
    Progress,
    Full,
    Ended,
    Failed(String),
}

/// 解码线程主循环的状态机。
struct Player {
    ctx: RunCtx,
    state: State,
    session: Option<Session>,
    last_report: Instant,
    last_position_ms: u64,
}

impl Player {
    fn new(ctx: RunCtx) -> Self {
        Self {
            ctx,
            state: State::Idle,
            session: None,
            last_report: Instant::now(),
            last_position_ms: 0,
        }
    }

    fn run(mut self) {
        while self.tick() {}
    }

    /// 处理一轮命令 + 解码 + 进度上报；返回 false 表示退出线程。
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
                    // 等待环形缓冲区尾音样本播放完毕（最长等待 2 秒以防死锁）
                    let mut waited = 0_u32;
                    while self.ctx.prod.occupied_len() > 0 && waited < 400 {
                        thread::sleep(Duration::from_millis(5));
                        waited += 1;
                    }
                    let dur = self.session.as_ref().map_or(0, |s| s.duration_ms);
                    self.session = None;
                    self.push_position(dur, dur);
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
            Command::Pause => {
                if self.state == State::Playing {
                    self.set_state(State::Paused);
                }
            }
            Command::Resume => {
                if self.state == State::Paused {
                    self.set_state(State::Playing);
                }
            }
            Command::Toggle => match self.state {
                State::Playing => self.set_state(State::Paused),
                State::Paused => self.set_state(State::Playing),
                _ => {}
            },
            Command::Stop => self.stop(),
            Command::Seek(ms) => self.seek(ms),
            Command::Shutdown => return false,
        }
        true
    }

    fn start(&mut self, track: &TrackRef) {
        // 更新加载态与当前曲目信息
        self.set_state(State::Loading);
        self.ctx.shared.set_track(Some(track.clone()));
        self.emit(Event::TrackChanged(Some(track.clone())));

        match Session::open(&self.ctx, track) {
            Ok(session) => {
                self.session = Some(session);
                self.push_position(0, track.duration_ms);
                self.set_state(State::Playing);
            }
            Err(e) => {
                self.session = None;
                self.emit(Event::Failed { message: e.to_string() });
                self.set_state(State::Idle);
            }
        }
    }

    fn stop(&mut self) {
        self.session = None;
        self.ctx.shared.set_track(None);
        self.push_position(0, 0);
        self.emit(Event::TrackChanged(None));
        self.set_state(State::Idle);
    }

    fn seek(&mut self, ms: u64) {
        let dst_rate = self.ctx.sample_rate;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Err(error) = session.decoder.seek(ms) {
            // 定位失败时上报失败事件通知
            let message = format!("seek 失败: {error}");
            self.emit(Event::Failed { message });
            return;
        }
        if let Some(r) = &mut session.resampler {
            r.reset();
        }
        session.pending.clear();
        session.pending_off = 0;
        session.pushed_frames = ms * u64::from(dst_rate) / 1000;
        // 发送刷新信号清空环形缓冲中旧位置残留样本
        self.ctx.shared.request_flush();
        // 目标定位位置直接更新，避免受异步清空延迟影响
        let duration = session.duration_ms;
        self.push_position(ms, duration);
    }

    /// 解码一段并推入 ring。ring 满时返回 [`Pump::Full`]，由外层短暂休眠后重试。
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
        self.emit(Event::StateChanged(state));
    }

    /// 每约 150ms 上报一次进度；仅在存在当前曲目时进行。
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
        if position == self.last_position_ms && self.state != State::Playing {
            return;
        }
        self.push_position(position, duration);
    }

    fn push_position(&mut self, position: u64, duration: u64) {
        self.last_position_ms = position;
        self.ctx.shared.set_position(position, duration);
        self.emit(Event::Position {
            ms: position,
            duration_ms: duration,
        });
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
