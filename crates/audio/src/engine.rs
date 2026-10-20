//! 音频引擎控制：解码线程生命周期与跨线程播放调度。

use std::sync::Arc;
use std::thread;
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

/// 环形缓冲区目标时长（毫秒）
const RING_MS: u32 = 200;

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
    /// 输出流句柄（随引擎销毁而停止）
    _stream: cpal::Stream,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        // 这里刻意不 join：解码线程可能正阻塞在网络读上（缓存未命中时最长可达数十秒），
        // join 会把调用方（UI 线程）一起拖住，窗口关闭时会直接卡死。
        // 需要同步收尾的地方请显式调用 `Engine::close()`。
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
        // f32::clamp 会把 NaN 原样传出去（NaN 与任何边界比较都是 false），
        // 非有限值必须在这里挡掉，否则会把 NaN 样本送进声卡。
        if !v.is_finite() {
            return;
        }
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
        if self.inner.cmd_tx.send(cmd).is_err() {
            // 解码线程已经退出（例如它 panic 了）：立刻把状态拉回空闲，
            // 否则 UI 会永远显示"播放中"而实际没有声音。
            self.inner.shared.set_state(State::Idle);
        }
    }
}

/// 启动音频引擎（配置输出设备、缓冲区与解码线程）
pub fn spawn(fetcher: Arc<dyn Fetcher>, events: Box<dyn Fn(Event) + Send + Sync>) -> Result<Engine, Error> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(Error::NoOutputDevice)?;
    let supported = device
        .default_output_config()
        .map_err(|e| Error::OutputConfig(e.to_string()))?;

    let sample_rate = supported.sample_rate().0;
    let channels = usize::from(supported.channels());
    let sample_format = supported.sample_format();
    let stream_config = supported.config();

    let capacity = ring_capacity(sample_rate, channels, RING_MS);
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
    thread::Builder::new()
        .name("lyra-audio".into())
        .spawn(move || {
            // Debug 构建下捕获 panic 转成日志；release 开启 panic = "abort" 时
            // 解码异常会直接终止进程，这里的捕获只在 Debug 下生效。
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Player::new(ctx).run()));
            if result.is_err() {
                tracing::error!("音频解码线程异常退出");
            }
        })
        .map_err(|e| Error::SpawnThread(e.to_string()))?;

    Ok(Engine {
        inner: Arc::new(EngineInner {
            shared,
            cmd_tx,
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
    src_rate: u32,
    dst_channels: usize,
    dst_rate: u32,
    duration_ms: u64,
    /// 重采样器引入的延迟（帧），用于修正播放位置
    resampler_delay_frames: u64,
    /// 尾部是否已经冲洗过（只能做一次）
    resampler_flushed: bool,
    /// 待推入缓冲区的转换后样本
    pending: Vec<f32>,
    pending_off: usize,
    /// 已推入缓冲区的总帧数
    pushed_frames: u64,
}

impl Session {
    fn open(ctx: &RunCtx, track: &TrackRef) -> Result<Self, Error> {
        let reader = ctx.fetcher.open(track).map_err(|e| Error::Decode(e.to_string()))?;
        let decoder = Decoder::open(reader)?;
        let src_channels = decoder.channels();
        let src_rate = decoder.sample_rate();
        // 采样率不一致时启用重采样
        let resampler = if src_rate == ctx.sample_rate {
            None
        } else {
            Some(Resampler::new(src_rate, ctx.sample_rate, src_channels)?)
        };
        let resampler_delay_frames = resampler.as_ref().map_or(0, |r| r.output_delay_frames() as u64);
        Ok(Self {
            decoder,
            track: track.clone(),
            resampler,
            src_channels,
            src_rate,
            dst_channels: ctx.channels,
            dst_rate: ctx.sample_rate,
            duration_ms: track.duration_ms,
            resampler_delay_frames,
            resampler_flushed: false,
            pending: Vec::new(),
            pending_off: 0,
            pushed_frames: 0,
        })
    }

    /// 转换源 PCM 样本并加入待推送队列
    fn stage(&mut self, chunk: &crate::DecodedChunk) -> Result<(), Error> {
        // 容器的 codec_params 偶尔会少报声道数/采样率，
        // 以实际解码结果为准并重建重采样器，避免声道映射错乱。
        if chunk.channels != self.src_channels || chunk.sample_rate != self.src_rate {
            self.src_channels = chunk.channels;
            self.src_rate = chunk.sample_rate;
            self.resampler = if self.src_rate == self.dst_rate {
                None
            } else {
                Some(Resampler::new(self.src_rate, self.dst_rate, self.src_channels)?)
            };
            self.resampler_delay_frames = self.resampler.as_ref().map_or(0, |r| r.output_delay_frames() as u64);
        }
        let resampled = match &mut self.resampler {
            Some(r) => r.process(&chunk.samples)?,
            None => chunk.samples.clone(),
        };
        let mapped = map_channels(&resampled, self.src_channels, self.dst_channels);
        self.pending.extend_from_slice(&mapped);
        Ok(())
    }

    /// 冲洗重采样器尾部（把结尾不足一整块的样本补零处理后输出）
    fn flush_resampler(&mut self) -> Result<(), Error> {
        if self.resampler_flushed {
            return Ok(());
        }
        self.resampler_flushed = true;
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(());
        };
        let tail = resampler.flush()?;
        if tail.is_empty() {
            return Ok(());
        }
        let mapped = map_channels(&tail, self.src_channels, self.dst_channels);
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
            let Some(chunk) = self.decoder.next_chunk()? else {
                break;
            };
            skipped += (chunk.samples.len() / chunk.channels.max(1)) as u64;
        }
        Ok(())
    }

    /// 计算当前播放进度（毫秒）
    fn position_ms(&self, prod: &HeapProd<f32>) -> u64 {
        if self.dst_rate == 0 {
            return 0;
        }
        let in_ring = (prod.occupied_len() / self.dst_channels) as u64;
        // 减去重采样器的固有延迟，否则进度条会比实际听到的声音超前
        let played = self
            .pushed_frames
            .saturating_sub(in_ring)
            .saturating_sub(self.resampler_delay_frames);
        played * 1000 / u64::from(self.dst_rate)
    }
}

/// 单次数据泵状态
enum Pump {
    Progress,
    Full,
    /// 解码已结束，但尾部样本还没推完（重采样收尾等）
    Draining,
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
            // 播放中：把待处理命令一次排空。
            // 定位命令必须合并 —— 拖动进度条会灌入几十个 Seek，
            // 而每个 Seek 都要重新打开解码器并快进解码，逐个执行会把引擎卡死数分钟。
            let mut latest_seek: Option<u64> = None;
            while let Ok(cmd) = self.ctx.cmd_rx.try_recv() {
                match cmd {
                    Command::Seek(ms) => latest_seek = Some(ms),
                    Command::Shutdown => return false,
                    other => {
                        if !self.handle(other) {
                            return false;
                        }
                    }
                }
            }
            if let Some(ms) = latest_seek {
                self.seek(ms);
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
                // 等待缓冲区腾出空间：短暂休眠，避免空转烧满一个核
                Pump::Progress => thread::sleep(Duration::from_millis(1)),
                // 环形缓冲已满 / 尾部样本还没推完：继续排空
                Pump::Full | Pump::Draining => thread::sleep(Duration::from_millis(2)),
                Pump::Ended => {
                    // 等待缓冲区尾音播放完毕（超时 2 秒防挂死）
                    let mut waited = 0_u32;
                    while self.ctx.prod.occupied_len() > 0 && waited < 400 {
                        thread::sleep(Duration::from_millis(5));
                        waited += 1;
                    }
                    // 超时后环形缓冲里仍有残留样本，必须显式请求清空，
                    // 否则这些尾音会留到下一首歌开头才被丢弃。
                    if self.ctx.prod.occupied_len() > 0 {
                        self.ctx.shared.request_flush();
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
        self.report_stream_error();
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
        // 刷新握手：回调还没来得及清空环形缓冲时，此刻写入的样本会被一起清掉
        // （表现为每首歌开头被削掉几十毫秒）。必须等回调消费掉刷新请求再写。
        if self.ctx.shared.flush_pending() {
            return Pump::Progress;
        }
        let Self { ctx, session, .. } = self;
        let Some(session) = session.as_mut() else {
            return Pump::Progress;
        };
        if !session.flush(&mut ctx.prod) {
            return Pump::Full;
        }
        match session.decoder.next_chunk() {
            Ok(Some(chunk)) => {
                if let Err(e) = session.stage(&chunk) {
                    return Pump::Failed(e.to_string());
                }
                session.flush(&mut ctx.prod);
                Pump::Progress
            }
            Ok(None) => {
                // 冲洗重采样器尾部，否则结尾几十毫秒会被丢掉
                if let Err(e) = session.flush_resampler() {
                    return Pump::Failed(e.to_string());
                }
                // 尾部样本可能因环形缓冲已满而没推完，需要继续排空
                if !session.flush(&mut ctx.prod) {
                    return Pump::Draining;
                }
                Pump::Ended
            }
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

    /// 打印输出流错误（实时线程只记录，这里由非实时线程负责输出日志）
    fn report_stream_error(&mut self) {
        if let Some(message) = self.ctx.shared.take_stream_error() {
            tracing::error!("音频输出流错误: {message}");
        }
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

    #[test]
    fn a_non_finite_volume_is_ignored() {
        // f32::clamp 会把 NaN 原样放行，必须在入口挡掉，
        // 否则 NaN 样本会被送进声卡。
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
        engine.set_volume(0.5);
        assert!((engine.snapshot().volume - 0.5).abs() < f32::EPSILON);
        engine.set_volume(f32::NAN);
        assert!(
            (engine.snapshot().volume - 0.5).abs() < f32::EPSILON,
            "NaN 音量必须被忽略，而不是写进引擎"
        );
    }
}
