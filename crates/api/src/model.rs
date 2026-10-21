//! 领域业务模型：上游多变数据结构的归一化映射。

use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codec;

/// 歌曲领域模型
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Song {
    /// 歌曲名称
    pub name: String,
    /// 歌手列表（顿号分隔）
    pub singer: String,
    /// 音频哈希标识（小写十六进制）
    pub hash: String,
    /// 专辑 ID
    pub album_id: String,
    /// 专辑音频 ID（album_audio_id / mixsongid）
    ///
    /// 用于精确定位音源与歌词，不可与全局 audio_id 混淆。
    pub album_audio_id: String,
    /// 专辑名称
    pub album_name: String,
    /// 音频总时长（毫秒）
    pub duration_ms: u64,
    /// 歌单内部文件 ID（删除歌曲使用）
    pub fileid: i64,
    /// 歌曲综合 ID
    pub mixsongid: i64,
    /// 权益标记（0 表示免费）
    pub privilege: i64,
    /// 封面图片 URL（含尺寸占位符）
    pub cover: String,
    /// 音质标识
    pub quality: String,
}

impl Song {
    /// 从上游 JSON 对象解析并归一化歌曲信息
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        // 提取并归一化音频哈希
        let hash = codec::pick_str_paths_cow(
            value,
            &[
                &["hash"],
                &["filehash"],
                &["audio_hash"],
                &["audio_info", "hash"],
                &["trans_param", "hash"],
            ],
        )
        .to_lowercase();
        // 优先提取纯歌名（songname / song_name），若无则降级提取复合字段（name / filename / audio_name）
        let mut name = codec::pick_str_cow(value, &["songname", "song_name", "name", "filename", "audio_name"]);
        if hash.is_empty() && name.is_empty() {
            return None;
        }
        // 上游时长单位不统一（秒/毫秒），字段命名多变，按量级归一化为毫秒
        let mut duration_raw = codec::pick_i64(
            value,
            &[
                "time_length",
                "timelength",
                "timelen",
                "timelen_ms",
                "Duration",
                "duration",
                "duration_ms",
                "time_len",
                "interval",
                "timelength_320",
                "time_length_320",
                "timelength_128",
                "time_length_128",
            ],
        );
        if duration_raw == 0 {
            duration_raw = codec::pick_i64_paths(
                value,
                &[
                    &["audio_info", "time_length"],
                    &["audio_info", "timelength"],
                    &["audio_info", "timelen"],
                    &["audio_info", "duration"],
                    &["trans_param", "time_length"],
                    &["trans_param", "timelength"],
                    &["trans_param", "duration"],
                ],
            );
        }
        let duration_ms = normalise_duration(duration_raw);
        let album_id = scalar_of(codec::pick(value, &["album_id", "albumid"]));
        // 专辑 ID 缺失时回退到 trans_param 中的同名字段
        let album_id = if album_id.is_empty() {
            scalar_of(codec::pick_path(value, &["trans_param", "album_id"]))
        } else {
            album_id
        };
        // 优先提取 album_audio_id 或 mixsongid，规避 audio_id 命名空间冲突
        let album_audio_id = codec::pick_str_cow(
            value,
            &[
                "album_audio_id",
                "album_audioid",
                "mixsongid",
                "mix_song_id",
                "add_mixsongid",
                "emixsongid",
            ],
        )
        .into_owned();
        let cover = codec::pick_str_paths_cow(
            value,
            &[
                &["trans_param", "union_cover"],
                &["trans_param", "sizable_cover"],
                &["trans_param", "cover"],
                &["trans_param", "cdn_pic"],
                &["album_info", "sizable_cover"],
                &["album_info", "cover"],
                &["album_info", "album_img"],
                &["album_info", "pic"],
                &["album_info", "img"],
                &["album_info", "image"],
                &["album_info", "web_album_img"],
                &["union_cover"],
                &["sizable_cover"],
                &["album_sizable_cover"],
                &["cover"],
                &["img"],
                &["image"],
                &["pic"],
                &["album_image"],
                &["album_img"],
                &["album_pic"],
                &["album_cover"],
                &["web_album_img"],
                &["singer_avatar"],
                &["singer_img"],
                &["author_img"],
                &["avatar"],
                &["imgurl"],
            ],
        )
        .into_owned();
        let mut singer = codec::pick_str_cow(
            value,
            &[
                "singername",
                "singer_name",
                "singer",
                "author_name",
                "author",
                "all_audios_singername",
            ],
        )
        .into_owned();

        // 如果歌曲名称包含“歌手 - 歌名”格式（常见于 Kugou 歌单 filename / audio_name），将其分离为纯歌名与歌手名
        if let Some((artist_part, title_part)) = name.split_once(" - ") {
            let artist_part = artist_part.trim();
            let title_part = title_part.trim();
            if !artist_part.is_empty() && !title_part.is_empty() {
                if singer.is_empty() {
                    artist_part.clone_into(&mut singer);
                }
                name = Cow::Owned(title_part.to_owned());
            }
        }

        Some(Self {
            name: name.into_owned(),
            singer,
            hash,
            album_id,
            album_audio_id,
            album_name: codec::pick_str_cow(value, &["albumname", "album_name", "album"]).into_owned(),
            duration_ms,
            fileid: codec::pick_i64(value, &["fileid", "file_id"]),
            mixsongid: codec::pick_i64(value, &["mixsongid", "mix_song_id"]),
            privilege: codec::pick_i64(value, &["privilege", "pay_type"]),
            cover,
            quality: codec::pick_str_cow(value, &["quality", "audio_quality"]).into_owned(),
        })
    }

    /// 批量解析歌曲列表
    #[must_use]
    pub fn list_from(values: &[Value]) -> Vec<Self> {
        values.iter().filter_map(Self::from_value).collect()
    }

    /// 获取展示用的主标题（纯歌名）
    #[must_use]
    pub fn display_title(&self) -> &str {
        if let Some((_, title)) = self.name.split_once(" - ") {
            let title = title.trim();
            if !title.is_empty() {
                return title;
            }
        }
        let trimmed = self.name.trim();
        if trimmed.is_empty() { "未知歌曲" } else { trimmed }
    }

    /// 获取展示用的副标题（歌手名）
    #[must_use]
    pub fn display_singer(&self) -> &str {
        if !self.singer.trim().is_empty() {
            return self.singer.trim();
        }
        if let Some((artist, _)) = self.name.split_once(" - ") {
            let artist = artist.trim();
            if !artist.is_empty() {
                return artist;
            }
        }
        "未知歌手"
    }

    /// 获取指定分辨率的封面 URL
    #[must_use]
    pub fn cover_at(&self, size: u32) -> String {
        if self.cover.is_empty() {
            return String::new();
        }
        self.cover.replace("{size}", &size.to_string())
    }
}

/// 时长归一化为毫秒
fn normalise_duration(value: i64) -> u64 {
    if value <= 0 {
        0
    } else if value < 1_000 {
        u64::try_from(value).unwrap_or(0) * 1_000
    } else {
        u64::try_from(value).unwrap_or(0)
    }
}

/// 仅提取标量字段文本，对象与数组均视为空
fn scalar_of(value: Option<&Value>) -> String {
    value
        .filter(|v| !v.is_object() && !v.is_array())
        .map(codec::as_str_cow)
        .unwrap_or_default()
        .into_owned()
}

/// 歌单摘要模型
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    /// 歌单 ID（我喜欢固定为 2）
    pub listid: String,
    /// 歌单名称
    pub name: String,
    /// 歌曲总数
    pub count: u64,
    /// 歌单类型（0 自建/喜欢，1 收藏）
    pub kind: i64,
    /// 全局收藏集 ID（获取详情用）
    pub global_collection_id: String,
    /// 封面 URL
    pub cover: String,
}

impl Playlist {
    /// 「我喜欢」歌单固定 ID
    pub const LIKED_LISTID: &'static str = "2";

    /// 从上游 JSON 解析歌单信息
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let listid = codec::pick_str(value, &["listid", "list_id", "id"]);
        if listid.is_empty() {
            return None;
        }
        Some(Self {
            listid,
            name: codec::pick_str(value, &["name", "listname", "list_name"]),
            count: codec::pick_u64(value, &["count", "song_count", "total"]),
            kind: codec::pick_i64(value, &["type", "list_type"]),
            global_collection_id: codec::pick_str(value, &["global_collection_id", "global_collectionid", "global_id"]),
            cover: codec::pick_str_paths(value, &[&["cover"], &["img"], &["pic"]]),
        })
    }

    /// 检查是否为「我喜欢」歌单
    #[must_use]
    pub fn is_liked(&self) -> bool {
        self.listid == Self::LIKED_LISTID
    }
}

/// 电台频道模型
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    /// 频道 ID
    pub fmid: String,
    /// 频道名称
    pub name: String,
    /// 所属分类 ID
    pub classid: String,
    /// 封面 URL
    pub cover: String,
    /// 横幅 URL
    pub banner: String,
    /// 热度播放量
    pub heat: u64,
    /// 频道描述
    pub description: String,
}

impl Station {
    /// 从上游 JSON 解析电台频道
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let fmid = codec::pick_str(value, &["fmid", "fm_id", "id"]);
        if fmid.is_empty() {
            return None;
        }
        Some(Self {
            fmid,
            name: codec::pick_str(value, &["fmname", "name", "fm_name"]),
            classid: codec::pick_str(value, &["classid", "class_id"]),
            cover: codec::pick_str_paths(value, &[&["imgurl"], &["cover"], &["img"]]),
            banner: codec::pick_str(value, &["banner", "bannerurl"]),
            heat: codec::pick_u64(value, &["heat", "hot", "playcount"]),
            description: codec::pick_str(value, &["description", "desc", "intro"]),
        })
    }
}

/// 电台分类模型
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StationClass {
    /// 分类 ID
    pub classid: String,
    /// 分类名称
    pub name: String,
    /// 频道总数
    pub count: u64,
    /// 包含的电台列表
    pub stations: Vec<Station>,
}

impl StationClass {
    /// 从上游 JSON 解析电台分类
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let classid = codec::pick_str(value, &["classid", "class_id", "id"]);
        if classid.is_empty() {
            return None;
        }
        Some(Self {
            classid,
            name: codec::pick_str(value, &["classname", "name", "class_name"]),
            count: codec::pick_u64(value, &["class_count", "count", "total"]),
            stations: codec::find_array(value, &["fmlist", "fm_list", "list"])
                .map(|items| items.iter().filter_map(Station::from_value).collect())
                .unwrap_or_default(),
        })
    }
}

/// 音频播放地址模型
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongUrl {
    /// 音频主地址
    pub url: String,
    /// 备用地址列表
    pub backup_urls: Vec<String>,
    /// 文件扩展名（小写）
    pub ext: String,
    /// 实际音质
    pub quality: String,
    /// 文件大小（字节）
    pub size: u64,
    /// 音频时长（毫秒）
    pub duration_ms: u64,
    /// 音频哈希标识
    pub hash: String,
    /// 试听片段时长（毫秒，0 表示完整曲）
    pub preview_ms: u64,
}

impl SongUrl {
    /// 计算 Blob 缓存键（优先使用音频哈希，试听片段追加 preview 后缀）
    #[must_use]
    pub fn blob_key(&self, requested: &str) -> String {
        let base = if self.hash.is_empty() {
            requested
        } else {
            self.hash.as_str()
        };
        if self.preview_ms == 0 {
            base.to_owned()
        } else {
            format!("{base}_preview")
        }
    }

    /// 从上游响应中解析主播放地址与备用列表
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        // 兼容对象数组与纯字符串数组两种上游播放地址返回结构
        let mut urls: Vec<String> = Vec::new();
        let mut node_meta: Option<&Value> = None;
        for item in codec::find_array(value, &["url", "urls", "play_url"])
            .into_iter()
            .flatten()
            .chain(
                codec::find_array(value, &["backupUrl", "backup_urls"])
                    .into_iter()
                    .flatten(),
            )
        {
            let (url, meta) = match item {
                Value::String(text) => (text.clone(), None),
                other => (codec::pick_str(other, &["url"]), Some(other)),
            };
            if url.is_empty() {
                continue;
            }
            if !urls.contains(&url) {
                urls.push(url);
            }
            if node_meta.is_none() {
                node_meta = meta;
            }
        }
        let url = urls.first()?.clone();
        let (meta, duration_ms) = node_meta.map_or_else(
            // 无节点元数据时回退至顶层提取时长（秒转毫秒）
            || {
                (
                    value,
                    codec::pick_u64(value, &["timelength", "timelen"]).saturating_mul(1_000),
                )
            },
            |meta| (meta, codec::pick_u64(meta, &["timelength", "timelen", "duration"])),
        );
        Some(Self {
            url,
            backup_urls: urls.into_iter().skip(1).collect(),
            ext: codec::pick_str(meta, &["extname", "ext", "filetype"]).to_lowercase(),
            quality: codec::pick_str(meta, &["quality", "quality_name"]),
            size: codec::pick_u64(meta, &["filesize", "size"]),
            duration_ms,
            hash: codec::pick_str(meta, &["hash"]).to_lowercase(),
            preview_ms: 0,
        })
    }

    /// 获取扩展名或从 URL 猜测
    #[must_use]
    pub fn ext_or_guess(&self) -> String {
        if !self.ext.is_empty() {
            return self.ext.clone();
        }
        for candidate in ["flac", "ogg", "mp3", "m4a", "aac", "wav"] {
            if self.url.contains(candidate) {
                return candidate.to_owned();
            }
        }
        "mp3".to_owned()
    }
}

/// 逐行歌词条目
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    /// 起始时间戳（毫秒）
    pub time_ms: u64,
    /// 歌词文本
    pub text: String,
}

/// 音质权益判定结果
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QualityRight {
    /// 音质标识
    pub quality: String,
    /// 上游是否提供该音质
    pub exists: bool,
    /// 上游音质等级
    pub level: i64,
    /// 码率（bps）
    pub bitrate: u64,
    /// 扩展名
    pub extname: String,
    /// 该音质的音频哈希
    pub hash: String,
    /// 文件大小（字节）
    pub size: u64,
    /// 上游权限码
    pub privilege: i64,
    /// 上游状态码
    pub status: i64,
    /// 是否需要 VIP
    pub need_vip: bool,
    /// 无版权或已下架标记
    pub no_copyright: bool,
    /// 是否可直接获取
    pub obtainable: bool,
    /// 免费听活动命中
    pub free_listen: bool,
    /// 要求的 VIP 类型（如 concept 或 suvip）
    pub require_vip_type: String,
}

/// 歌曲完整音质权益矩阵
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SongRights {
    /// 音频哈希标识
    pub hash: String,
    /// 歌曲名称
    pub name: String,
    /// 歌手列表
    pub singer: String,
    /// 专辑名称
    pub albumname: String,
    /// 是否整首歌曲免费
    pub free_song: bool,
    /// 资源级认证票据（auth）
    pub auth: String,
    /// 各音质权益详情
    pub qualities: Vec<QualityRight>,
    /// 可获取的音质列表
    pub free_qualities: Vec<String>,
    /// 需要 VIP 的音质列表
    pub vip_qualities: Vec<String>,
    /// 需要超级 VIP 的音质列表
    pub super_vip_qualities: Vec<String>,
    /// 上游未提供的音质列表
    pub missing_qualities: Vec<String>,
    /// 最佳可用音质
    pub best_free_quality: String,
}

impl SongRights {
    /// 查询指定音质的判定信息
    #[must_use]
    pub fn quality(&self, quality: &str) -> Option<&QualityRight> {
        self.qualities.iter().find(|q| q.quality == quality)
    }

    /// 检查歌曲是否被判为无版权或已下架
    #[must_use]
    pub fn is_no_copyright(&self) -> bool {
        self.qualities.iter().any(|q| q.no_copyright)
    }
}

/// VIP 权益条目
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VipEntry {
    /// 业务类型（如 concept）
    pub busi_type: String,
    /// 产品类型（如 svip / tvip）
    pub product_type: String,
    /// 是否为有效 VIP（对应上游 is_vip 字段）
    pub is_vip: bool,
    /// 生效时间戳（毫秒）
    pub begin_ms: i64,
    /// 到期时间戳（毫秒）
    pub end_ms: i64,
    /// 原始到期时间字符串（如 "2026-10-21 13:41:21"）
    pub end_time_str: String,
}

impl VipEntry {
    /// 检查指定时间戳是否处于会员有效期内
    #[must_use]
    pub fn is_active_at(&self, now_ms: i64) -> bool {
        if self.end_ms > 0 {
            return now_ms < self.end_ms && (self.begin_ms == 0 || now_ms >= self.begin_ms);
        }
        self.is_vip
    }
}

/// 解析时间为 Unix 毫秒时间戳（支持数字时间戳与 "YYYY-MM-DD HH:MM:SS" 格式）
fn parse_time_ms(v: &Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(val) = v.get(*key) {
            if let Some(n) = val.as_i64()
                && n > 0
            {
                return if n > 1_000_000_000_000 { n } else { n * 1000 };
            }
            if let Some(s) = val.as_str() {
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                if let Ok(n) = s.parse::<i64>()
                    && n > 0
                {
                    return if n > 1_000_000_000_000 { n } else { n * 1000 };
                }
                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                    && let Some(cst) = chrono::FixedOffset::east_opt(8 * 3600)
                    && let Some(dt_cst) = dt.and_local_timezone(cst).single()
                {
                    return dt_cst.timestamp_millis();
                }
            }
        }
    }
    0
}

/// VIP 权益状态汇总
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VipStatus {
    /// 全部会员条目列表
    pub entries: Vec<VipEntry>,
}

impl VipStatus {
    /// 从上游响应解析 VIP 状态
    #[must_use]
    pub fn from_value(value: &Value) -> Self {
        let entries = codec::find_array(value, &["busi_vip", "vip_list", "list"])
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let is_vip = codec::pick_i64(item, &["is_vip"]) == 1;
                        let end_time_str = codec::pick_str(item, &["vip_end_time", "end_time", "end"]);
                        VipEntry {
                            busi_type: codec::pick_str(item, &["busi_type", "busitype"]),
                            product_type: codec::pick_str(item, &["product_type", "producttype"]),
                            is_vip,
                            begin_ms: parse_time_ms(item, &["vip_begin_time", "begin_time", "begin"]),
                            end_ms: parse_time_ms(item, &["vip_end_time", "end_time", "end"]),
                            end_time_str,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { entries }
    }

    /// 获取畅听 VIP 条目
    #[must_use]
    pub fn concept(&self) -> Option<&VipEntry> {
        let now = crate::spec::now_millis();
        self.entries
            .iter()
            .find(|e| {
                e.busi_type.eq_ignore_ascii_case("concept")
                    && (e.product_type.eq_ignore_ascii_case("svip") || e.product_type.eq_ignore_ascii_case("tvip"))
                    && e.is_active_at(now)
            })
            .or_else(|| {
                self.entries.iter().find(|e| {
                    e.busi_type.eq_ignore_ascii_case("concept")
                        && (e.product_type.eq_ignore_ascii_case("svip") || e.product_type.eq_ignore_ascii_case("tvip"))
                })
            })
    }

    /// 检查当前畅听 VIP 是否有效
    #[must_use]
    pub fn is_concept_active(&self) -> bool {
        let now = crate::spec::now_millis();
        self.concept().is_some_and(|e| e.is_active_at(now))
    }

    /// 获取畅听 VIP 到期时间戳（毫秒）
    #[must_use]
    pub fn concept_expires_at(&self) -> Option<i64> {
        self.concept().map(|e| e.end_ms)
    }

    /// 获取畅听 VIP 到期时间字符串（如 "2026-10-21 13:41:21"）
    #[must_use]
    pub fn concept_expires_str(&self) -> Option<&str> {
        self.concept()
            .map(|e| e.end_time_str.as_str())
            .filter(|s| !s.is_empty())
    }
}

/// 分页通用数据结构
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    /// 本页数据列表
    pub items: Vec<T>,
    /// 下一页游标（为 None 表示无更多分页）
    pub cursor: Option<String>,
    /// 伴随元数据
    pub meta: Value,
}

impl<T> Page<T> {
    /// 创建无游标的单页数据
    #[must_use]
    pub fn single(items: Vec<T>) -> Self {
        Self {
            items,
            cursor: None,
            meta: Value::Null,
        }
    }

    /// 检查是否存在后续分页
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.cursor.as_ref().is_some_and(|c| !c.is_empty())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_a_pubsongs_style_song() {
        let value = json!({
            "name": "夜曲", "singername": "周杰伦", "hash": "ABC123DEF",
            "album_id": "1234", "audio_id": 98765, "mixsongid": 424242,
            "fileid": 777, "timelen": 226000,
            "trans_param": {"union_cover": "http://imge.kugou.com/stdmusic/{size}/x.jpg"}
        });
        let song = Song::from_value(&value).unwrap();
        assert_eq!(song.name, "夜曲");
        assert_eq!(song.hash, "abc123def", "hash 必须小写");
        assert_eq!(song.duration_ms, 226_000);
        assert_eq!(song.fileid, 777);
        // `audio_id`（98765）不是 album_audio_id：歌单响应里真正的专辑音频 id 是 `mixsongid`。
        assert_eq!(song.album_audio_id, "424242");
        assert_eq!(song.cover_at(240), "http://imge.kugou.com/stdmusic/240/x.jpg");
    }

    #[test]
    fn blob_key_prefers_the_upstream_variant_hash() {
        let mut url = SongUrl {
            hash: "variant".to_owned(),
            ..SongUrl::default()
        };
        assert_eq!(
            url.blob_key("requested"),
            "variant",
            "有变体 hash 时必须用它做内容寻址键"
        );
        url.hash.clear();
        assert_eq!(url.blob_key("requested"), "requested", "缺失时必须回退到请求 hash");
        // 试听片段与完整曲内容不同，不能共用缓存键。
        url.preview_ms = 60_000;
        assert_eq!(url.blob_key("requested"), "requested_preview");
    }

    #[test]
    fn durations_are_normalised_from_seconds_and_milliseconds() {
        // 歌单/详情的 `timelen` 是毫秒。
        assert_eq!(
            Song::from_value(&json!({"hash": "a", "timelen": 225_000}))
                .unwrap()
                .duration_ms,
            225_000
        );
        // 搜索的 `Duration` 是秒：曾经被当成毫秒，四分钟的歌成了 0.2 秒。
        assert_eq!(
            Song::from_value(&json!({"hash": "a", "Duration": 225}))
                .unwrap()
                .duration_ms,
            225_000
        );
        // 个别端点把 timelen 写成秒。
        assert_eq!(
            Song::from_value(&json!({"hash": "a", "timelen": 225}))
                .unwrap()
                .duration_ms,
            225_000
        );
        // 同一个字段名也可能下发毫秒（实测两种都存在）。
        assert_eq!(
            Song::from_value(&json!({"hash": "a", "Duration": 226_000}))
                .unwrap()
                .duration_ms,
            226_000
        );
        assert_eq!(Song::from_value(&json!({"hash": "a"})).unwrap().duration_ms, 0);
        assert_eq!(
            Song::from_value(&json!({"hash": "a", "timelen": 0}))
                .unwrap()
                .duration_ms,
            0
        );
    }

    #[test]
    fn album_id_never_becomes_a_serialised_object() {
        // 上游个别响应把整个 trans_param 当成 album_id 下发：必须视为缺失并回退到真路径。
        let value = json!({"hash": "a", "album_id": {"language": "国语"}, "trans_param": {"album_id": 42}});
        assert_eq!(Song::from_value(&value).unwrap().album_id, "42");
        let nested = json!({"hash": "a", "trans_param": {"album_id": "77"}});
        assert_eq!(Song::from_value(&nested).unwrap().album_id, "77");
    }

    #[test]
    fn song_rights_surface_the_no_copyright_flag() {
        let rights = SongRights {
            qualities: vec![QualityRight {
                quality: "128".to_owned(),
                exists: true,
                no_copyright: true,
                ..QualityRight::default()
            }],
            ..SongRights::default()
        };
        assert!(rights.is_no_copyright());
        assert!(!SongRights::default().is_no_copyright());
    }

    #[test]
    fn album_audio_id_never_comes_from_audio_id() {
        // 歌单响应只给 audio_id/mixsongid：必须挑 mixsongid（实测它与上游 album_audio_id 相等）。
        let from_playlist = json!({"songname": "x", "hash": "AB", "audio_id": 301_293_698, "mixsongid": 39_612_569});
        assert_eq!(Song::from_value(&from_playlist).unwrap().album_audio_id, "39612569");

        // 只有 audio_id 时宁可为空：上游会按 hash 解析到正确的音频，而 audio_id 会解析到别的音频。
        let only_audio_id = json!({"songname": "x", "hash": "AB", "audio_id": 301_293_698});
        assert_eq!(Song::from_value(&only_audio_id).unwrap().album_audio_id, "");

        // 显式 album_audio_id 优先级最高。
        let explicit = json!({"songname": "x", "hash": "AB", "album_audio_id": 777, "mixsongid": 1});
        assert_eq!(Song::from_value(&explicit).unwrap().album_audio_id, "777");
    }

    #[test]
    fn normalizes_alternative_field_names() {
        let value = json!({"songname": "x", "author_name": "y", "audio_info": {"hash": "AB"}});
        let song = Song::from_value(&value).unwrap();
        assert_eq!(song.name, "x");
        assert_eq!(song.singer, "y");
        assert_eq!(song.hash, "ab");
        assert_eq!(song.duration_ms, 0);
    }

    #[test]
    fn separates_compound_song_name_into_title_and_singer() {
        let value = json!({"name": "周杰伦 - 晴天", "hash": "AB"});
        let song = Song::from_value(&value).unwrap();
        assert_eq!(song.name, "晴天");
        assert_eq!(song.singer, "周杰伦");
        assert_eq!(song.display_title(), "晴天");
        assert_eq!(song.display_singer(), "周杰伦");

        let with_singer = json!({"name": "周杰伦 - 晴天", "singername": "周杰伦", "hash": "AB"});
        let song2 = Song::from_value(&with_singer).unwrap();
        assert_eq!(song2.name, "晴天");
        assert_eq!(song2.singer, "周杰伦");

        let legacy = Song {
            name: "周杰伦 - 晴天".to_owned(),
            singer: String::new(),
            ..Song::default()
        };
        assert_eq!(legacy.display_title(), "晴天");
        assert_eq!(legacy.display_singer(), "周杰伦");

        let unknown = Song {
            name: "七里香".to_owned(),
            singer: String::new(),
            ..Song::default()
        };
        assert_eq!(unknown.display_title(), "七里香");
        assert_eq!(unknown.display_singer(), "未知歌手");
    }

    #[test]
    fn rejects_empty_objects() {
        assert!(Song::from_value(&json!({})).is_none());
        assert!(Playlist::from_value(&json!({"name": "no id"})).is_none());
        assert!(Station::from_value(&json!({"name": "no fmid"})).is_none());
    }

    #[test]
    fn liked_playlist_is_listid_two() {
        let p = Playlist::from_value(&json!({"listid": 2, "name": "我喜欢"})).unwrap();
        assert!(p.is_liked());
        assert_eq!(p.listid, "2");
    }

    #[test]
    fn song_url_prefers_the_first_url_with_a_value() {
        let value = json!({"data": {"url": [
            {"url": "", "extname": "mp3"},
            {"url": "http://fs.youthandroid2.kugou.com/x.flac", "extname": "flac", "filesize": 40_000_000, "timelength": 226000, "hash": "AB"}
        ]}});
        let url = SongUrl::from_value(&value).unwrap();
        assert_eq!(url.ext, "flac");
        assert_eq!(url.ext_or_guess(), "flac");
        assert_eq!(url.size, 40_000_000);
        assert_eq!(url.hash, "ab");
        assert!(url.backup_urls.is_empty());
    }

    #[test]
    fn song_url_reads_the_string_array_form_used_by_tracker_v5() {
        // 验证 tracker/v5 接口返回纯字符串数组形式的播放地址解析。
        let value = json!({
            "status": 1,
            "url": ["http://fs.youthandroid2.kugou.com/a.ogg", "http://fs.youthandroid.kugou.com/a.ogg"],
            "backupUrl": ["http://fs.youthandroid.kugou.com/a.ogg"],
            "extName": "ogg",
            "fileSize": 1_757_029,
            "timeLength": 146,
            "hash": "AB5791AA979A51AA86F6E7BAC7D088F6"
        });
        let url = SongUrl::from_value(&value).unwrap();
        assert_eq!(url.url, "http://fs.youthandroid2.kugou.com/a.ogg");
        assert_eq!(url.backup_urls, vec!["http://fs.youthandroid.kugou.com/a.ogg"]);
        assert_eq!(url.ext, "ogg");
        assert_eq!(url.size, 1_757_029);
        assert_eq!(url.duration_ms, 146_000, "顶层 timeLength 是秒");
        assert_eq!(url.hash, "ab5791aa979a51aa86f6e7bac7d088f6");
    }

    #[test]
    fn song_url_backup_nodes_are_collected() {
        let value = json!({"url": [
            {"url": "http://a/1.flac", "extname": "flac"},
            {"url": "http://b/1.flac"},
            {"url": "http://c/1.flac"}
        ]});
        let url = SongUrl::from_value(&value).unwrap();
        assert_eq!(url.url, "http://a/1.flac");
        assert_eq!(url.backup_urls, vec!["http://b/1.flac", "http://c/1.flac"]);
    }

    #[test]
    fn song_url_guesses_extension_from_the_path() {
        let url = SongUrl {
            url: "http://x/y/ap3116_us1_quflac_pa967177915.flac".to_owned(),
            ..SongUrl::default()
        };
        assert_eq!(url.ext_or_guess(), "flac");
        assert_eq!(SongUrl::default().ext_or_guess(), "mp3");
    }

    #[test]
    fn vip_status_uses_busi_vip_not_top_level_flags() {
        // 通过 busi_vip 判断权益有效性。
        let value = json!({
            "status": 1, "vip_type": 0, "is_vip": 0,
            "data": {"busi_vip": [
                {"busi_type": "concept", "product_type": "svip", "vip_begin_time": 1000, "vip_end_time": 4102444800000_i64},
                {"busi_type": "tvip", "product_type": "svip", "vip_begin_time": 0, "vip_end_time": 0}
            ]}
        });
        let vip = VipStatus::from_value(&value);
        assert_eq!(vip.entries.len(), 2);
        assert!(vip.is_concept_active(), "必须看 busi_vip 而不是顶层 vip_type");
        assert_eq!(vip.concept_expires_at(), Some(4_102_444_800_000));
    }

    #[test]
    fn vip_status_is_inactive_when_expired() {
        let value = json!({"data": {"busi_vip": [
            {"busi_type": "concept", "product_type": "svip", "vip_begin_time": 0, "vip_end_time": 1}
        ]}});
        assert!(!VipStatus::from_value(&value).is_concept_active());
    }

    #[test]
    fn page_cursor_semantics() {
        let page: Page<Song> = Page::single(vec![]);
        assert!(!page.has_more());
        let paged = Page {
            items: vec![],
            cursor: Some("s=5".to_owned()),
            meta: Value::Null,
        };
        assert!(paged.has_more());
        assert!(
            !Page::<Song> {
                cursor: Some(String::new()),
                ..paged
            }
            .has_more()
        );
    }

    #[test]
    fn test_song_duration_from_various_fields() {
        // 验证每日推荐等接口使用的 time_length 与 timelength_320 能够正确解析为毫秒
        let json_time_length = json!({
            "songname": "默契",
            "hash": "EA5F68BBA10C12AEBE810360A515CD82",
            "author_name": "海南小崇",
            "time_length": 208
        });
        let song = Song::from_value(&json_time_length).unwrap();
        assert_eq!(song.duration_ms, 208_000);

        let json_timelength_fallback = json!({
            "songname": "晴天",
            "hash": "DF1CBA1FFC7EE33989069D80CBE5C064",
            "timelength_320": 269
        });
        let song2 = Song::from_value(&json_timelength_fallback).unwrap();
        assert_eq!(song2.duration_ms, 269_000);

        let json_nested = json!({
            "songname": "七里香",
            "hash": "EB63DFC82FA0FC03EE58A5DA1F2DFF78",
            "audio_info": {
                "time_length": 299
            }
        });
        let song3 = Song::from_value(&json_nested).unwrap();
        assert_eq!(song3.duration_ms, 299_000);
    }
}
