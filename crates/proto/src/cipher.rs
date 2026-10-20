//! 密码学原语：哈希、对称加密（AES-CBC）、非对称加密（RSA）与 KRC 歌词解码。

use std::fmt;
use std::sync::OnceLock;

use aes::cipher::block_padding::{Pkcs7, RawPadding};
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::NoPadding};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use md5::{Digest as _, Md5};
use rsa::RsaPublicKey;
// 大数模幂计算：rsa 0.9 使用 num-bigint-dig，其 `e()` / `n()` 就是这个类型
use rsa::BigUint;
use rsa::pkcs8::DecodePublicKey as _;
use rsa::traits::PublicKeyParts as _;
use zeroize::Zeroize as _;

use crate::consts::{CHARSET_UPPER, FP_IV, FP_RSA_PUBLIC, KRC_KEY, RSA_PUBLIC};
use crate::error::{CryptoError, Error, Result};
use crate::rng::JsRng;

/// 计算 MD5 哈希（32 位小写十六进制）
#[must_use]
pub fn md5_hex(data: &[u8]) -> String {
    md5_hex_parts(&[data])
}

/// 依次拼接多段数据后计算 MD5（签名按「盐 + 参数 + 体」的顺序拼接）
pub(crate) fn md5_hex_parts(parts: &[&[u8]]) -> String {
    let mut hasher = Md5::new();
    for part in parts {
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

/// 由会话密钥派生 AES-128 的 key 与 iv（md5 前 16 字节为 key，后 16 字节为 iv）
fn secret_key_iv(secret: &str) -> ([u8; 16], [u8; 16]) {
    let digest = md5_hex(secret.as_bytes());
    let bytes = digest.as_bytes();
    let mut key = [0_u8; 16];
    let mut iv = [0_u8; 16];
    key.copy_from_slice(&bytes[0..16]);
    iv.copy_from_slice(&bytes[16..32]);
    (key, iv)
}

/// 由临时密钥派生 AES-256 的 key 与 iv（iv 取自 key 的后 16 字节）
fn derived_key_iv(temp_key: &str) -> ([u8; 32], [u8; 16]) {
    let digest = md5_hex(temp_key.as_bytes());
    let bytes = digest.as_bytes();
    let mut key = [0_u8; 32];
    let mut iv = [0_u8; 16];
    key.copy_from_slice(&bytes[0..32]);
    iv.copy_from_slice(&bytes[16..32]);
    (key, iv)
}

/// 计算 SHA-256 哈希（64 位小写十六进制）
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(data))
}

/// Base64 编码
#[must_use]
pub fn base64_encode(data: &[u8]) -> String {
    B64.encode(data)
}

/// Base64 解码
pub fn base64_decode(s: &str) -> Result<Vec<u8>> {
    Ok(B64.decode(s)?)
}

// AES-CBC 加解密

/// 密钥或 IV 长度非法的转换
fn length_err(_: aes::cipher::InvalidLength) -> Error {
    Error::Crypto(CryptoError::Length("invalid AES key or iv length".to_owned()))
}

fn cbc_encrypt(key: &[u8], iv: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    // 拷贝明文并执行 PKCS7 填充加密
    let mut buf = vec![0_u8; plain.len() + 16];
    buf[..plain.len()].copy_from_slice(plain);
    let ct_len = match key.len() {
        16 => cbc::Encryptor::<aes::Aes128>::new_from_slices(key, iv)
            .map_err(length_err)?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
            .map_err(|_| Error::Crypto(CryptoError::PadBuffer))?
            .len(),
        32 => cbc::Encryptor::<aes::Aes256>::new_from_slices(key, iv)
            .map_err(length_err)?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
            .map_err(|_| Error::Crypto(CryptoError::PadBuffer))?
            .len(),
        n => {
            return Err(Error::Crypto(CryptoError::Length(format!(
                "unsupported AES key length {n}"
            ))));
        }
    };
    buf.truncate(ct_len);
    Ok(buf)
}

fn cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut buf = data.to_vec();
    let n = match key.len() {
        16 => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv)
            .map_err(length_err)?
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(|_| Error::Crypto(CryptoError::BlockSize))?
            .len(),
        32 => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
            .map_err(length_err)?
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(|_| Error::Crypto(CryptoError::BlockSize))?
            .len(),
        n => {
            return Err(Error::Crypto(CryptoError::Length(format!(
                "unsupported AES key length {n}"
            ))));
        }
    };
    buf.truncate(n);
    unpad_pkcs7(buf)
}

/// 校验并移除 PKCS7 填充（RFC 5652）。
///
/// 复用 `block_padding::Pkcs7` 的实现：填充长度只能是 1..=16（AES 分组大小），
/// 否则视为密文被篡改。
fn unpad_pkcs7(mut data: Vec<u8>) -> Result<Vec<u8>> {
    let Some(&last) = data.last() else {
        return Ok(data);
    };
    let n = usize::from(last);
    // 填充长度只能是 1..=16（AES 分组大小），且密文必须是完整分组
    if !(1..=16).contains(&n) || n > data.len() || !data.len().is_multiple_of(16) {
        return Err(Error::Crypto(CryptoError::Padding));
    }
    // `raw_unpad` 只接受单个分组（超过 255 字节或空分组会 panic），因此只对最后一个分组调用。
    let block_start = data.len() - 16;
    let plain = Pkcs7::raw_unpad(&data[block_start..]).map_err(|_| Error::Crypto(CryptoError::Padding))?;
    let len = block_start + plain.len();
    data.truncate(len);
    Ok(data)
}

/// 生成指定长度随机字符串（对齐逆向 JS 的向上取整逻辑）
pub fn random_string_with(rng: &mut impl JsRng, len: usize) -> String {
    let alphabet = CHARSET_UPPER;
    (0..len)
        .map(|_| {
            let idx = crate::rng::js_ceil((alphabet.len() - 1) as f64 * rng.next_f64()) as usize;
            char::from(alphabet[idx.min(alphabet.len() - 1)])
        })
        .collect()
}

/// 使用系统熵源生成指定长度随机字符串
#[must_use]
pub fn random_string(len: usize) -> String {
    random_string_with(&mut crate::rng::OsRng, len)
}

/// 派生密钥 AES 加密结果
#[derive(Clone, PartialEq, Eq)]
pub struct AesBox {
    /// 十六进制密文
    pub hex: String,
    /// 临时密钥（用于附加在请求参数中）
    pub key: String,
}

impl fmt::Debug for AesBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 密钥不得出现在日志里
        f.debug_struct("AesBox")
            .field("hex", &self.hex)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl Drop for AesBox {
    fn drop(&mut self) {
        // 临时密钥不得留在内存里
        self.key.zeroize();
    }
}

/// 随机派生密钥 AES 加密（生成 16 位临时密钥并派生密钥与 IV）
pub fn aes_encrypt_derived(plaintext: &str) -> Result<AesBox> {
    let temp_key = random_string(16).to_lowercase();
    aes_encrypt_derived_with(plaintext, &temp_key)
}

/// 基于指定临时密钥派生 AES 加密
pub fn aes_encrypt_derived_with(plaintext: &str, temp_key: &str) -> Result<AesBox> {
    let (key, iv) = derived_key_iv(temp_key);
    let ct = cbc_encrypt(&key, &iv, plaintext.as_bytes())?;
    Ok(AesBox {
        hex: hex::encode(ct),
        key: temp_key.to_owned(),
    })
}

/// 显式 key 与 iv 的 AES 加密
pub fn aes_encrypt_with_key_iv(plaintext: &str, key: &str, iv: &str) -> Result<String> {
    Ok(hex::encode(cbc_encrypt(
        key.as_bytes(),
        iv.as_bytes(),
        plaintext.as_bytes(),
    )?))
}

/// 基于临时密钥派生 AES 解密
pub fn aes_decrypt_derived(data_hex: &str, temp_key: &str) -> Result<Vec<u8>> {
    let (key, iv) = derived_key_iv(temp_key);
    cbc_decrypt(&key, &iv, &hex::decode(data_hex)?)
}

/// 会话加密结果
#[derive(Clone, PartialEq, Eq)]
pub struct SecretBox {
    /// 会话密钥
    pub key: String,
    /// Base64 密文
    pub base64: String,
}

impl fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 会话密钥不得出现在日志里
        f.debug_struct("SecretBox")
            .field("key", &"<redacted>")
            .field("base64", &self.base64)
            .finish()
    }
}

impl Drop for SecretBox {
    fn drop(&mut self) {
        // 会话密钥不得留在内存里
        self.key.zeroize();
    }
}

/// 会话加密（生成 6 位随机密钥并执行 AES-128-CBC 加密）
pub fn secret_encrypt(plaintext: &str) -> Result<SecretBox> {
    let key = random_string(6).to_lowercase();
    secret_encrypt_with(plaintext, &key)
}

/// 基于指定会话密钥执行加密
pub fn secret_encrypt_with(plaintext: &str, secret: &str) -> Result<SecretBox> {
    let (key, iv) = secret_key_iv(secret);
    let ct = cbc_encrypt(&key, &iv, plaintext.as_bytes())?;
    Ok(SecretBox {
        key: secret.to_owned(),
        base64: base64_encode(&ct),
    })
}

/// 解密 Base64 格式会话密文
pub fn secret_decrypt_base64(cipher_b64: &str, secret: &str) -> Result<Vec<u8>> {
    let (key, iv) = secret_key_iv(secret);
    cbc_decrypt(&key, &iv, &base64_decode(cipher_b64)?)
}

/// 解密原始字节会话密文
pub fn secret_decrypt_bytes(cipher: &[u8], secret: &str) -> Result<Vec<u8>> {
    let (key, iv) = secret_key_iv(secret);
    cbc_decrypt(&key, &iv, cipher)
}

// RSA 与指纹加密

/// 业务 RSA 公钥的解析缓存（公钥是编译期常量，只需解析一次）。
static RSA_KEY_CACHE: OnceLock<RsaPublicKey> = OnceLock::new();
/// 指纹 RSA 公钥的解析缓存（与业务公钥**分开**缓存）。
static FP_RSA_KEY_CACHE: OnceLock<RsaPublicKey> = OnceLock::new();

/// 从缓存中取公钥；未命中时解析一次并写入缓存。
fn cached_key(cache: &'static OnceLock<RsaPublicKey>, pem: &'static str) -> Result<&'static RsaPublicKey> {
    if let Some(key) = cache.get() {
        return Ok(key);
    }
    let key = RsaPublicKey::from_public_key_pem(pem).map_err(CryptoError::Pem)?;
    // 并发写入时可能已有别的线程写入同一份密钥，忽略失败即可。
    let _ = cache.set(key);
    match cache.get() {
        Some(key) => Ok(key),
        None => Err(Error::Crypto(CryptoError::Other(
            "rsa public key cache is unavailable".to_owned(),
        ))),
    }
}

/// 取公钥：编译期常量走 `OnceLock` 缓存，其它 PEM 一次性解析。
fn with_public_key<T>(pem: &str, f: impl FnOnce(&RsaPublicKey) -> Result<T>) -> Result<T> {
    if pem == RSA_PUBLIC {
        return f(cached_key(&RSA_KEY_CACHE, RSA_PUBLIC)?);
    }
    if pem == FP_RSA_PUBLIC {
        return f(cached_key(&FP_RSA_KEY_CACHE, FP_RSA_PUBLIC)?);
    }
    // 非常量 PEM：无法安全缓存，直接解析一次。
    let key = RsaPublicKey::from_public_key_pem(pem).map_err(CryptoError::Pem)?;
    f(&key)
}

/// 业务 RSA 无填充模幂加密（右补零对齐逆向逻辑，输出小写十六进制）
pub fn rsa_raw_encrypt_hex(data: &[u8], pem: &str) -> Result<String> {
    with_public_key(pem, |key| {
        let key_bytes = key.size();
        if data.len() > key_bytes {
            return Err(Error::PayloadTooLong {
                len: data.len(),
                key_bytes,
            });
        }
        let mut padded = vec![0_u8; key_bytes];
        padded[..data.len()].copy_from_slice(data);
        let c = BigUint::from_bytes_be(&padded).modpow(key.e(), key.n());
        let mut out = vec![0_u8; key_bytes];
        let raw = c.to_bytes_be();
        out[key_bytes - raw.len()..].copy_from_slice(&raw);
        Ok(hex::encode(out))
    })
}

/// RSA PKCS#1 v1.5 加密（输出**大写**十六进制）。
///
/// 方案 §5.2 / §5.5 明确要求 cloudlist 的 `p` 字段为大写十六进制。
pub fn rsa_pkcs1_encrypt_hex(data: &[u8], pem: &str) -> Result<String> {
    with_public_key(pem, |key| {
        let mut rng = rand::thread_rng();
        let ct = key
            .encrypt(&mut rng, rsa::Pkcs1v15Encrypt, data)
            .map_err(CryptoError::Rsa)?;
        Ok(hex::encode_upper(ct))
    })
}

/// 行为指纹 SID 加密（RSA-OAEP-SHA256，输出 Base64）
pub fn rsa_oaep_sha256_base64(data: &[u8], pem: &str) -> Result<String> {
    with_public_key(pem, |key| {
        let mut rng = rand::thread_rng();
        let ct = key
            .encrypt(&mut rng, rsa::Oaep::new::<sha2::Sha256>(), data)
            .map_err(CryptoError::Rsa)?;
        Ok(base64_encode(&ct))
    })
}

/// 行为指纹 EDT 加密（AES-128-CBC 固定 IV，输出 Base64）
pub fn fingerprint_edt(plaintext: &str, key16: &str) -> Result<String> {
    // 校验密钥长度为 16 字节（AES-128）
    if key16.len() != 16 {
        return Err(Error::Invalid(format!(
            "fingerprint_edt 需要 16 字节密钥，收到 {} 字节",
            key16.len()
        )));
    }
    // 密钥必须是 16 位 ASCII 十六进制（上游取 md5 前 16 位）
    if !key16.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid(
            "fingerprint_edt 密钥必须是 16 位 ASCII 十六进制".to_owned(),
        ));
    }
    Ok(base64_encode(&cbc_encrypt(
        key16.as_bytes(),
        FP_IV,
        plaintext.as_bytes(),
    )?))
}

// KRC 歌词解密

/// KRC 歌词解密（跳过 4 字节头部后异或解密并 zlib 解压）
pub fn krc_decode(data: &[u8]) -> Result<String> {
    // 限制最大解压体积（8MB），防止解压炸弹
    const MAX_KRC_BYTES: u64 = 8 * 1024 * 1024;
    use std::io::Read as _;

    if data.len() < 4 {
        return Err(Error::Crypto(CryptoError::Other(
            "krc payload shorter than 4 bytes".to_owned(),
        )));
    }
    let mut buf = data[4..].to_vec();
    for (i, byte) in buf.iter_mut().enumerate() {
        *byte ^= KRC_KEY[i % KRC_KEY.len()];
    }
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&buf[..])
        .take(MAX_KRC_BYTES)
        .read_to_end(&mut out)
        .map_err(Error::Inflate)?;
    if out.len() as u64 >= MAX_KRC_BYTES {
        return Err(Error::Inflate(std::io::Error::other(format!(
            "krc 解压结果超过 {MAX_KRC_BYTES} 字节上限"
        ))));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// KRC 歌词解密（输入 Base64）
pub fn krc_decode_base64(b64: &str) -> Result<String> {
    krc_decode(&base64_decode(b64)?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn md5_matches_known_vector() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn aes_derived_key_is_256_bit() {
        // MD5 十六进制字符串直接作为 32 字节密钥
        let boxed = aes_encrypt_derived_with("hello", "abcdefghijklmnop").unwrap();
        let key = &md5_hex(b"abcdefghijklmnop")[0..32];
        assert_eq!(key.len(), 32);
        let back = cbc_decrypt(
            key.as_bytes(),
            &key.as_bytes()[16..32],
            &hex::decode(&boxed.hex).unwrap(),
        )
        .unwrap();
        assert_eq!(back, b"hello");
    }

    #[test]
    fn aes_with_explicit_key_iv_round_trips() {
        const KEY: &str = "5e4ef500e9597fe004bd09a46d8add98";
        const IV: &str = "04bd09a46d8add98";
        let ct = aes_encrypt_with_key_iv("|1750000000000", KEY, IV).unwrap();
        let pt = cbc_decrypt(KEY.as_bytes(), IV.as_bytes(), &hex::decode(&ct).unwrap()).unwrap();
        assert_eq!(pt, b"|1750000000000");
    }

    #[test]
    fn secret_box_round_trips() {
        let boxed = secret_encrypt_with(r#"{"aes":"x","uid":1}"#, "123456").unwrap();
        let pt = secret_decrypt_base64(&boxed.base64, "123456").unwrap();
        assert_eq!(pt, br#"{"aes":"x","uid":1}"#);
    }

    #[test]
    fn secret_material_is_redacted_in_debug_output() {
        // 密钥不得出现在 Debug 输出里
        let boxed = secret_encrypt_with("x", "123456").unwrap();
        let text = format!("{boxed:?}");
        assert!(!text.contains("123456"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");

        let aes = aes_encrypt_derived_with("x", "abcdefghijklmnop").unwrap();
        let text = format!("{aes:?}");
        assert!(!text.contains("abcdefghijklmnop"), "{text}");
    }

    #[test]
    fn rsa_raw_output_is_modulus_sized() {
        let hex = rsa_raw_encrypt_hex(b"{}", crate::consts::RSA_PUBLIC).unwrap();
        assert_eq!(hex.len(), 128 * 2, "1024-bit key → 128 bytes → 256 hex chars");
    }

    #[test]
    fn rsa_raw_rejects_oversized_payload() {
        let err = rsa_raw_encrypt_hex(&[0_u8; 200], crate::consts::RSA_PUBLIC).unwrap_err();
        assert!(matches!(err, Error::PayloadTooLong { .. }));
    }

    #[test]
    fn rsa_pkcs1_output_is_uppercase_hex() {
        // 方案 §5.2 / §5.5：cloudlist 的 p 字段是大写十六进制
        let hex = rsa_pkcs1_encrypt_hex(b"{}", crate::consts::RSA_PUBLIC).unwrap();
        assert_eq!(hex.len(), 256);
        assert!(
            hex.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()),
            "p 字段必须是大写十六进制：{hex}"
        );
    }

    #[test]
    fn krc_decode_skips_header_and_inflates() {
        // 构造测试 KRC 载荷：压缩后异或并添加头部魔数
        let plain = "<krc>hello</krc>";
        let mut deflated = Vec::new();
        {
            use std::io::Write as _;
            let mut enc = flate2::write::ZlibEncoder::new(&mut deflated, flate2::Compression::default());
            enc.write_all(plain.as_bytes()).unwrap();
            enc.finish().unwrap();
        }
        for (i, byte) in deflated.iter_mut().enumerate() {
            *byte ^= KRC_KEY[i % KRC_KEY.len()];
        }
        let mut blob = vec![0x6b, 0x72, 0x63, 0x31];
        blob.extend_from_slice(&deflated);
        assert_eq!(krc_decode(&blob).unwrap(), plain);
    }

    #[test]
    fn krc_decode_rejects_a_decompression_bomb() {
        // 超过 8MB 上限应拒绝解压，防止解压炸弹
        let mut deflated = Vec::new();
        {
            use std::io::Write as _;
            let mut enc = flate2::write::ZlibEncoder::new(&mut deflated, flate2::Compression::fast());
            enc.write_all(&vec![b'x'; 9 * 1024 * 1024]).unwrap();
            enc.finish().unwrap();
        }
        let mut blob = vec![0x6b, 0x72, 0x63, 0x31];
        blob.extend_from_slice(&deflated);
        assert!(krc_decode(&blob).is_err());
    }

    #[test]
    fn pkcs7_padding_must_be_well_formed() {
        // 末字节声明填充 5 字节但前序字节不匹配时必须报错
        assert!(unpad_pkcs7(b"0123456789abcde\x05".to_vec()).is_err());
        // 合法填充：11 字节明文 + 5 字节 0x05
        assert_eq!(
            unpad_pkcs7(b"0123456789a\x05\x05\x05\x05\x05".to_vec()).unwrap(),
            b"0123456789a"
        );
        // 校验非密文输入（末尾全 0 填充）
        assert!(unpad_pkcs7(b"{\"status\":1}\x00\x00\x00".to_vec()).is_err());
        // 填充长度超出分组大小同样非法
        assert!(unpad_pkcs7(b"0123456789abcde\x11".to_vec()).is_err());
    }

    #[test]
    fn fingerprint_edt_requires_a_16_byte_key() {
        assert!(fingerprint_edt("x", "0123456789abcdef").is_ok());
        assert!(
            fingerprint_edt("x", &"a".repeat(32)).is_err(),
            "32 字节不得静默升级成 AES-256"
        );
        // 非十六进制密钥必须被拒绝
        assert!(fingerprint_edt("x", "zzzzzzzzzzzzzzzz").is_err());
    }
}
