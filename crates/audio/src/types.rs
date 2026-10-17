//! 音频引擎基础类型与接口定义。

/// 待播放曲目引用
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TrackRef {
    /// 音频哈希（内容寻址与缓存键）
    pub hash: String,
    /// 带签名的播放地址
    pub url: String,
    /// 音频总时长（毫秒）
    pub duration_ms: u64,
    /// 音频格式扩展名（如 mp3、flac）
    pub ext: String,
}

/// 支持寻址与读取的流 trait 别名
pub trait ReadSeek: std::io::Read + std::io::Seek {}

impl<T: std::io::Read + std::io::Seek> ReadSeek for T {}

/// 音频拉取错误
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// 拉取失败描述
    #[error("{0}")]
    Message(String),
}

/// Fetcher 返回结果类型
pub type FetchResult<T> = std::result::Result<T, FetchError>;

/// 音频流拉取接口（支持边下边播）
pub trait Fetcher: Send + Sync + 'static {
    /// 打开曲目并返回可寻址的数据流
    fn open(&self, track: &TrackRef) -> FetchResult<Box<dyn ReadSeek + Send>>;
}

/// 播放状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// 空闲（未加载曲目）
    Idle,
    /// 缓冲加载中
    Loading,
    /// 播放中
    Playing,
    /// 已暂停
    Paused,
}

/// 引擎状态快照（无锁读取）
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// 当前播放状态
    pub state: State,
    /// 当前曲目
    pub track: Option<TrackRef>,
    /// 播放进度（毫秒）
    pub position_ms: u64,
    /// 总时长（毫秒）
    pub duration_ms: u64,
    /// 音量增益（1.0 为原始音量）
    pub volume: f32,
}

/// 音频引擎异步通知事件
#[derive(Debug, Clone)]
pub enum Event {
    /// 曲目正常播放结束
    Ended,
    /// 播放异常失败
    Failed {
        /// 错误信息
        message: String,
    },
}
