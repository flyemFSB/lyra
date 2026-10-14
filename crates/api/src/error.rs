//! 传输与端点错误。

use thiserror::Error;

/// 上游业务错误码（方案 §5.7 的实测错误码表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// `20002`：userid 类型错 / 未登录。
    BadUserid,
    /// `20010`：参数缺失（例如歌词未带 id/accesskey）。
    MissingParam,
    /// `20028`：本次请求需要验证 → 带 `sid`/`edt` 重试一次。
    NeedVerify,
    /// `130012`：听歌上报失败，忽略，不阻断。
    ListenReportFailed,
    /// `131001`：每日领取失败。
    DailyClaimFailed,
    /// `297002`：已领取过 → 按**幂等成功**处理，不报错。
    AlreadyClaimed,
    /// 未收录的上游错误码。
    Other(i64),
}

impl Code {
    /// 从上游返回的 `error_code` 归类。
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

    /// 该错误码是否应被视为成功（幂等）。
    #[must_use]
    pub const fn is_idempotent_success(self) -> bool {
        matches!(self, Self::AlreadyClaimed)
    }

    /// 该错误码是否可以被安全忽略。
    #[must_use]
    pub const fn is_ignorable(self) -> bool {
        matches!(self, Self::ListenReportFailed)
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
    /// 未登录但该端点需要登录态。
    #[error("login required")]
    NotLoggedIn,
    /// 响应体不是合法 JSON / 结构不符。
    #[error("decode failed: {0}")]
    Decode(String),
    /// JSON 解析失败。
    #[error("json: {0}")]
    Json(String),
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

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

impl From<lyra_proto::Error> for Error {
    fn from(error: lyra_proto::Error) -> Self {
        Self::Protocol(error.to_string())
    }
}

impl Error {
    /// 取上游错误码（若有）。
    #[must_use]
    pub fn code(&self) -> Option<Code> {
        match self {
            Self::Upstream { code, .. } => Some(Code::from_i64(*code)),
            _ => None,
        }
    }
}

/// 结果别名。
pub type Result<T> = std::result::Result<T, Error>;
