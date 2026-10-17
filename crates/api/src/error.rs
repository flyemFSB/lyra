//! 传输与端点错误定义。

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

/// 传输与端点错误枚举。
#[derive(Debug, Clone, Error)]
pub enum Error {
    /// 传输层失败（DNS / TLS / 超时 / 连接中断）。
    #[error("transport failed: {0}")]
    Transport(String),
    /// HTTP 状态码非 2xx。
    #[error("http {status}: {body}")]
    Http {
        /// HTTP 状态码。
        status: u16,
        /// 响应体（截断后）。
        body: String,
    },
    /// 上游返回了业务错误码。
    #[error("upstream error_code={code} msg={message}")]
    Upstream {
        /// 上游错误码。
        code: i64,
        /// 上游错误描述。
        message: String,
    },
    /// 需要行为验证，但重试一次后仍然失败。
    #[error("upstream still requires verification after retry (ssa-code={0})")]
    NeedsVerify(String),
    /// 上游状态成功但关键字段为空，通常需短时间后重试。
    #[error("empty data: {0}")]
    EmptyData(String),
    /// 未登录但该端点需要登录态。
    #[error("login required")]
    NotLoggedIn,
    /// 响应体不是合法 JSON / 结构不符。
    #[error("decode failed: {0}")]
    Decode(String),
    /// 协议原语失败。
    #[error("protocol: {0}")]
    Protocol(String),
    /// 文件系统失败。
    #[error("io: {0}")]
    Io(String),
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<lyra_proto::Error> for Error {
    fn from(error: lyra_proto::Error) -> Self {
        Self::Protocol(error.to_string())
    }
}

impl Error {
    /// 获取上游业务错误码
    #[must_use]
    pub fn code(&self) -> Option<Code> {
        match self {
            Self::Upstream { code, .. } => Some(Code::from_i64(*code)),
            _ => None,
        }
    }
}

/// 模块通用结果类型
pub type Result<T> = std::result::Result<T, Error>;
