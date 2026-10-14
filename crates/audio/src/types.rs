//! 音频引擎对外暴露的基础数据类型与抽象接口。

/// 可播放曲目信息（包含音频 Hash、播放地址、时长与文件格式）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TrackRef {
    /// 文件 hash（内容寻址，亦作缓存键）。
    pub hash: String,
    /// 带签名的下载地址。
    pub url: String,
    /// 总时长（毫秒）。
    pub duration_ms: u64,
    /// 扩展名，如 `flac` / `mp3`。
    pub ext: String,
}

/// `Read + Seek` 的组合别名，便于 `Box<dyn ReadSeek>` 作为音频来源。
pub trait ReadSeek: std::io::Read + std::io::Seek {}

impl<T: std::io::Read + std::io::Seek> ReadSeek for T {}

/// 抓取层错误。当前只承载一段文本，具体分类留给 `lyra-api`。
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// 任意抓取失败（网络、HTTP 状态、解码缓存等）。
    #[error("{0}")]
    Message(String),
}

/// `Fetcher` 的结果别名。
pub type FetchResult<T> = std::result::Result<T, FetchError>;

/// 音频抓取器接口（支持边下边播的流式读取）。
pub trait Fetcher: Send + Sync + 'static {
    /// 打开一首曲目，返回可读可 seek 的字节流。
    ///
    /// # Errors
    /// 抓取失败（网络、签名失效、缓存缺失等）时返回 [`FetchError`]，由 `lyra-api` 分类。
    fn open(&self, track: &TrackRef) -> FetchResult<Box<dyn ReadSeek + Send>>;

    /// 预取提示，默认空实现。播放前端可在真正播放前调用以减少首帧延迟。
    fn prefetch(&self, _track: &TrackRef) {}
}

/// 播放状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// 未加载任何曲目。
    Idle,
    /// 正在打开 / 缓冲，尚未出声。
    Loading,
    /// 播放中。
    Playing,
    /// 已暂停（保留进度）。
    Paused,
}

/// 引擎状态的只读快照。`snapshot()` 返回它，永不阻塞调用方。
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// 当前状态。
    pub state: State,
    /// 当前曲目；空闲时为 `None`。
    pub track: Option<TrackRef>,
    /// 播放进度（毫秒）。
    pub position_ms: u64,
    /// 总时长（毫秒），来自 `TrackRef`。
    pub duration_ms: u64,
    /// 音量，`1.0` 为原始增益。
    pub volume: f32,
}

/// 引擎向 UI 推送的事件。
#[derive(Debug, Clone)]
pub enum Event {
    /// 状态变化。
    StateChanged(State),
    /// 曲目变化；`None` 表示已停止。
    TrackChanged(Option<TrackRef>),
    /// 进度更新，由解码线程按固定节奏发出（约每 100–200 ms）。
    Position {
        /// 当前进度（毫秒）。
        ms: u64,
        /// 总时长（毫秒）。
        duration_ms: u64,
    },
    /// 当前曲目自然播放结束。
    Ended,
    /// 播放失败（打开、解码等）。
    Failed {
        /// 错误描述。
        message: String,
    },
}
