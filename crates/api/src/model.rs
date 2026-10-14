//! 领域模型：负责上游多变字段的宽容映射与统一归一化。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codec;

/// 归一化歌曲。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Song {
    /// 歌名。
    pub name: String,
    /// 歌手（多个用 `、` 连接）。
    pub singer: String,
    /// 音频 hash（小写）。
    pub hash: String,
    /// 专辑 ID。
    pub album_id: String,
    /// 专辑音频 ID（`album_audio_id`）。
    pub album_audio_id: String,
    /// 专辑名。
    pub album_name: String,
    /// 时长（毫秒）。
    pub duration_ms: u64,
    /// 歌单内部 fileid，删歌用。
    pub fileid: i64,
    /// `mixsongid`。
    pub mixsongid: i64,
    /// 权益标记（0 = 免费）。
    pub privilege: i64,
    /// 封面 URL（可能含 `{size}` 占位符）。
    pub cover: String,
    /// 音质标识，若上游提供。
    pub quality: String,
}

impl Song {
    /// 从任意上游歌曲对象归一化。
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        // 兼容搜索结果与详情页等不同场景的 Hash 字段别名。
        let hash = codec::pick_str_paths(
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
        let name = codec::pick_str(value, &["name", "songname", "song_name", "filename", "audio_name"]);
        if hash.is_empty() && name.is_empty() {
            return None;
        }
        let duration = codec::pick_i64(value, &["timelen", "duration", "timelen_ms", "duration_ms"]);
        let album_id = codec::pick_str(value, &["album_id", "albumid"])
            .or_else_nonempty(|| codec::pick_str(value, &["trans_param", "album_id"]));
        let album_audio_id = codec::pick_str(
            value,
            &["album_audio_id", "audio_id", "emixsongid", "mixsongid", "audioid"],
        );
        let cover = codec::pick_str_paths(
            value,
            &[
                &["trans_param", "union_cover"],
                &["trans_param", "cover"],
                &["cover"],
                &["img"],
                &["image"],
                &["album_image"],
                &["trans_param", "cdn_pic"],
            ],
        );
        Some(Self {
            name,
            singer: codec::pick_str(
                value,
                &[
                    "singername",
                    "singer_name",
                    "singer",
                    "author_name",
                    "author",
                    "all_audios_singername",
                ],
            ),
            hash,
            album_id,
            album_audio_id,
            album_name: codec::pick_str(value, &["albumname", "album_name", "album"]),
            duration_ms: u64::try_from(duration.max(0)).unwrap_or(0),
            fileid: codec::pick_i64(value, &["fileid", "file_id"]),
            mixsongid: codec::pick_i64(value, &["mixsongid", "mix_song_id"]),
            privilege: codec::pick_i64(value, &["privilege", "pay_type"]),
            cover,
            quality: codec::pick_str(value, &["quality", "audio_quality"]),
        })
    }

    /// 一批歌曲。
    #[must_use]
    pub fn list_from(values: &[Value]) -> Vec<Self> {
        values.iter().filter_map(Self::from_value).collect()
    }

    /// 按目标分辨率替换模板中的尺寸占位符获取封面。
    #[must_use]
    pub fn cover_at(&self, size: u32) -> String {
        if self.cover.is_empty() {
            return String::new();
        }
        self.cover.replace("{size}", &size.to_string())
    }
}

/// 字符串非空回退辅助特征。
trait OrElseNonEmpty {
    fn or_else_nonempty(self, fallback: impl FnOnce() -> String) -> String;
}
impl OrElseNonEmpty for String {
    fn or_else_nonempty(self, fallback: impl FnOnce() -> String) -> String {
        if self.is_empty() { fallback() } else { self }
    }
}

/// 云歌单条目。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    /// `listid`。「我喜欢」固定为 `2`。
    pub listid: String,
    /// 名称。
    pub name: String,
    /// 歌曲数。
    pub count: u64,
    /// `0` = 自建 / 我喜欢，`1` = 收藏。
    pub kind: i64,
    /// 跨设备稳定 ID，取歌单详情用它。
    pub global_collection_id: String,
    /// 封面。
    pub cover: String,
}

impl Playlist {
    /// 「我喜欢」歌单固定标识。
    pub const LIKED_LISTID: &'static str = "2";

    /// 归一化。
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

    /// 是否「我喜欢」。
    #[must_use]
    pub fn is_liked(&self) -> bool {
        self.listid == Self::LIKED_LISTID
    }
}

/// 电台频道（`/v1/rcmd_list`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    /// `fmid`。
    pub fmid: String,
    /// 名称。
    pub name: String,
    /// 所属分类。
    pub classid: String,
    /// 封面。
    pub cover: String,
    /// 横幅。
    pub banner: String,
    /// 热度。
    pub heat: u64,
    /// 描述。
    pub description: String,
}

impl Station {
    /// 归一化。
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

/// 电台分类（`/v1/class_fm_song`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StationClass {
    /// `classid`。
    pub classid: String,
    /// 分类名。
    pub name: String,
    /// 该分类下的频道数。
    pub count: u64,
    /// 分类内的频道（上游会给一小撮）。
    pub stations: Vec<Station>,
}

impl StationClass {
    /// 归一化。
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

/// 播放地址。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongUrl {
    /// 音频基地址（上游还会给 `url[]` 的备用节点）。
    pub url: String,
    /// 备用地址。
    pub backup_urls: Vec<String>,
    /// 扩展名，如 `flac` / `ogg` / `mp3`。
    pub ext: String,
    /// 实际音质。
    pub quality: String,
    /// 文件大小（字节）。
    pub size: u64,
    /// 时长（毫秒）。
    pub duration_ms: u64,
    /// 音频 hash（可能与请求的 hash 不同，例如 ogg 变体）。
    pub hash: String,
}

impl SongUrl {
    /// 解析上游播放地址对象，提取主地址与备选节点列表。
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        // 兼容对象数组与纯字符串数组两种上游播放地址返回结构。
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
            // `backupUrl` 与 `url` 的尾节点重叠，只留一份。
            if !urls.contains(&url) {
                urls.push(url);
            }
            if node_meta.is_none() {
                node_meta = meta;
            }
        }
        let url = urls.first()?.clone();
        let (meta, duration_ms) = node_meta.map_or_else(
            // 无节点元数据时回退至顶层提取时长（秒转毫秒）。
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
        })
    }

    /// 扩展名兜底：从 URL 猜。
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

/// 单行歌词。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    /// 起始时间（毫秒）。
    pub time_ms: u64,
    /// 文本。
    pub text: String,
}

/// 某个音质的权益判定（`/v2/get_res_privilege/lite`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QualityRight {
    /// 音质标识。
    pub quality: String,
    /// 上游是否提供该音质。
    pub exists: bool,
    /// 上游 level。
    pub level: i64,
    /// 码率。
    pub bitrate: u64,
    /// 扩展名。
    pub extname: String,
    /// 该音质的音频 hash（128/320 是 ogg 变体）。
    pub hash: String,
    /// 文件大小。
    pub size: u64,
    /// 上游 privilege。
    pub privilege: i64,
    /// 上游 status。
    pub status: i64,
    /// 是否需要会员 / 付费。
    pub need_vip: bool,
    /// 是否可免 VIP 获取。
    pub obtainable: bool,
    /// 概念版「免费听」命中。
    pub free_listen: bool,
    /// 需要的会员类型：`concept` 或 `suvip`。
    pub require_vip_type: String,
}

/// 一首歌的音质矩阵。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SongRights {
    /// 音频 hash。
    pub hash: String,
    /// 歌名。
    pub name: String,
    /// 歌手。
    pub singer: String,
    /// 专辑名。
    pub albumname: String,
    /// 是否整首免费。
    pub free_song: bool,
    /// 取播放地址时要带上的 `auth`（上游给的资源级票据，免费歌曲为空）。
    pub auth: String,
    /// 各音质判定。
    pub qualities: Vec<QualityRight>,
    /// 可获取的音质。
    pub free_qualities: Vec<String>,
    /// 需要 VIP 的音质。
    pub vip_qualities: Vec<String>,
    /// 需要超级 VIP 的音质。
    pub super_vip_qualities: Vec<String>,
    /// 上游缺失的音质。
    pub missing_qualities: Vec<String>,
    /// 最佳可获取音质。
    pub best_free_quality: String,
}

impl SongRights {
    /// 取某个音质的判定。
    #[must_use]
    pub fn quality(&self, quality: &str) -> Option<&QualityRight> {
        self.qualities.iter().find(|q| q.quality == quality)
    }
}

/// 会员条目（`/v1/get_union_vip` 的 `busi_vip[]`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VipEntry {
    /// 业务类型。概念版为 `concept`。
    pub busi_type: String,
    /// 产品类型。畅听 VIP 为 `svip`。
    pub product_type: String,
    /// 生效时间（毫秒）。
    pub begin_ms: i64,
    /// 到期时间（毫秒）。
    pub end_ms: i64,
}

impl VipEntry {
    /// 在给定时刻是否有效。
    #[must_use]
    pub fn is_active_at(&self, now_ms: i64) -> bool {
        now_ms >= self.begin_ms && now_ms < self.end_ms
    }
}

/// 概念版 VIP 权益状态，以 busi_vip 列表判决。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VipStatus {
    /// 全部会员条目。
    pub entries: Vec<VipEntry>,
}

impl VipStatus {
    /// 归一化。
    #[must_use]
    pub fn from_value(value: &Value) -> Self {
        let entries = codec::find_array(value, &["busi_vip", "vip_list", "list"])
            .map(|items| {
                items
                    .iter()
                    .map(|item| VipEntry {
                        busi_type: codec::pick_str(item, &["busi_type", "busitype"]),
                        product_type: codec::pick_str(item, &["product_type", "producttype"]),
                        begin_ms: codec::pick_i64(item, &["vip_begin_time", "begin_time", "begin"]),
                        end_ms: codec::pick_i64(item, &["vip_end_time", "end_time", "end"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { entries }
    }

    /// 概念版畅听 VIP 条目。
    #[must_use]
    pub fn concept(&self) -> Option<&VipEntry> {
        self.entries
            .iter()
            .find(|e| e.busi_type.eq_ignore_ascii_case("concept") && e.product_type.eq_ignore_ascii_case("svip"))
    }

    /// 概念版 VIP 此刻是否有效。
    #[must_use]
    pub fn is_concept_active(&self) -> bool {
        let now = crate::spec::now_millis();
        self.concept().is_some_and(|e| e.is_active_at(now))
    }

    /// 到期时间（毫秒）。
    #[must_use]
    pub fn concept_expires_at(&self) -> Option<i64> {
        self.concept().map(|e| e.end_ms)
    }
}

/// 分页数据封装，统一推荐流与歌单分页游标。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    /// 本页条目。
    pub items: Vec<T>,
    /// 下一页游标；无游标的流（每日推荐）为 `None`。
    pub cursor: Option<String>,
    /// 流的差异元数据（UI 用同一套列表组件渲染）。
    pub meta: Value,
}

impl<T> Page<T> {
    /// 造一页无游标的结果。
    #[must_use]
    pub fn single(items: Vec<T>) -> Self {
        Self {
            items,
            cursor: None,
            meta: Value::Null,
        }
    }

    /// 是否还有下一页。
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
        assert_eq!(song.album_audio_id, "98765");
        assert_eq!(song.cover_at(240), "http://imge.kugou.com/stdmusic/240/x.jpg");
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
        // 概念版通过 busi_vip 判断权益有效性。
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
}
