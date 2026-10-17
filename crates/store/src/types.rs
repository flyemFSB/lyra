//! 跨层共享的持久化数据结构。

use serde::{Deserialize, Serialize};

/// 默认音量。
const DEFAULT_VOLUME: f32 = 0.8;
/// 默认音质。
const DEFAULT_QUALITY: &str = "flac";
/// 默认缓存上限：2 GiB。
const DEFAULT_MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 登录账号信息（包含用户 ID、昵称及内存中持有的会话令牌）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// 用户 ID，同时作为凭据管理器的用户名。
    pub userid: i64,
    /// 登录令牌（仅短暂驻留内存，落盘前清空）。
    pub token: String,
    /// 会员类型。
    pub vip_type: i64,
    /// 会员令牌。
    pub vip_token: String,
    /// 昵称。
    pub nickname: String,
}

/// 播放循环模式（需跨启动记住，所以定义在持久化层）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    /// 列表循环（默认）。
    #[default]
    ListLoop,
    /// 单曲循环。
    SingleLoop,
    /// 随机播放。
    Shuffle,
}

/// 用户配置项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 播放音量（0.0..=1.0）。
    pub volume: f32,
    /// 默认偏好音质（如 flac、320 等）。
    pub quality: String,
    /// 缓存目录（None 表示使用默认目录）。
    pub cache_dir: Option<String>,
    /// 缓存容量上限（字节）。
    pub max_cache_bytes: u64,
    /// 播放模式；`default` 保证旧配置（无此字段）也能反序列化。
    #[serde(default)]
    pub play_mode: PlayMode,
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
