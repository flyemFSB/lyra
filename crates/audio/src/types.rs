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
    /// 打开曲目并返回支持读取与定位的字节流。
    fn open(&self, track: &TrackRef) -> FetchResult<Box<dyn ReadSeek + Send>>;
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
    /// 当前曲目信息（未播放过时为 `None`，播放结束保留最后曲目）。
    pub track: Option<TrackRef>,
    /// 播放进度（毫秒）。
    pub position_ms: u64,
    /// 总时长（毫秒），来自 `TrackRef`。
    pub duration_ms: u64,
    /// 音量，`1.0` 为原始增益。
    pub volume: f32,
}

/// 音频引擎向界面层推送的单次状态事件。
#[derive(Debug, Clone)]
pub enum Event {
    /// 当前曲目自然播放结束。
    Ended,
    /// 播放失败（打开、解码等）。
    Failed {
        /// 错误描述。
        message: String,
    },
}
