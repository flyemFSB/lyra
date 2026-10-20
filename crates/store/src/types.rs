//! 存储层数据结构（账号、配置与播放模式）。

use serde::{Deserialize, Serialize};

/// 默认音量
const DEFAULT_VOLUME: f32 = 0.8;
/// 默认偏好音质
const DEFAULT_QUALITY: &str = "flac";
/// 默认缓存上限（2GB）
const DEFAULT_MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 登录账号信息
///
/// 所有字段都带 `#[serde(default)]`：旧版本落盘的记录缺少新字段时必须能读回来，
/// 否则一次升级就会让全部账号记录反序列化失败。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// 用户 ID（兼作密钥环用户名）
    #[serde(default)]
    pub userid: i64,
    /// 登录令牌（内存持有，不直接落盘）
    #[serde(default)]
    pub token: String,
    /// VIP 会员类型
    #[serde(default)]
    pub vip_type: i64,
    /// VIP 令牌（内存持有，不直接落盘）
    #[serde(default)]
    pub vip_token: String,
    /// 用户昵称
    #[serde(default)]
    pub nickname: String,
}

/// 播放循环模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    /// 列表循环
    #[default]
    ListLoop,
    /// 单曲循环
    SingleLoop,
    /// 随机播放
    Shuffle,
}

/// 用户配置项
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 播放音量（0.0..=1.0）
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// 偏好音质
    ///
    /// 取值是上游音质标识（`128` / `320` / `flac` / `viper_tape` 等）。
    /// 规范枚举由接口层（`lyra-api`）拥有，本层不反向依赖它，只做字符串存取。
    #[serde(default = "default_quality")]
    pub quality: String,
    /// 自定义缓存目录（为空使用系统默认路径）
    #[serde(default)]
    pub cache_dir: Option<String>,
    /// 缓存上限（字节）
    #[serde(default = "default_max_cache_bytes")]
    pub max_cache_bytes: u64,
    /// 播放模式
    #[serde(default)]
    pub play_mode: PlayMode,
}

/// 默认音量（供 serde 使用）
fn default_volume() -> f32 {
    DEFAULT_VOLUME
}

/// 默认偏好音质（供 serde 使用）
fn default_quality() -> String {
    DEFAULT_QUALITY.to_owned()
}

/// 默认缓存上限（供 serde 使用）
fn default_max_cache_bytes() -> u64 {
    DEFAULT_MAX_CACHE_BYTES
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: DEFAULT_VOLUME,
            quality: DEFAULT_QUALITY.to_owned(),
            cache_dir: None,
            max_cache_bytes: DEFAULT_MAX_CACHE_BYTES,
            play_mode: PlayMode::default(),
        }
    }
}

impl Settings {
    /// 归一化配置：把音量收敛到 `0.0..=1.0`。
    ///
    /// 音量超出范围时按边界收敛（而不是让下游拿到越界值导致削波或静音）；
    /// 非有限值（NaN / ±inf）无法收敛，由调用方拒绝。
    #[must_use]
    pub fn normalized(&self) -> Self {
        let mut normalized = self.clone();
        normalized.volume = self.volume.clamp(0.0, 1.0);
        normalized
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

    use super::*;

    #[test]
    fn volume_is_clamped_into_range() {
        let loud = Settings {
            volume: 3.5,
            ..Settings::default()
        };
        assert_eq!(loud.normalized().volume, 1.0);

        let quiet = Settings {
            volume: -2.0,
            ..Settings::default()
        };
        assert_eq!(quiet.normalized().volume, 0.0);

        let fine = Settings {
            volume: 0.42,
            ..Settings::default()
        };
        assert_eq!(fine.normalized().volume, 0.42);
    }

    #[test]
    fn legacy_settings_without_new_fields_still_deserialize() {
        // 旧版本落盘的配置缺少 play_mode / cache_dir，必须回落到默认值
        let legacy = br#"{"volume":0.5,"quality":"128","max_cache_bytes":10}"#;
        let settings: Settings = serde_json::from_slice(legacy).unwrap();
        assert_eq!(settings.volume, 0.5);
        assert_eq!(settings.quality, "128");
        assert_eq!(settings.cache_dir, None);
        assert_eq!(settings.play_mode, PlayMode::ListLoop);
    }

    #[test]
    fn legacy_account_without_new_fields_still_deserializes() {
        // 旧版本落盘的账号记录缺少 nickname，必须能读回来
        let legacy = br#"{"userid":42,"token":"t","vip_type":1}"#;
        let account: Account = serde_json::from_slice(legacy).unwrap();
        assert_eq!(account.userid, 42);
        assert_eq!(account.token, "t");
        assert_eq!(account.vip_type, 1);
        assert_eq!(account.nickname, "");
    }
}
