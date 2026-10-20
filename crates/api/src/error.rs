//! 传输与端点错误定义。

use std::sync::Arc;

use thiserror::Error;

/// 上游业务错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// 用户 ID 类型错误或未登录（20002）
    BadUserid,
    /// 请求参数缺失（20010）
    MissingParam,
    /// 请求需二次验证（20028）
    NeedVerify,
    /// 听歌上报失败（130012，非阻断错误）
    ListenReportFailed,
    /// 每日特权领取失败（131001）
    DailyClaimFailed,
    /// 当日已领取（297002，按幂等成功处理）
    AlreadyClaimed,
    /// 未收录的上游错误码
    Other(i64),
}

impl Code {
    /// 解析上游错误码
    #[must_use]
    pub const fn from_i64(code: i64) -> Self {
        match code {
            20002 => Self::BadUserid,
            20010 => Self::MissingParam,
            20028 => Self::NeedVerify,
            130_012 => Self::ListenReportFailed,
            131_001 => Self::DailyClaimFailed,
            297_002 => Self::AlreadyClaimed,
            other => Self::Other(other),
        }
    }
}

/// 传输与端点错误
///
/// 变体带有 `#[source]`，调用方可用 `std::error::Error::source()` 取到原始原因链；
/// 底层错误以 `Arc` 装箱，保证本枚举仍可 `Clone`（单飞合并需要克隆错误结果）。
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum Error {
    /// 传输层失败（网络、超时或连接中断）
    #[error("transport failed: {0}")]
    Transport(String),
    /// HTTP 状态码非 2xx
    #[error("http {status}: {body}")]
    Http {
        /// HTTP 状态码
        status: u16,
        /// 响应体摘要
        body: String,
    },
    /// 上游返回业务错误码
    #[error("upstream error_code={code} msg={message}")]
    Upstream {
        /// 上游错误码
        code: i64,
        /// 上游错误描述
        message: String,
    },
    /// 二次验证重试后仍失败
    ///
    /// 保留上游原始错误（`source`）与原始业务码（`code`），
    /// 避免用验证错误顶掉上游真实错误码（20028 / 20010 / ...）。
    #[error("upstream still requires verification after retry (ssa-code={ssa})")]
    NeedsVerify {
        /// 上游下发的 ssa-code
        ssa: String,
        /// 上游原始业务错误码（无则为空）
        code: Option<i64>,
        /// 上游原始错误
        #[source]
        source: Box<Error>,
    },
    /// 响应成功但关键数据为空
    #[error("empty data: {0}")]
    EmptyData(String),
    /// 无版权或歌曲已下架
    #[error("no copyright: {0}")]
    NoCopyright(String),
    /// 需会员或单独购买且无试听片段
    #[error("entitlement required: {0}")]
    NotEntitled(String),
    /// 缺少登录态
    #[error("login required")]
    NotLoggedIn,
    /// 响应反序列化失败
    #[error("decode failed: {0}")]
    Decode(String),
    /// 协议处理失败
    #[error("protocol: {0}")]
    Protocol(#[source] Arc<lyra_proto::Error>),
    /// 缓存标识非法（路径穿越、非法字符等）
    #[error("invalid cache key: {0}")]
    InvalidCacheKey(String),
    /// I/O 错误
    #[error("io: {0}")]
    Io(#[source] Arc<std::io::Error>),
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(Arc::new(error))
    }
}

impl From<lyra_proto::Error> for Error {
    fn from(error: lyra_proto::Error) -> Self {
        Self::Protocol(Arc::new(error))
    }
}

impl Error {
    /// 获取上游业务错误码
    #[must_use]
    pub fn code(&self) -> Option<Code> {
        match self {
            Self::Upstream { code, .. } => Some(Code::from_i64(*code)),
            // 二次验证路径必须仍能解析出业务码：上游给了 20028 就是 NeedVerify，
            // 没给则退化为 NeedVerify，避免把验证错误伪装成“无错误码”。
            Self::NeedsVerify { code, .. } => Some(code.map_or(Code::NeedVerify, Code::from_i64)),
            _ => None,
        }
    }
}

/// 模块通用结果类型
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn io_errors_keep_their_source_chain() {
        let error = Error::from(std::io::Error::new(std::io::ErrorKind::NotFound, "缺少文件"));
        assert!(std::error::Error::source(&error).is_some(), "必须保留原始来源");
        assert!(error.to_string().contains("缺少文件"), "{error}");
    }

    #[test]
    fn protocol_errors_keep_their_source_chain() {
        let error = Error::from(lyra_proto::base64_decode("!!!").unwrap_err());
        assert!(std::error::Error::source(&error).is_some(), "必须保留原始来源");
        assert!(!error.to_string().is_empty(), "{error}");
    }

    #[test]
    fn needs_verify_keeps_the_upstream_business_code() {
        let error = Error::NeedsVerify {
            ssa: "SSA123".to_owned(),
            code: Some(20028),
            source: Box::new(Error::Upstream {
                code: 20028,
                message: "验证".to_owned(),
            }),
        };
        assert_eq!(error.code(), Some(Code::NeedVerify));
        assert!(std::error::Error::source(&error).is_some());
    }
}
