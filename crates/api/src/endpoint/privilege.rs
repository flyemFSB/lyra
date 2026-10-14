//! 音质权益矩阵与会员领奖端点，处理概念版特权、音质可用性及每日领奖。

use lyra_proto::consts::{APPID, CLIENTVER};
use serde_json::Value;

use crate::cache::Freshness;
use crate::client::Api;
use crate::codec;
use crate::error::{Code, Result};
use crate::model::{QualityRight, Song, SongRights, VipStatus};
use crate::spec::RequestSpec;

/// 音质排序优先级，从低到高。
pub const QUALITY_ORDER: &[&str] = &[
    "128",
    "320",
    "flac",
    "high",
    "super",
    "viper_clear",
    "viper_tape",
    "viper_atmos",
    "multitrack",
];

/// 上游全量音质查询列表。
const UPSTREAM_QUALITIES: &[&str] = &[
    "128",
    "320",
    "flac",
    "high",
    "viper_atmos",
    "viper_tape",
    "viper_clear",
    "super",
    "multitrack",
];

/// 客户端默认请求的音质列表。
const DEFAULT_QUALITIES: &str = "128,320,flac,high,viper_tape,viper_clear,viper_atmos";

/// 一个待查询的歌曲资源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RightRequest {
    /// 音频 hash。
    pub hash: String,
    /// 专辑 ID。
    pub album_id: i64,
}

impl RightRequest {
    /// 从歌曲构造。
    #[must_use]
    pub fn from_song(song: &Song) -> Self {
        Self {
            hash: song.hash.clone(),
            album_id: song.album_id.parse().unwrap_or(0),
        }
    }
}

/// 音质级别的强类型枚举表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quality {
    /// `128`：概念版免费听覆盖的就是它。
    Standard,
    /// `320`。
    High,
    /// `flac`。
    Flac,
    /// `high`（Hi-Res）。
    HiRes,
    /// `super`。
    Super,
    /// `viper_tape`（蝰蛇母带）。
    ViperTape,
    /// `viper_clear`（蝰蛇超清）。
    ViperClear,
    /// `viper_atmos`（蝰蛇全景声）。
    ViperAtmos,
    /// `multitrack`。
    Multitrack,
    /// 上游新出现的、本版本还不认识的音质。
    Unknown,
}

impl Quality {
    /// 解析上游音质字符串。
    fn parse(text: &str) -> Self {
        match text {
            "128" => Self::Standard,
            "320" => Self::High,
            "flac" => Self::Flac,
            "high" => Self::HiRes,
            "super" => Self::Super,
            "viper_tape" => Self::ViperTape,
            "viper_clear" => Self::ViperClear,
            "viper_atmos" => Self::ViperAtmos,
            "multitrack" => Self::Multitrack,
            _ => Self::Unknown,
        }
    }

    /// 在 relate_goods 中优先匹配的等级标识候选。
    fn level_candidates(self) -> &'static [i64] {
        match self {
            Self::Standard => &[1, 2],
            Self::High => &[4],
            Self::Flac => &[5],
            Self::HiRes => &[6],
            Self::Super => &[7],
            // 蝰蛇系与多轨在上游没有固定 level，按 quality 名匹配
            Self::ViperTape | Self::ViperClear | Self::ViperAtmos | Self::Multitrack | Self::Unknown => &[],
        }
    }

    /// 获取当前音质所需 VIP 会员级别。
    fn required_vip(self) -> &'static str {
        match self {
            Self::ViperTape | Self::ViperClear | Self::ViperAtmos => "suvip",
            Self::Standard | Self::High | Self::Flac | Self::HiRes | Self::Super | Self::Multitrack | Self::Unknown => {
                "concept"
            }
        }
    }

    /// 获取 OGG 变体在 trans_param 中的前缀标识。
    fn ogg_key_prefix(self) -> Option<&'static str> {
        match self {
            Self::Standard => Some("ogg_128_"),
            Self::High => Some("ogg_320_"),
            Self::Flac
            | Self::HiRes
            | Self::Super
            | Self::ViperTape
            | Self::ViperClear
            | Self::ViperAtmos
            | Self::Multitrack
            | Self::Unknown => None,
        }
    }
}

/// 批量查询歌曲音质权益矩阵。
pub async fn privilege(api: &Api, items: &[RightRequest], qualities: Option<&str>) -> Result<Vec<SongRights>> {
    if items.is_empty() {
        return Err(crate::error::Error::Decode("缺少歌曲 hash".to_owned()));
    }

    // 不携带 album_audio_id 保证上游下发 128 标准音质的 OGG 节点。
    let resource: Vec<Value> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "type": "audio",
                "page_id": 0,
                "hash": item.hash.to_lowercase(),
                "album_id": item.album_id,
            })
        })
        .collect();

    let wanted: Vec<String> = match qualities {
        // all 表示查询全部音质。
        Some("all") => UPSTREAM_QUALITIES.iter().map(|s| (*s).to_owned()).collect(),
        Some(list) if !list.trim().is_empty() => list
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => DEFAULT_QUALITIES.split(',').map(str::to_owned).collect(),
    };

    let body = serde_json::json!({
        "appid": APPID,
        "area_code": 1,
        "behavior": "play",
        "clientver": CLIENTVER,
        "need_hash_offset": 1,
        "relate": 1,
        "support_verify": 1,
        "resource": resource,
        "qualities": UPSTREAM_QUALITIES,
    });
    let spec = RequestSpec::post("/v2/get_res_privilege/lite")
        .body(body)
        .header("x-router", "media.store.kugou.com")
        .header("Content-Type", "application/json");

    let value = api.call(spec, Freshness::PLAYLIST).await?;
    let data = value.get("data").and_then(Value::as_array).cloned().unwrap_or_default();

    Ok(items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let hash = item.hash.to_lowercase();
            // 依据 Hash 进行结果关联匹配。
            let entry = data
                .iter()
                .find(|candidate| codec::pick_str(candidate, &["hash"]).to_lowercase() == hash)
                .or_else(|| data.get(index))
                .cloned()
                .unwrap_or(Value::Null);
            build_quality_vip(&hash, item.album_id, &entry, &wanted)
        })
        .collect())
}

/// 查询单首歌曲的音质权益判定。
pub async fn privilege_for_song(api: &Api, song: &Song, qualities: Option<&str>) -> Result<SongRights> {
    let mut rights = privilege(api, &[RightRequest::from_song(song)], qualities).await?;
    Ok(rights.pop().unwrap_or_default())
}

/// 将上游返回的原始权益数据组装为归一化判定模型。
fn build_quality_vip(hash: &str, _album_id: i64, resource: &Value, qualities: &[String]) -> SongRights {
    let relate_goods = codec::find_array(resource, &["relate_goods", "relategoods"])
        .cloned()
        .unwrap_or_default();

    let list: Vec<QualityRight> = qualities
        .iter()
        .map(|quality| {
            let relate = pick_relate(&relate_goods, quality);
            // 检查上游是否包含对应音质。
            let exists = relate.is_some();
            // 判定是否属于会员/付费音质。
            let need_vip = relate.is_some_and(|item| codec::pick_i64(item, &["privilege"]) != 0);
            // 概念版在标准音质下享有免费畅听权益。
            let free_listen = quality == "128" && need_vip;
            let status = relate.map_or(0, |item| codec::pick_i64(item, &["status"]));
            let obtainable = free_listen || (exists && !need_vip && status != 0);
            // 标准音质优先选用概念版专享 OGG 变体。
            let (ogg_hash, ogg_size) = ogg_of(Quality::parse(quality), resource);
            let size = if exists {
                if ogg_size > 0 {
                    ogg_size
                } else {
                    codec::pick_i64_paths(relate.unwrap_or(&Value::Null), &[&["info", "filesize"]]).max(0) as u64
                }
            } else {
                0
            };
            let extname = if ogg_hash.is_empty() {
                relate.map_or(String::new(), |item| {
                    codec::pick_str_paths(item, &[&["info", "extname"]])
                })
            } else {
                "ogg".to_owned()
            };
            let resolved_hash = if ogg_hash.is_empty() {
                relate
                    .map_or(String::new(), |item| codec::pick_str(item, &["hash"]))
                    .to_lowercase()
            } else {
                ogg_hash.clone()
            };

            QualityRight {
                quality: quality.clone(),
                exists,
                level: relate.map_or(0, |item| codec::pick_i64(item, &["level"])),
                bitrate: if exists {
                    relate
                        .map_or(0, |item| codec::pick_i64_paths(item, &[&["info", "bitrate"]]))
                        .max(0) as u64
                } else {
                    0
                },
                extname,
                hash: if exists { resolved_hash } else { String::new() },
                size,
                privilege: relate.map_or(-1, |item| codec::pick_i64(item, &["privilege"])),
                status,
                need_vip,
                obtainable,
                free_listen,
                require_vip_type: if exists && !obtainable && need_vip {
                    Quality::parse(quality).required_vip().to_owned()
                } else {
                    String::new()
                },
            }
        })
        .collect();

    let free_qualities: Vec<String> = list
        .iter()
        .filter(|q| q.obtainable)
        .map(|q| q.quality.clone())
        .collect();
    let vip_qualities: Vec<String> = list
        .iter()
        .filter(|q| q.exists && q.need_vip && !q.obtainable)
        .map(|q| q.quality.clone())
        .collect();
    let super_vip_qualities: Vec<String> = list
        .iter()
        .filter(|q| q.require_vip_type == "suvip")
        .map(|q| q.quality.clone())
        .collect();
    let missing_qualities: Vec<String> = list.iter().filter(|q| !q.exists).map(|q| q.quality.clone()).collect();
    let free_song = !list.is_empty() && list.iter().all(|q| !q.need_vip);
    // 综合 level 与音质顺序计算最优可播音质。
    let best_free_quality = list
        .iter()
        .filter(|q| q.obtainable)
        .max_by_key(|q| (q.level, order_index(&q.quality)))
        .map_or(String::new(), |q| q.quality.clone());

    SongRights {
        hash: hash.to_owned(),
        auth: codec::pick_str(resource, &["auth"]),
        name: codec::pick_str(resource, &["name"]),
        singer: codec::pick_str(resource, &["singername"]),
        albumname: codec::pick_str(resource, &["albumname"]),
        free_song,
        qualities: list,
        free_qualities,
        vip_qualities,
        super_vip_qualities,
        missing_qualities,
        best_free_quality,
    }
}

fn order_index(quality: &str) -> usize {
    QUALITY_ORDER.iter().position(|q| *q == quality).unwrap_or(usize::MAX)
}

/// 在商品关联表中查找指定音质条目。
fn pick_relate<'a>(relate_goods: &'a [Value], quality: &str) -> Option<&'a Value> {
    for level in Quality::parse(quality).level_candidates() {
        let hit = relate_goods
            .iter()
            .find(|item| codec::pick_i64(item, &["level"]) == *level && codec::pick_str(item, &["quality"]) == quality);
        if let Some(hit) = hit {
            return Some(hit);
        }
    }
    relate_goods
        .iter()
        .find(|item| codec::pick_str(item, &["quality"]) == quality)
}

/// 提取 128/320 标准音质的 OGG 变体 Hash 与文件大小。
fn ogg_of(quality: Quality, resource: &Value) -> (String, u64) {
    let Some(prefix) = quality.ogg_key_prefix() else {
        return (String::new(), 0);
    };
    let hash_key = format!("{prefix}hash");
    let size_key = format!("{prefix}filesize");
    let hash = codec::pick_path(resource, &["trans_param", hash_key.as_str()])
        .map(codec::as_str)
        .unwrap_or_default();
    let size = codec::pick_i64_paths(resource, &[&["trans_param", size_key.as_str()]]).max(0);
    (hash, u64::try_from(size).unwrap_or(0))
}

/// 查询当前账号的会员权益有效性。
pub async fn vip_status(api: &Api) -> Result<VipStatus> {
    let spec = RequestSpec::get("/v1/get_union_vip")
        .base(crate::spec::VIP_BASE)
        .param("busi_type", "concept");
    let value = api.call(spec, Freshness::ACCOUNT).await?;
    Ok(VipStatus::from_value(&value))
}

/// 每日畅听 VIP 领取结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VipClaim {
    /// 今天已经领过（上游 `297002`）—— 视为幂等成功。
    pub already_claimed: bool,
    /// 本次到账时长（小时），上游 `recharge_hours`。
    pub recharge_hours: u64,
    /// 原始响应。
    pub raw: Value,
}

/// 领取每日 24 小时畅听 VIP。
pub async fn claim_daily_vip(api: &Api) -> Result<VipClaim> {
    let session = api.session();
    crate::spec::ensure_logged_in(&session)?;
    let spec = RequestSpec::post("/youth/v1/listen_song/upgrade_vip_reward")
        .param("kugouid", session.userid())
        .param("ad_type", 1);

    // 领取是账号侧写操作：不能带毫秒扰动，上游对多出的未签名参数直接回 `20006 err signature`。
    match api.call_auth(spec).await {
        Ok(value) => Ok(VipClaim {
            already_claimed: false,
            recharge_hours: codec::pick_u64_paths(&value, &[&["data", "recharge_hours"][..], &["recharge_hours"][..]]),
            raw: value,
        }),
        // 已领取错误按幂等成功处理。
        Err(error) if error.code() == Some(Code::AlreadyClaimed) => Ok(VipClaim {
            already_claimed: true,
            recharge_hours: 0,
            raw: Value::Null,
        }),
        Err(error) => Err(error),
    }
}

/// 查询月度 VIP 领奖历史记录。
pub async fn month_vip_record(api: &Api) -> Result<Value> {
    let spec = RequestSpec::get("/youth/v1/activity/get_month_vip_record").param("latest_limit", 100);
    api.call(spec, Freshness::ACCOUNT).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::Arc;

    use super::*;
    use crate::testing::api;
    use crate::transport::MockTransport;

    fn rights_response() -> serde_json::Value {
        serde_json::json!({"status": 1, "data": [{
            "hash": "abc", "name": "夜曲", "singername": "周杰伦", "albumname": "十一月的萧邦", "album_id": 1234,
            "trans_param": {"ogg_128_hash": "OGG128", "ogg_128_filesize": 3_000_000,
                            "ogg_320_hash": "OGG320", "ogg_320_filesize": 8_000_000},
            // 128 的 level=1 是付费条目 → 触发概念版「免费听」
            "relate_goods": [
                {"level": 1, "quality": "128", "privilege": 10, "status": 1, "info": {"filesize": 2_000_000, "bitrate": 128}},
                {"level": 2, "quality": "128", "privilege": 0, "status": 1, "info": {"extname": "mp3", "bitrate": 128}},
                {"level": 4, "quality": "320", "privilege": 10, "status": 1, "info": {"bitrate": 320}},
                {"level": 5, "quality": "flac", "privilege": 10, "status": 1, "hash": "FLACHASH", "info": {"filesize": 40_000_000, "bitrate": 900}},
                {"quality": "viper_atmos", "privilege": 10, "status": 1, "info": {"bitrate": 2000}}
            ]
        }]})
    }

    #[tokio::test]
    async fn builds_the_documented_quality_matrix() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v2/get_res_privilege/lite", &rights_response());
        let api = api(mock.clone());
        let rights = privilege(
            &api,
            &[RightRequest {
                hash: "ABC".to_owned(),
                album_id: 1234,
            }],
            None,
        )
        .await
        .unwrap();
        let rights = &rights[0];

        assert_eq!(rights.name, "夜曲");
        assert!(!rights.free_song, "存在付费音质");

        // 验证 128 音质优先命中 OGG 变体。
        let q128 = rights.quality("128").unwrap();
        assert!(q128.exists);
        assert!(q128.free_listen, "128 需会员时概念版免费听");
        assert!(q128.obtainable);
        assert_eq!(q128.extname, "ogg");
        assert_eq!(q128.hash, "OGG128");
        assert_eq!(q128.size, 3_000_000);

        // 验证 320 高品质音质需要普通 VIP。
        let q320 = rights.quality("320").unwrap();
        assert!(q320.need_vip);
        assert!(!q320.obtainable);
        assert_eq!(q320.require_vip_type, "concept");

        // 验证无损音质使用关联条目的 Hash。
        let flac = rights.quality("flac").unwrap();
        assert_eq!(flac.hash, "flachash");
        assert_eq!(flac.level, 5);
        assert_eq!(flac.size, 40_000_000);

        // 验证全景声音质需要超级 VIP。
        let atmos = rights.quality("viper_atmos").unwrap();
        assert!(atmos.exists);
        assert_eq!(atmos.require_vip_type, "suvip");
        assert_eq!(atmos.level, 0);

        assert!(rights.super_vip_qualities.contains(&"viper_atmos".to_owned()));
        assert!(rights.vip_qualities.contains(&"flac".to_owned()));
        assert_eq!(rights.best_free_quality, "128");
    }

    #[tokio::test]
    async fn free_song_is_detected() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v2/get_res_privilege/lite",
            &serde_json::json!({"status": 1, "data": [{
                "hash": "abc",
                "relate_goods": [
                    {"level": 2, "quality": "128", "privilege": 0, "status": 1, "info": {"filesize": 1_000_000}},
                    {"level": 5, "quality": "flac", "privilege": 0, "status": 1, "hash": "F", "info": {"filesize": 30_000_000}}
                ]
            }]}),
        );
        let api = api(mock.clone());
        let rights = privilege_for_song(
            &api,
            &Song {
                hash: "abc".to_owned(),
                ..Song::default()
            },
            Some("128,flac"),
        )
        .await
        .unwrap();
        assert!(rights.free_song);
        // 两个音质均免费时最佳音质判定为无损。
        assert_eq!(rights.best_free_quality, "flac");
        assert_eq!(rights.qualities.len(), 2, "只返回请求的音质");
        assert!(rights.missing_qualities.is_empty());
    }

    #[tokio::test]
    async fn missing_qualities_are_reported() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v2/get_res_privilege/lite",
            &serde_json::json!({"status": 1, "data": [{"hash": "abc", "relate_goods": [
                {"level": 2, "quality": "128", "privilege": 0, "status": 1, "info": {}}
            ]}]}),
        );
        let api = api(mock.clone());
        let rights = privilege(
            &api,
            &[RightRequest {
                hash: "abc".to_owned(),
                album_id: 0,
            }],
            Some("128,flac,high"),
        )
        .await
        .unwrap();
        assert_eq!(rights[0].missing_qualities, vec!["flac".to_owned(), "high".to_owned()]);
        // 确保未提供的音质标记为不存在。
        let flac = rights[0].quality("flac").expect("请求过的音质应有条目");
        assert!(!flac.exists);
        assert_eq!(flac.privilege, -1);
        assert!(rights[0].quality("viper_tape").is_none(), "没请求过的音质不应出现");
    }

    #[tokio::test]
    async fn requested_qualities_all_expands_to_the_upstream_set() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v2/get_res_privilege/lite",
            &serde_json::json!({"status": 1, "data": [{"hash": "abc", "relate_goods": []}]}),
        );
        let api = api(mock.clone());
        let rights = privilege(
            &api,
            &[RightRequest {
                hash: "abc".to_owned(),
                album_id: 0,
            }],
            Some("all"),
        )
        .await
        .unwrap();
        assert_eq!(rights[0].qualities.len(), UPSTREAM_QUALITIES.len());

        // 验证请求体未携带 album_audio_id。
        let body: Value = serde_json::from_slice(mock.last_request().unwrap().body.as_deref().unwrap()).unwrap();
        assert_eq!(body["resource"][0]["type"], "audio");
        assert!(body["resource"][0].get("album_audio_id").is_none());
        assert_eq!(body["need_hash_offset"], 1);
        assert_eq!(body["relate"], 1);
    }

    #[tokio::test]
    async fn vip_status_reads_busi_vip() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v1/get_union_vip",
            &serde_json::json!({"status": 1, "vip_type": 0, "is_vip": 0, "data": {"busi_vip": [
                {"busi_type": "concept", "product_type": "svip", "vip_begin_time": 0, "vip_end_time": 4102444800000_i64}
            ]}}),
        );
        let api = api(mock.clone());
        let status = vip_status(&api).await.unwrap();
        assert!(status.is_concept_active());
        let request = mock.last_request().unwrap();
        assert!(request.url.starts_with("https://kugouvip.kugou.com/v1/get_union_vip"));
        assert_eq!(
            request.query.iter().find(|(k, _)| k == "busi_type").unwrap().1,
            "concept"
        );
    }

    #[tokio::test]
    async fn daily_claim_reports_hours() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/youth/v1/listen_song/upgrade_vip_reward",
            &serde_json::json!({"status": 1, "data": {"recharge_hours": 24}}),
        );
        let api = api(mock.clone());
        let claim = claim_daily_vip(&api).await.unwrap();
        assert!(!claim.already_claimed);
        assert_eq!(claim.recharge_hours, 24);
        let request = mock.last_request().unwrap();
        assert_eq!(request.query.iter().find(|(k, _)| k == "ad_type").unwrap().1, "1");
        assert_eq!(
            request.query.iter().find(|(k, _)| k == "kugouid").unwrap().1,
            "2043841403"
        );
        // 账号侧写操作不能带毫秒扰动：上游会回 20006 err signature。
        assert!(
            !request.query.iter().any(|(k, _)| k == "_lyra_t"),
            "{:?}",
            request.query
        );
    }

    #[tokio::test]
    async fn duplicate_daily_claim_is_idempotent_success() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/youth/v1/listen_song/upgrade_vip_reward",
            &serde_json::json!({"status": 0, "error_code": 297002, "error": "已领取"}),
        );
        let api = api(mock.clone());
        let claim = claim_daily_vip(&api).await.unwrap();
        assert!(claim.already_claimed, "297002 必须按幂等成功处理");
        assert_eq!(claim.recharge_hours, 0);
    }
}
