//! 设备身份标识生成与转换（guid、mid、dev、webgl、dfid）。

use num_bigint::BigUint;
use serde::{Deserialize, Serialize};

use crate::cipher::{md5_hex, random_string};
use crate::rng::{JsRng, OsRng};

/// `calculateMid`：把 `md5(input)` 的 hex 当大整数转十进制字符串。
#[must_use]
pub fn mid_from_guid(guid: &str) -> String {
    let digest = md5_hex(guid.as_bytes());
    BigUint::parse_bytes(digest.as_bytes(), 16).map_or_else(|| "0".to_owned(), |v| v.to_str_radix(10))
}

/// JS `getGuid()` 的单段：取 `(65536*(1+rand))|0` 的 hex 末 4 位。
fn guid_segment(rng: &mut impl JsRng) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = (65536.0 * (1.0 + rng.next_f64())) as i64;
    format!("{v:x}").chars().skip(1).collect()
}

/// JS `getGuid()`：`8-4-4-4-12` 形式的随机串（不强制 UUID v4 的版本位）。
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

/// JS `generateWebGLHash()` 的 Node 分支：两个 32 位随机数拼成 uint64 的十进制串。
#[must_use]
pub fn random_webgl(rng: &mut impl JsRng) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let hi = (rng.next_f64() * 4_294_967_295.0).floor() as u64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lo = (rng.next_f64() * 4_294_967_295.0).floor() as u64;
    ((hi << 32) | lo).to_string()
}

/// 设备身份，首次生成后持久化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// `md5(getGuid())`，32 位小写 hex。注册接口的 `uuid` / `imei` 都用它。
    pub guid: String,
    /// `calculateMid(guid)` 的十进制串。
    pub mid: String,
    /// 固定为 `02:00:00:00:00:00`。
    pub mac: String,
    /// 10 位大写字母数字。
    pub dev: String,
    /// WebGL 指纹（十进制 uint64 串）。
    pub webgl: String,
    /// `register_dev` 返回；未注册时为 `-`。
    pub dfid: String,
}

impl Device {
    /// 生成一份新的设备身份（仅首启调用一次）。
    #[must_use]
    pub fn generate() -> Self {
        let mut rng = OsRng;
        Self::generate_with(&mut rng)
    }

    /// 生成一份新的设备身份，随机源可注入以便测试。
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

    /// `dfid`（未注册时用 `-`）。
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
    fn random_webgl_is_decimal_u64() {
        let mut rng = Mulberry32::new(99);
        let v = random_webgl(&mut rng);
        assert!(v.parse::<u64>().is_ok(), "{v}");
    }
}
