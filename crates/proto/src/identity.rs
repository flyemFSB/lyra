//! 设备标识生成与转换（GUID、MID、WebGL、DFID）。

// 大整数：与 cipher.rs 统一使用 rsa 0.9 转出的 num-bigint-dig 类型
use rsa::BigUint;
use serde::{Deserialize, Serialize};

use crate::cipher::{md5_hex, random_string};
use crate::rng::{JsRng, OsRng};

/// 根据 GUID 的 MD5 计算十进制大数 MID
///
/// `md5_hex` 恒定输出 32 位小写十六进制，`BigUint::parse_bytes` 不会失败；
/// 该不变量由 `debug_assert!` 在开发期钉住。
#[must_use]
pub fn mid_from_guid(guid: &str) -> String {
    let digest = md5_hex(guid.as_bytes());
    let parsed = BigUint::parse_bytes(digest.as_bytes(), 16);
    debug_assert!(parsed.is_some(), "md5 十六进制串必须可解析为大整数");
    parsed.map_or_else(|| "0".to_owned(), |v| v.to_str_radix(10))
}

/// 生成 GUID 单段十六进制随机串
fn guid_segment(rng: &mut impl JsRng) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = (65536.0 * (1.0 + rng.next_f64())) as i64;
    format!("{v:x}").chars().skip(1).collect()
}

/// 生成 UUID **形状**的随机标识串
///
/// ⚠️ 不是 RFC 4122 v4：版本号与变体位仍是随机值。该串只作为 MD5 的输入，
/// 落盘的 `guid` 是它的 MD5，原始 UUID 从不发给服务端，因此无需改成标准 v4。
#[must_use]
pub fn random_uuid(rng: &mut impl JsRng) -> String {
    format!(
        "{}{}-{}-{}-{}-{}{}{}",
        guid_segment(rng),
        guid_segment(rng),
        guid_segment(rng),
        guid_segment(rng),
        guid_segment(rng),
        guid_segment(rng),
        guid_segment(rng),
        guid_segment(rng),
    )
}

/// 模拟生成 WebGL 指纹（64 位十进制整数）
#[must_use]
pub fn random_webgl(rng: &mut impl JsRng) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let hi = (rng.next_f64() * 4_294_967_295.0).floor() as u64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lo = (rng.next_f64() * 4_294_967_295.0).floor() as u64;
    ((hi << 32) | lo).to_string()
}

/// 设备标识集合（首次生成后持久化）
///
/// 所有字段都带 `#[serde(default)]`：设备身份**必须**持久化且跨版本稳定，
/// 新增字段时旧记录仍要能读回来，否则会触发风控（方案 §5.3「随机化会导致风控命中」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// GUID 哈希（32 位十六进制，兼作 uuid 与 imei；源串是 UUID 形状的随机标识，非 RFC 4122）
    #[serde(default)]
    pub guid: String,
    /// 十进制 MID 字符串
    #[serde(default)]
    pub mid: String,
    /// MAC 地址（固定为 02:00:00:00:00:00）
    #[serde(default)]
    pub mac: String,
    /// 10 位设备标识码
    #[serde(default)]
    pub dev: String,
    /// WebGL 指纹
    #[serde(default)]
    pub webgl: String,
    /// 设备指纹 DFID（未注册时为 "-"）
    #[serde(default)]
    pub dfid: String,
}

impl Device {
    /// 生成新设备标识（仅首次启动调用）
    #[must_use]
    pub fn generate() -> Self {
        let mut rng = OsRng;
        Self::generate_with(&mut rng)
    }

    /// 生成新设备标识（支持注入随机源）
    #[must_use]
    pub fn generate_with(rng: &mut impl JsRng) -> Self {
        let guid = md5_hex(random_uuid(rng).as_bytes());
        let mid = mid_from_guid(&guid);
        Self {
            guid,
            mid,
            mac: "02:00:00:00:00:00".to_owned(),
            dev: random_string(10),
            webgl: random_webgl(rng),
            dfid: "-".to_owned(),
        }
    }

    /// 获取 DFID（未注册时返回 "-"）
    #[must_use]
    pub fn dfid_or_dash(&self) -> &str {
        if self.dfid.is_empty() { "-" } else { &self.dfid }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::rng::Mulberry32;

    #[test]
    fn mid_matches_reference_vectors() {
        assert_eq!(
            mid_from_guid("e1f2a3b4c5d6e7f8091a2b3c4d5e6f70"),
            "93471393307406524540615984101025407175"
        );
        assert_eq!(
            mid_from_guid("e8456cee7fd2348cfa239342b10a23bd"),
            "150330140572226157714874292511398898608"
        );
    }

    #[test]
    fn guid_hash_is_32_hex_chars() {
        let guid = md5_hex(b"550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(guid, "e8456cee7fd2348cfa239342b10a23bd");
        assert_eq!(guid.len(), 32);
    }

    #[test]
    fn generated_device_is_self_consistent() {
        let mut rng = Mulberry32::new(0x1234_5678);
        let d = Device::generate_with(&mut rng);
        assert_eq!(d.guid.len(), 32);
        assert!(d.guid.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(d.mid, mid_from_guid(&d.guid));
        assert_eq!(d.dev.len(), 10);
        assert_eq!(d.mac, "02:00:00:00:00:00");
        assert_eq!(d.dfid_or_dash(), "-");
    }

    #[test]
    fn random_uuid_has_uuid_shape() {
        let mut rng = Mulberry32::new(7);
        let u = random_uuid(&mut rng);
        let parts: Vec<usize> = u.split('-').map(str::len).collect();
        assert_eq!(parts, vec![8, 4, 4, 4, 12]);
    }

    #[test]
    fn legacy_device_without_new_fields_still_deserializes() {
        // 旧版本落盘的设备记录缺少后加的字段，必须能读回来而不是整条反序列化失败
        // （失败会让上层重新生成身份，方案 §5.3：随机化会导致风控命中）
        let legacy = br#"{"guid":"e8456cee7fd2348cfa239342b10a23bd","mid":"12345678901234567890","mac":"02:00:00:00:00:00","dev":"ABCDEFGHIJ"}"#;
        let device: Device = serde_json::from_slice(legacy).unwrap();
        assert_eq!(device.guid, "e8456cee7fd2348cfa239342b10a23bd");
        assert_eq!(device.mid, "12345678901234567890");
        assert_eq!(device.dev, "ABCDEFGHIJ");
        assert_eq!(device.webgl, "");
        assert_eq!(device.dfid, "");
    }

    #[test]
    fn random_webgl_is_decimal_u64() {
        let mut rng = Mulberry32::new(99);
        let v = random_webgl(&mut rng);
        assert!(v.parse::<u64>().is_ok(), "{v}");
    }
}
