//! 音质权益矩阵与会员领奖端点。

use lyra_proto::consts::{APPID, CLIENTVER};
use serde_json::Value;

use crate::cache::Freshness;
use crate::client::Api;
use crate::codec;
use crate::error::{Code, Result};
use crate::model::{QualityRight, Song, SongRights, VipStatus};
use crate::spec::RequestSpec;

/// 音质等级排序（从低到高）
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

/// 上游全量音质查询列表
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

/// 客户端默认请求的音质列表
const DEFAULT_QUALITIES: &str = "128,320,flac,high,viper_tape,viper_clear,viper_atmos";

/// 音质权益查询项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RightRequest {
    /// 音频 hash
    pub hash: String,
    /// 专辑 ID
    pub album_id: i64,
}

impl RightRequest {
    /// 从歌曲构建查询项
    #[must_use]
    pub fn from_song(song: &Song) -> Self {
        Self {
            hash: song.hash.clone(),
            album_id: song.album_id.parse().unwrap_or(0),
        }
    }
}

/// 音质枚举表示
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quality {
    /// 标准音质（128k，概念版免费听）
    Standard,
    /// 高品音质（320k）
    High,
    /// 无损音质（FLAC）
    Flac,
    /// Hi-Res 高解析
    HiRes,
    /// 超高清音质
    Super,
    /// 蝰蛇母带音质
    ViperTape,
    /// 蝰蛇超清音质
    ViperClear,
    /// 蝰蛇全景声音质
    ViperAtmos,
    /// 多轨音质
    Multitrack,
    /// 未知音质
    Unknown,
}

impl Quality {
    /// 解析音质枚举
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

    /// 获取 relate_goods 候选等级标识
    fn level_candidates(self) -> &'static [i64] {
        match self {
            Self::Standard => &[1, 2],
            Self::High => &[4],
            Self::Flac => &[5],
            Self::HiRes => &[6],
            Self::Super => &[7],
            // 蝰蛇系与多轨在上游按 quality 字段匹配
            Self::ViperTape | Self::ViperClear | Self::ViperAtmos | Self::Multitrack | Self::Unknown => &[],
        }
    }

    /// 获取音质所需的 VIP 会员级别
    fn required_vip(self) -> &'static str {
        match self {
            Self::ViperTape | Self::ViperClear | Self::ViperAtmos => "suvip",
            Self::Standard | Self::High | Self::Flac | Self::HiRes | Self::Super | Self::Multitrack | Self::Unknown => {
                "concept"
            }
        }
    }

    /// 获取 OGG 变体参数前缀
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

/// 批量查询歌曲音质权益矩阵
pub async fn privilege(api: &Api, items: &[RightRequest], qualities: Option<&str>) -> Result<Vec<SongRights>> {
    if items.is_empty() {
        return Err(crate::error::Error::Decode("缺少歌曲 hash".to_owned()));
    }

    // 避免携带 album_audio_id 以获取标准音质 OGG 变体
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
        // all 表示查询全部音质
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

    // 一次性建立 hash → 权益条目的查找表：逐项扫描是 O(n²)，且每次比较都要分配字符串
    let mut by_hash: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for candidate in &data {
        let hash = codec::pick_str_cow(candidate, &["hash"]);
        if hash.is_empty() {
            continue;
        }
        by_hash.entry(hash.to_lowercase()).or_insert_with(|| candidate.clone());
    }

    Ok(items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let hash = item.hash.to_lowercase();
            // 依据 Hash 进行结果关联匹配
            let entry = by_hash
                .get(&hash)
                .cloned()
                .or_else(|| data.get(index).cloned())
                .unwrap_or(Value::Null);
            build_quality_vip(&hash, item.album_id, &entry, &wanted)
        })
        .collect())
}

/// 查询单曲音质权益详情
pub async fn privilege_for_song(api: &Api, song: &Song, qualities: Option<&str>) -> Result<SongRights> {
    let mut rights = privilege(api, &[RightRequest::from_song(song)], qualities).await?;
    Ok(rights.pop().unwrap_or_default())
}

/// 组装音质权益判定结果
fn build_quality_vip(hash: &str, _album_id: i64, resource: &Value, qualities: &[String]) -> SongRights {
    let relate_goods = codec::find_array(resource, &["relate_goods", "relategoods"])
        .cloned()
        .unwrap_or_default();

    // 概念版下架或无版权标记：资源级 privilege=5 或响应提示 Deny
    // 注意：不能依 status=0 判定，VIP 音源条目的 status 亦为 0
    let denied_resource = codec::pick_i64(resource, &["privilege"]) == 5
        || codec::pick_str_cow(resource, &["_msg"]).to_lowercase().contains("deny");

    let list: Vec<QualityRight> = qualities
        .iter()
        .map(|quality| {
            let relate = pick_relate(&relate_goods, quality);
            let exists = relate.is_some();
            // 上游 privilege：0 免费，5 无版权/下架，8/10 付费或会员
            let privilege = relate.map_or(-1, |item| codec::pick_i64(item, &["privilege"]));
            let need_vip = exists && privilege != 0;
            let status = relate.map_or(0, |item| codec::pick_i64(item, &["status"]));
            // 下架或无版权音源无法通过免费听权益获取
            let no_copyright = exists && denied_resource;
            // 概念版 128 标准音质支持免费畅听（无版权除外）
            let free_listen = quality == "128" && need_vip && !no_copyright;
            let obtainable = exists && !no_copyright && (free_listen || (!need_vip && status != 0));
            // 标准音质优先选用概念版专享 OGG 变体
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
                relate.map_or(String::new(), |item| {
                    codec::pick_str_cow(item, &["hash"]).to_lowercase()
                })
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
                privilege,
                status,
                need_vip,
                no_copyright,
                obtainable,
                free_listen,
                require_vip_type: if exists && !obtainable && need_vip && !no_copyright {
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
        .filter(|q| q.exists && q.need_vip && !q.obtainable && !q.no_copyright)
        .map(|q| q.quality.clone())
        .collect();
    let super_vip_qualities: Vec<String> = list
        .iter()
        .filter(|q| q.require_vip_type == "suvip")
        .map(|q| q.quality.clone())
        .collect();
    let missing_qualities: Vec<String> = list.iter().filter(|q| !q.exists).map(|q| q.quality.clone()).collect();
    let free_song = !list.is_empty() && list.iter().all(|q| !q.need_vip);
    // 按层级与音质优先级选取最优可用音质
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

/// 在商品关联表中查找指定音质条目
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

/// 提取 128/320 音质的 OGG 变体哈希与文件大小
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

/// 查询当前账号的会员状态
pub async fn vip_status(api: &Api) -> Result<VipStatus> {
    let spec = RequestSpec::get("/v1/get_union_vip")
        .base(crate::spec::VIP_BASE)
        .param("busi_type", "concept");
    let value = api.call(spec, Freshness::NoStore).await?;
    Ok(VipStatus::from_value(&value))
}

/// 每日畅听 VIP 领取结果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VipClaim {
    /// 今日是否已领取（上游 297002，按幂等成功处理）
    pub already_claimed: bool,
    /// 到账时长（小时，对应上游 recharge_hours）
    pub recharge_hours: u64,
    /// 原始响应数据
    pub raw: Value,
}

/// 领取每日畅听 VIP（自然日内有效，有效期至当天 23:59:59）
///
/// 酷狗概念版（Youth/Lite）采用双阶段听歌激励：
/// 1. 尝试升级/领取激励端点 (`/youth/v1/listen_song/upgrade_vip_reward`)；
/// 2. 若上游返回非幂等错误（如 297000 未达条件），回退尝试基础领取端点 (`/youth/v1/recharge/receive_vip_listen_song`)。
/// 3. 若任一端点成功或返回 297002（今日已领），均按幂等成功处理。
pub async fn claim_daily_vip(api: &Api) -> Result<VipClaim> {
    let session = api.session();
    crate::spec::ensure_logged_in(&session)?;

    let is_already_claimed = |err: &crate::error::Error| {
        err.code() == Some(Code::AlreadyClaimed) || err.code() == Some(Code::Other(297_002))
    };

    // 1. 优先尝试升级激励端点
    let upgrade_spec = RequestSpec::post("/youth/v1/listen_song/upgrade_vip_reward")
        .param("kugouid", session.userid())
        .param("ad_type", 1);

    match api.call_auth(upgrade_spec).await {
        Ok(value) => Ok(VipClaim {
            already_claimed: false,
            recharge_hours: codec::pick_u64_paths(&value, &[&["data", "recharge_hours"][..], &["recharge_hours"][..]]),
            raw: value,
        }),
        Err(error) if is_already_claimed(&error) => Ok(VipClaim {
            already_claimed: true,
            recharge_hours: 0,
            raw: Value::Null,
        }),
        Err(first_error) => {
            // 2. 若升级端点未达条件（如 297000）或失败，回退尝试基础听歌激励端点
            let receive_spec = RequestSpec::post("/youth/v1/recharge/receive_vip_listen_song")
                .param("kugouid", session.userid())
                .param("ad_type", 1);

            match api.call_auth(receive_spec).await {
                Ok(value) => Ok(VipClaim {
                    already_claimed: false,
                    recharge_hours: codec::pick_u64_paths(
                        &value,
                        &[&["data", "recharge_hours"][..], &["recharge_hours"][..]],
                    ),
                    raw: value,
                }),
                Err(error) if is_already_claimed(&error) => Ok(VipClaim {
                    already_claimed: true,
                    recharge_hours: 0,
                    raw: Value::Null,
                }),
                Err(_) => Err(first_error),
            }
        }
    }
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
            // 128 音质 level=1 为付费条目，触发概念版免费听权益
            "relate_goods": [
                {"level": 1, "quality": "128", "privilege": 10, "status": 1, "info": {"filesize": 2_000_000, "bitrate": 128}},
                {"level": 2, "quality": "128", "privilege": 0, "status": 1, "info": {"extname": "mp3", "bitrate": 128}},
                {"level": 4, "quality": "320", "privilege": 10, "status": 1, "info": {"bitrate": 320}},
                {"level": 5, "quality": "flac", "privilege": 10, "status": 1, "hash": "FLACHASH", "info": {"filesize": 40_000_000, "bitrate": 900}},
                {"quality": "viper_atmos", "privilege": 10, "status": 1, "info": {"bitrate": 2000}}
            ]
        }]})
    }

    // 模拟上游无版权/下架音源响应
    fn no_copyright_response() -> serde_json::Value {
        serde_json::json!({"status": 1, "data": [{
            "hash": "ABC", "name": "阴天", "singername": "汪苏泷", "privilege": 5, "status": 0,
            "_msg": "Deny: the current behavior of the audio is forbidden.",
            "trans_param": {"cpy_grade": 75, "cpy_map": "2"},
            "relate_goods": [
                {"level": 2, "quality": "128", "privilege": 5, "status": 0},
                {"level": 5, "quality": "flac", "privilege": 5, "status": 0}
            ]
        }]})
    }

    #[tokio::test]
    async fn no_copyright_resources_are_never_offered_as_free_listen() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v2/get_res_privilege/lite", &no_copyright_response());
        let api = api(mock.clone());
        let rights = privilege(
            &api,
            &[RightRequest {
                hash: "ABC".to_owned(),
                album_id: 0,
            }],
            None,
        )
        .await
        .unwrap();
        let rights = &rights[0];

        assert!(rights.is_no_copyright(), "privilege=5 / status=0 必须标成无版权");
        let q128 = rights.quality("128").unwrap();
        assert!(q128.exists);
        assert!(!q128.free_listen, "无版权音源不是「免费听」，请求它必然拿不到地址");
        assert!(!q128.obtainable);
        assert_eq!(q128.require_vip_type, "", "这不是会员问题，不能提示开会员");
        assert!(rights.free_qualities.is_empty());
        assert!(rights.vip_qualities.is_empty(), "无版权音源不该列入「需要 VIP」");
        assert!(rights.best_free_quality.is_empty());
    }

    #[tokio::test]
    async fn a_resource_level_deny_message_also_means_no_copyright() {
        // 上游响应包含 Deny 时同样视为无版权
        let mut response = no_copyright_response();
        response["data"][0]["privilege"] = serde_json::json!(10);
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v2/get_res_privilege/lite", &response);
        let api = api(mock.clone());
        let rights = privilege(
            &api,
            &[RightRequest {
                hash: "ABC".to_owned(),
                album_id: 0,
            }],
            None,
        )
        .await
        .unwrap();
        assert!(rights[0].is_no_copyright());
        assert!(!rights[0].quality("128").unwrap().free_listen);
    }

    #[tokio::test]
    async fn vip_resources_keep_their_free_standard_quality() {
        // VIP 音源的 relate_goods.status 正常为 0，不能误判为无版权
        let mut response = rights_response();
        for entry in response["data"][0]["relate_goods"].as_array_mut().unwrap() {
            entry["status"] = serde_json::json!(0);
        }
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v2/get_res_privilege/lite", &response);
        let api = api(mock.clone());
        let rights = privilege(
            &api,
            &[RightRequest {
                hash: "ABC".to_owned(),
                album_id: 0,
            }],
            None,
        )
        .await
        .unwrap();
        let q128 = rights[0].quality("128").unwrap();
        assert!(!rights[0].is_no_copyright());
        assert!(q128.free_listen && q128.obtainable, "VIP 曲的 128 免费听必须保留");
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

        // 128 音质优先选取 OGG 变体
        let q128 = rights.quality("128").unwrap();
        assert!(q128.exists);
        assert!(q128.free_listen, "128 需会员时概念版免费听");
        assert!(q128.obtainable);
        assert_eq!(q128.extname, "ogg");
        assert_eq!(q128.hash, "OGG128");
        assert_eq!(q128.size, 3_000_000);

        // 320 音质需要普通 VIP
        let q320 = rights.quality("320").unwrap();
        assert!(q320.need_vip);
        assert!(!q320.obtainable);
        assert_eq!(q320.require_vip_type, "concept");

        // 无损音质取关联条目哈希
        let flac = rights.quality("flac").unwrap();
        assert_eq!(flac.hash, "flachash");
        assert_eq!(flac.level, 5);
        assert_eq!(flac.size, 40_000_000);

        // 全景声音质需要超级 VIP
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
        // 均免费时优先选取高规格音质
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
        // 未匹配音质标记为不存在
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

        // 验证请求体未携带 album_audio_id
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
        // 账号写操作不应携带时间扰动参数
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
