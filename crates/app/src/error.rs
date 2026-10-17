//! 应用层错误定义

use thiserror::Error;

/// 应用层错误枚举
#[derive(Debug, Clone, Error)]
pub enum Error {
    /// 接口层错误
    #[error("api: {0}")]
    Api(String),
    /// 持久化错误
    #[error("store: {0}")]
    Store(String),
    /// 音频引擎错误
    #[error("audio: {0}")]
    Audio(String),
    /// 协议原语错误
    #[error("protocol: {0}")]
    Protocol(String),
    /// 文件系统错误
    #[error("io: {0}")]
    Io(String),
    /// 未登录错误
    #[error("需要先登录")]
    NotLoggedIn,
    /// 缺少可用播放音质错误
    #[error("没有可播放的音质（请求 {requested}；可用 {available:?}）")]
    NoPlayableQuality {
        /// 请求的音质
        requested: String,
        /// 可用的音质列表
        available: Vec<String>,
    },
    /// 需要人机行为验证
    #[error("需要行为验证（ssa-code={0}）")]
    NeedsVerify(String),
    /// 概念版无版权（已下架）
    #[error("这首歌在概念版没有版权（已下架）")]
    NoCopyright,
    /// 完整曲需会员或付费，且上游未提供试听片段
    #[error("完整版需要会员或购买，且没有试听片段")]
    NotEntitled,
    /// 非法参数错误
    #[error("参数错误: {0}")]
    Invalid(String),
}

impl From<lyra_api::Error> for Error {
    fn from(error: lyra_api::Error) -> Self {
        match error {
            lyra_api::Error::NotLoggedIn => Self::NotLoggedIn,
            lyra_api::Error::NeedsVerify(code) => Self::NeedsVerify(code),
            // 详细原因记录至日志，错误枚举仅对外暴露清晰提示
            lyra_api::Error::NoCopyright(reason) => {
                tracing::debug!(%reason, "上游判定概念版无版权");
                Self::NoCopyright
            }
            lyra_api::Error::NotEntitled(reason) => {
                tracing::debug!(%reason, "上游要求会员/购买且无试听片段");
                Self::NotEntitled
            }
            other => Self::Api(other.to_string()),
        }
    }
}

impl From<lyra_store::Error> for Error {
    fn from(error: lyra_store::Error) -> Self {
        Self::Store(error.to_string())
    }
}

impl From<lyra_proto::Error> for Error {
    fn from(error: lyra_proto::Error) -> Self {
        Self::Protocol(error.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// 应用层 Result 类型别名
pub type Result<T> = std::result::Result<T, Error>;
