//! 协议层错误。

use thiserror::Error;

/// 协议层统一错误。
#[derive(Debug, Error)]
pub enum Error {
    /// 十六进制解码失败。
    #[error("hex decode failed: {0}")]
    Hex(#[from] hex::FromHexError),
    /// Base64 解码失败。
    #[error("base64 decode failed: {0}")]
    Base64(#[from] base64::DecodeError),
    /// JSON 序列化 / 反序列化失败。
    #[error("json failed: {0}")]
    Json(#[from] serde_json::Error),
    /// 加密 / 解密失败。
    #[error("crypto failed: {0}")]
    Crypto(String),
    /// 明文超过 RSA 模长
    #[error("plaintext of {len} bytes exceeds the {key_bytes}-byte RSA modulus")]
    PayloadTooLong {
        /// 明文长度
        len: usize,
        /// 密钥模长字节数
        key_bytes: usize,
    },
    /// 解压失败。
    #[error("inflate failed: {0}")]
    Inflate(String),
}

/// 协议层 Result 别名
pub type Result<T> = std::result::Result<T, Error>;
