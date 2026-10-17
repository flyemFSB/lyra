//! 存储层数据结构（账号、配置与播放模式）。

use serde::{Deserialize, Serialize};

/// 默认音量
const DEFAULT_VOLUME: f32 = 0.8;
/// 默认偏好音质
const DEFAULT_QUALITY: &str = "flac";
/// 默认缓存上限（2GB）
const DEFAULT_MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 登录账号信息
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// 用户 ID（兼作密钥环用户名）
    pub userid: i64,
    /// 登录令牌（内存持有，不直接落盘）
    pub token: String,
    /// VIP 会员类型
    pub vip_type: i64,
    /// VIP 令牌（内存持有，不直接落盘）
    pub vip_token: String,
    /// 用户昵称
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
    pub volume: f32,
    /// 偏好音质（如 128、320、flac 等）
    pub quality: String,
    /// 自定义缓存目录（为空使用系统默认路径）
    pub cache_dir: Option<String>,
    /// 缓存上限（字节）
    pub max_cache_bytes: u64,
    /// 播放模式
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
