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
    /// 用户 id，同时用作凭据管理器的 user name。
    pub userid: i64,
    /// 登录令牌；仅短暂存在于内存，落盘前会被清空。
    pub token: String,
    /// 会员类型。
    pub vip_type: i64,
    /// 会员令牌。
    pub vip_token: String,
    /// 昵称。
    pub nickname: String,
}

/// 用户设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 音量（`0.0..=1.0`）。
    pub volume: f32,
    /// 音质标识（如 `flac`）。
    pub quality: String,
    /// 缓存目录；`None` 表示使用默认目录。
    pub cache_dir: Option<String>,
    /// 缓存上限（字节）。
    pub max_cache_bytes: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: DEFAULT_VOLUME,
            quality: DEFAULT_QUALITY.to_owned(),
            cache_dir: None,
            max_cache_bytes: DEFAULT_MAX_CACHE_BYTES,
        }
    }
}

/// 一条播放历史。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 歌曲 hash，也是去重键。
    pub hash: String,
    /// 歌名。
    pub name: String,
    /// 歌手。
    pub singer: String,
    /// 专辑 id。
    pub album_id: String,
    /// `album_audio_id`。
    pub album_audio_id: String,
    /// 播放时间（Unix 毫秒）。
    pub played_at_ms: i64,
}
