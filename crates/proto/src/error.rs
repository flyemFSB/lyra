//! 协议层错误。

use thiserror::Error;

/// 协议层统一错误。
///
/// 标记为 `#[non_exhaustive]`：新增变体对下游是向后兼容的破坏性变更，
/// 下游必须保留通配分支。
#[derive(Debug, Error)]
#[non_exhaustive]
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
    Crypto(#[from] CryptoError),
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
    Inflate(#[from] std::io::Error),
    /// 调用方传入的参数非法。
    #[error("invalid argument: {0}")]
    Invalid(String),
}

/// 密码学原语的底层错误。
///
/// 尽量携带具体的错误类型（而非字符串），保证 `anyhow` 错误链能追到根因。
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CryptoError {
    /// RSA 运算失败。
    #[error("rsa failed: {0}")]
    Rsa(#[from] rsa::Error),
    /// PEM 解析失败。
    #[error("pem parse failed: {0}")]
    Pem(#[from] rsa::pkcs8::spki::Error),
    /// 密钥或 IV 长度非法。
    #[error("invalid key or iv length: {0}")]
    Length(String),
    /// PKCS#7 填充非法（密文尾部字节不是合法的填充长度）。
    #[error("invalid PKCS7 padding")]
    Padding,
    /// 缓冲区不足以容纳填充后的密文。
    #[error("padding buffer too small")]
    PadBuffer,
    /// 密文长度不是分组长度的整数倍。
    #[error("ciphertext length is not a multiple of the block size")]
    BlockSize,
    /// 其它密码学错误（带说明文字）。
    #[error("{0}")]
    Other(String),
}

/// 协议层 Result 别名
pub type Result<T, E = Error> = core::result::Result<T, E>;
