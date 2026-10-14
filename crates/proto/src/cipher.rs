//! 密码学原语：MD5、SHA、AES-CBC、RSA 与 KRC 歌词编解码。

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::NoPadding};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use md5::{Digest as _, Md5};
use rand::Rng as _;
use rsa::RsaPublicKey;
// 使用 rsa crate 内部的 BigUint 执行大数模幂运算
use rsa::BigUint;
use rsa::pkcs8::DecodePublicKey as _;
use rsa::traits::PublicKeyParts as _;

use crate::consts::{CHARSET_B62, CHARSET_UPPER, FP_IV, KRC_KEY};
use crate::error::{Error, Result};
use crate::rng::JsRng;

fn cerr<E: core::fmt::Display>(e: E) -> Error {
    Error::Crypto(e.to_string())
}

/// 计算输入字节的 MD5 哈希（32 位小写十六进制字符串）。
#[must_use]
pub fn md5_hex(data: &[u8]) -> String {
    hex::encode(Md5::digest(data))
}

/// 计算输入字节的 SHA-256 哈希（64 位小写十六进制字符串）。
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(data))
}

/// Base64 编码。
#[must_use]
pub fn base64_encode(data: &[u8]) -> String {
    B64.encode(data)
}

/// Base64 解码。
pub fn base64_decode(s: &str) -> Result<Vec<u8>> {
    Ok(B64.decode(s)?)
}

// AES-CBC 加解密核心实现

fn cbc_encrypt(key: &[u8], iv: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    // 拷贝明文至目标缓冲并执行 PKCS7 填充加密
    let mut buf = vec![0_u8; plain.len() + 16];
    buf[..plain.len()].copy_from_slice(plain);
    let ct_len = match key.len() {
        16 => cbc::Encryptor::<aes::Aes128>::new_from_slices(key, iv)
            .map_err(cerr)?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
            .map_err(cerr)?
            .len(),
        32 => cbc::Encryptor::<aes::Aes256>::new_from_slices(key, iv)
            .map_err(cerr)?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
            .map_err(cerr)?
            .len(),
        n => return Err(Error::Crypto(format!("unsupported AES key length {n}"))),
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
            .map_err(cerr)?
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(cerr)?
            .len(),
        32 => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
            .map_err(cerr)?
            .decrypt_padded_mut::<NoPadding>(&mut buf)
            .map_err(cerr)?
            .len(),
        n => return Err(Error::Crypto(format!("unsupported AES key length {n}"))),
    };
    buf.truncate(n);
    unpad_pkcs7(buf)
}

/// 严格按 RFC 5652 校验并移除 PKCS7 填充。
fn unpad_pkcs7(data: Vec<u8>) -> Result<Vec<u8>> {
    let Some(&last) = data.last() else {
        return Ok(data);
    };
    let n = last as usize;
    let padding_ok =
        (1..=16).contains(&n) && n <= data.len() && data[data.len() - n..].iter().all(|byte| usize::from(*byte) == n);
    if !padding_ok {
        return Err(Error::Crypto(format!("invalid PKCS7 padding (last byte {n})")));
    }
    let mut data = data;
    data.truncate(data.len() - n);
    Ok(data)
}

/// 生成指定长度随机字符串（向上取整索引以对齐 JS 参考实现）。
pub fn random_string_with(rng: &mut impl JsRng, len: usize) -> String {
    let alphabet = CHARSET_UPPER;
    (0..len)
        .map(|_| {
            let idx = crate::rng::js_ceil((alphabet.len() - 1) as f64 * rng.next_f64()) as usize;
            char::from(alphabet[idx.min(alphabet.len() - 1)])
        })
        .collect()
}

/// 生成指定长度随机字符串（使用操作系统熵源）。
#[must_use]
pub fn random_string(len: usize) -> String {
    random_string_with(&mut crate::rng::OsRng, len)
}

/// 生成 6 位随机 Base62 云歌单会话串。
#[must_use]
pub fn random_aes_secret() -> String {
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| char::from(CHARSET_B62[rng.gen_range(0..CHARSET_B62.len())]))
        .collect()
}

/// 派生密钥 AES 加密结果（包含十六进制密文与临时密钥）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AesBox {
    /// 密文（十六进制字符串）。
    pub hex: String,
    /// 临时密钥（用于携带在请求参数中）。
    pub key: String,
}

/// 随机派生密钥 AES 加密（生成 16 位临时密钥并派生 32 字节 AES-256 密钥与 IV）。
pub fn aes_encrypt_derived(plaintext: &str) -> Result<AesBox> {
    let temp_key = random_string(16).to_lowercase();
    aes_encrypt_derived_with(plaintext, &temp_key)
}

/// 指定临时密钥派生 AES 加密。
pub fn aes_encrypt_derived_with(plaintext: &str, temp_key: &str) -> Result<AesBox> {
    let digest = md5_hex(temp_key.as_bytes());
    let key = &digest[0..32];
    let iv = &key[16..32];
    let ct = cbc_encrypt(key.as_bytes(), iv.as_bytes(), plaintext.as_bytes())?;
    Ok(AesBox {
        hex: hex::encode(ct),
        key: temp_key.to_owned(),
    })
}

/// 指定临时密钥单向派生 AES 加密（仅返回密文字符串）。
pub fn aes_encrypt_with_key_iv_derived(plaintext: &str, temp_key: &str) -> Result<String> {
    Ok(aes_encrypt_derived_with(plaintext, temp_key)?.hex)
}

/// `cryptoAesEncrypt(data, { key, iv })`：key/iv 显式给出，按 UTF-8 字节使用。
pub fn aes_encrypt_with_key_iv(plaintext: &str, key: &str, iv: &str) -> Result<String> {
    Ok(hex::encode(cbc_encrypt(
        key.as_bytes(),
        iv.as_bytes(),
        plaintext.as_bytes(),
    )?))
}

/// 派生密钥 AES 解密（使用临时密钥哈希派生 32 字节密钥与 IV）。
pub fn aes_decrypt_derived(data_hex: &str, temp_key: &str) -> Result<Vec<u8>> {
    let digest = md5_hex(temp_key.as_bytes());
    let key = &digest[0..32];
    let iv = &key[16..32];
    cbc_decrypt(key.as_bytes(), iv.as_bytes(), &hex::decode(data_hex)?)
}

/// 云歌单与注册会话加密结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretBox {
    /// 6 位会话密钥。
    pub key: String,
    /// 密文（Base64 字符串）。
    pub base64: String,
}

/// 云歌单与注册会话加密（生成 6 位随机密钥并执行 AES-128-CBC 加密）。
pub fn secret_encrypt(plaintext: &str) -> Result<SecretBox> {
    let key = random_string(6).to_lowercase();
    secret_encrypt_with(plaintext, &key)
}

/// 使用指定会话密钥执行会话加密。
pub fn secret_encrypt_with(plaintext: &str, secret: &str) -> Result<SecretBox> {
    let digest = md5_hex(secret.as_bytes());
    let ct = cbc_encrypt(
        &digest.as_bytes()[0..16],
        &digest.as_bytes()[16..32],
        plaintext.as_bytes(),
    )?;
    Ok(SecretBox {
        key: secret.to_owned(),
        base64: base64_encode(&ct),
    })
}

/// Base64 格式会话密分解密。
pub fn secret_decrypt_base64(cipher_b64: &str, secret: &str) -> Result<Vec<u8>> {
    let digest = md5_hex(secret.as_bytes());
    cbc_decrypt(
        &digest.as_bytes()[0..16],
        &digest.as_bytes()[16..32],
        &base64_decode(cipher_b64)?,
    )
}

/// 原始字节格式会话密分解密。
pub fn secret_decrypt_bytes(cipher: &[u8], secret: &str) -> Result<Vec<u8>> {
    let digest = md5_hex(secret.as_bytes());
    cbc_decrypt(&digest.as_bytes()[0..16], &digest.as_bytes()[16..32], cipher)
}

/// 云请求体加密（输出原始密文字节流）。
pub fn secret_encrypt_bytes(plaintext: &str, secret: &str) -> Result<Vec<u8>> {
    let digest = md5_hex(secret.as_bytes());
    cbc_encrypt(
        &digest.as_bytes()[0..16],
        &digest.as_bytes()[16..32],
        plaintext.as_bytes(),
    )
}

// RSA 加解密与指纹加密实现

/// 业务 RSA 无填充模幂加密（明文右补零对齐参考实现，输出小写十六进制）。
pub fn rsa_raw_encrypt_hex(data: &[u8], pem: &str) -> Result<String> {
    let key = RsaPublicKey::from_public_key_pem(pem).map_err(cerr)?;
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
}

/// RSA PKCS#1 v1.5 加密（输出小写十六进制字符串）。
pub fn rsa_pkcs1_encrypt_hex(data: &[u8], pem: &str) -> Result<String> {
    let key = RsaPublicKey::from_public_key_pem(pem).map_err(cerr)?;
    let mut rng = rand::thread_rng();
    let ct = key.encrypt(&mut rng, rsa::Pkcs1v15Encrypt, data).map_err(cerr)?;
    Ok(hex::encode(ct))
}

/// 行为指纹 SID 加密：RSA-OAEP-SHA256（输出 Base64 字符串）。
pub fn rsa_oaep_sha256_base64(data: &[u8], pem: &str) -> Result<String> {
    let key = RsaPublicKey::from_public_key_pem(pem).map_err(cerr)?;
    let mut rng = rand::thread_rng();
    let ct = key
        .encrypt(&mut rng, rsa::Oaep::new::<sha2::Sha256>(), data)
        .map_err(cerr)?;
    Ok(base64_encode(&ct))
}

/// 行为指纹 EDT 加密：AES-128-CBC 固定 IV（输出 Base64 字符串）。
pub fn fingerprint_edt(plaintext: &str, key16: &str) -> Result<String> {
    // 校验密钥为 16 字节，防止误用 AES-256 导致解密失败。
    if key16.len() != 16 {
        return Err(Error::Crypto(format!(
            "fingerprint_edt 需要 16 字节密钥，收到 {} 字节",
            key16.len()
        )));
    }
    Ok(base64_encode(&cbc_encrypt(
        key16.as_bytes(),
        FP_IV,
        plaintext.as_bytes(),
    )?))
}

// KRC 歌词解密实现

/// KRC 歌词解密（跳过 4 字节头，循环异或解密并 zlib 解压为 UTF-8 文本）。
pub fn krc_decode(data: &[u8]) -> Result<String> {
    // 限制最大解压体积，防止解压炸弹导致内存耗尽。
    const MAX_KRC_BYTES: u64 = 8 * 1024 * 1024;
    use std::io::Read as _;

    if data.len() < 4 {
        return Err(Error::Crypto("krc payload shorter than 4 bytes".to_owned()));
    }
    let mut buf = data[4..].to_vec();
    for (i, byte) in buf.iter_mut().enumerate() {
        *byte ^= KRC_KEY[i % KRC_KEY.len()];
    }
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&buf[..])
        .take(MAX_KRC_BYTES)
        .read_to_end(&mut out)
        .map_err(|e| Error::Inflate(e.to_string()))?;
    if out.len() as u64 >= MAX_KRC_BYTES {
        return Err(Error::Inflate(format!("krc 解压结果超过 {MAX_KRC_BYTES} 字节上限")));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// KRC 歌词解密（输入 Base64 编码字符串）。
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
        // MD5 的 32 位十六进制字符直接作为 32 字节 AES-256 密钥
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
    fn krc_decode_skips_header_and_inflates() {
        // 构造测试 KRC 载荷：明文压缩后异或并拼接 4 字节魔数头
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
        // 8 MiB 上限：一份精心构造的歌词不得把进程撑爆。
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
        // 末字节说填充 5 字节，却只有最后一字节是 0x05 → 必须报错，而不是「截断后成功」。
        assert!(unpad_pkcs7(b"0123456789abc\x05".to_vec()).is_err());
        assert_eq!(unpad_pkcs7(b"abc\x05\x05\x05\x05\x05".to_vec()).unwrap(), b"abc");
        // 非密文（16 字节对齐的 JSON）不得被当成解开的明文。
        assert!(unpad_pkcs7(b"{\"status\":1}\x00\x00\x00".to_vec()).is_err());
    }

    #[test]
    fn fingerprint_edt_requires_a_16_byte_key() {
        assert!(fingerprint_edt("x", "0123456789abcdef").is_ok());
        assert!(
            fingerprint_edt("x", &"a".repeat(32)).is_err(),
            "32 字节不得静默升级成 AES-256"
        );
    }

    #[test]
    fn random_aes_secret_is_six_b62_chars() {
        let s = random_aes_secret();
        assert_eq!(s.len(), 6);
        assert!(s.bytes().all(|b| b.is_ascii_alphanumeric()));
    }
}
