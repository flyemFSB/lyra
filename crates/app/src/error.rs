//! 应用层错误定义

use thiserror::Error;

/// 应用层错误枚举
///
/// 除少数“纯提示”变体外，所有变体都保留原始错误作为 [`std::error::Error::source`]，
/// 便于上层（UI / CLI）沿 `source()` 链定位真正的失败原因，而不是只看到一个压平后的字符串。
#[derive(Debug, Error)]
pub enum Error {
    /// 接口层错误（保留上游原始错误链）
    #[error("api: {0}")]
    Api(#[source] lyra_api::Error),
    /// 持久化错误（保留存储层原始错误链）
    #[error("store: {0}")]
    Store(#[from] lyra_store::Error),
    /// 音频引擎错误（仅用于提示性文案，底层错误在日志中记录）
    #[error("audio: {0}")]
    Audio(String),
    /// 文件系统错误
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
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
    /// 歌曲暂无版权（已下架）
    #[error("这首歌暂无版权（已下架）")]
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
            lyra_api::Error::NeedsVerify { ssa, .. } => Self::NeedsVerify(ssa),
            // 详细原因记录至日志，错误枚举仅对外暴露清晰提示
            lyra_api::Error::NoCopyright(reason) => {
                tracing::debug!(%reason, "上游判定无版权");
                Self::NoCopyright
            }
            lyra_api::Error::NotEntitled(reason) => {
                tracing::debug!(%reason, "上游要求会员/购买且无试听片段");
                Self::NotEntitled
            }
            // 其余错误保留原始类型，调用方可用 source() 继续下钻
            other => Self::Api(other),
        }
    }
}

/// 应用层 Result 类型别名
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn api_errors_keep_their_source_chain() {
        let error = Error::from(lyra_api::Error::Upstream {
            code: 20028,
            message: "需要验证".to_owned(),
        });
        let source = std::error::Error::source(&error).expect("必须保留上游错误链");
        assert!(matches!(
            source.downcast_ref::<lyra_api::Error>(),
            Some(lyra_api::Error::Upstream { code: 20028, .. })
        ));
    }

    #[test]
    fn upstream_no_copyright_is_still_flattened_to_the_friendly_variant() {
        let error = Error::from(lyra_api::Error::NoCopyright("已下架".to_owned()));
        assert!(matches!(error, Error::NoCopyright));
        assert_eq!(format!("{error}"), "这首歌暂无版权（已下架）");
    }
}
