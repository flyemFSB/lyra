//! 音频播放地址获取端点。

use lyra_proto::consts::{P_PAGE_ID, PAGE_ID, PID};
use lyra_proto::random_string;
use serde_json::Value;

use crate::cache::Freshness;
use crate::client::Api;
use crate::codec;
use crate::error::{Error, Result};
use crate::model::SongUrl;
use crate::spec::RequestSpec;

/// 上游无版权状态码（status=3）
const STATUS_NO_COPYRIGHT: i64 = 3;

/// 音频获取模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// 完整音轨
    Full,
    /// 试听片段（IsFreePart=1）
    Preview,
}

/// 获取指定歌曲与音质的播放地址
///
/// 仅授权试听时自动回退提取试听片段并记录 preview_ms。
pub async fn song_url(
    api: &Api,
    hash: &str,
    album_id: i64,
    album_audio_id: i64,
    quality: &str,
    auth: &str,
) -> Result<SongUrl> {
    let value = api
        .call(
            spec(api, hash, album_id, album_audio_id, quality, auth, Part::Full),
            Freshness::SONG_URL,
        )
        .await?;
    if let Some(url) = SongUrl::from_value(&value) {
        return Ok(url);
    }

    let preview_ms = preview_end_ms(&value);
    if preview_ms > 0 {
        // 降级请求试听片段
        let retry = spec(api, hash, album_id, album_audio_id, quality, auth, Part::Preview);
        if let Ok(preview_value) = api.call(retry, Freshness::SONG_URL).await
            && let Some(mut url) = SongUrl::from_value(&preview_value)
        {
            url.preview_ms = preview_ms;
            url.duration_ms = preview_ms;
            // 替换为试听片段独立哈希
            let clip = clip_hash(&preview_value);
            if !clip.is_empty() {
                url.hash = clip;
            }
            return Ok(url);
        }
    }

    Err(classify(&value))
}

/// 构造播放地址获取请求规格
fn spec(
    api: &Api,
    hash: &str,
    album_id: i64,
    album_audio_id: i64,
    quality: &str,
    auth: &str,
    part: Part,
) -> RequestSpec {
    let session = api.session();
    let dfid = if session.device.dfid.is_empty() || session.device.dfid == "-" {
        random_string(24)
    } else {
        session.device.dfid.clone()
    };

    let spec = RequestSpec::get("/tracker/v5/url")
        .dfid(dfid)
        .param("album_id", album_id)
        .param("area_code", 1)
        .param("module", "")
        .param("hash", hash.to_lowercase())
        .param("need_m", 0)
        .param("ssa_flag", "is_fromtrack")
        .param("version", 11_430)
        .param("open_time", 0)
        .param("ptype", 0)
        .param("need_ogg", 1)
        .param("page_id", PAGE_ID)
        .param("auth", auth)
        .param("mtype", 0)
        .param("quality", if quality.is_empty() { "128" } else { quality })
        .param("album_audio_id", album_audio_id)
        .param("behavior", "play")
        .param("pid", PID)
        .param("module_id", 51)
        .param("cmd", 26)
        .param("ppage_id", P_PAGE_ID)
        .param("clientver", 11_561)
        .param("pidversion", 3001)
        .param("cdnBackup", 1)
        .encrypt_key();

    match part {
        Part::Full => spec,
        // 试听片段模式设置 IsFreePart=1
        Part::Preview => spec.param("IsFreePart", 1),
    }
}

/// 提取试听片段结束时间戳（毫秒）
fn preview_end_ms(value: &Value) -> u64 {
    codec::pick_u64_paths(value, &[&["hash_offset", "end_ms"], &["hashOffset", "endMs"]])
}

/// 提取试听片段独立哈希
fn clip_hash(value: &Value) -> String {
    codec::pick_str_paths(value, &[&["hash_offset", "clip_hash"], &["hashOffset", "clipHash"]]).to_lowercase()
}

/// 分析上游拒绝原因并归类错误
fn classify(value: &Value) -> Error {
    let status = codec::pick_i64(value, &["status"]);
    let fail_process = codec::pick(value, &["fail_process"])
        .map(ToString::to_string)
        .unwrap_or_default();
    let cpy_grade = codec::pick_i64_paths(value, &[&["tracker_through", "cpy_grade"]]);
    let detail = format!("status={status} fail_process={fail_process} cpy_grade={cpy_grade}");
    if status == STATUS_NO_COPYRIGHT {
        return Error::NoCopyright(detail);
    }
    if !fail_process.is_empty() && fail_process != "[]" {
        return Error::NotEntitled(detail);
    }
    Error::Decode(format!("上游未返回可用播放地址（{detail}）"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::testing::api_with_dfid;
    use crate::transport::MockTransport;

    #[tokio::test]
    async fn song_url_sends_the_documented_params_and_sign_key() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/tracker/v5/url",
            &serde_json::json!({"status": 1, "data": {"url": [
                {"url": "http://fs.youthandroid2.kugou.com/x/ap3116_us1_quflac_pa967177915.flac",
                 "extname": "flac", "filesize": 40000000, "timelength": 226000, "hash": "ABC"}
            ]}}),
        );
        let api = api_with_dfid(mock.clone(), "-");
        let url = song_url(&api, "ABC123", 1234, 98765, "flac", "").await.unwrap();
        assert_eq!(url.ext, "flac");
        assert_eq!(url.ext_or_guess(), "flac");
        assert_eq!(url.preview_ms, 0, "完整曲不能被标成试听");

        let request = mock.last_request().unwrap();
        assert_eq!(request.url, "https://gateway.kugou.com/tracker/v5/url");
        let get = |k: &str| request.query.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        assert_eq!(get("page_id").unwrap(), "967177915");
        assert_eq!(get("pid").unwrap(), "411");
        assert_eq!(get("ppage_id").unwrap(), "356753938,823673182,967485191");
        assert_eq!(get("hash").unwrap(), "abc123");
        assert_eq!(get("need_ogg").unwrap(), "1");
        assert_eq!(get("pidversion").unwrap(), "3001");
        assert_eq!(get("version").unwrap(), "11430");
        assert_eq!(get("clientver").unwrap(), "11561");
        assert_eq!(get("IsFreePart"), None, "完整曲请求不应带试听标记");
        assert!(get("key").is_some(), "signKey 必须注入");
        let dfid = get("dfid").unwrap();
        assert_ne!(dfid, "-");
        assert_eq!(dfid.len(), 24);
        assert_eq!(request.get_header("dfid"), Some(dfid.as_str()));
    }

    #[tokio::test]
    async fn status_three_means_the_concept_edition_has_no_copyright() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/tracker/v5/url",
            &serde_json::json!({
                "status": 3, "fail_process": [], "priv_status": 0,
                "tracker_through": {"cpy_grade": 75},
                "trans_param": {"cpy_map": "2"}
            }),
        );
        let api = api_with_dfid(mock.clone(), "-");
        let error = song_url(&api, "ABC", 0, 0, "flac", "").await.unwrap_err();
        assert!(
            matches!(error, Error::NoCopyright(_)),
            "status=3 必须报无版权，不能是笼统的解码错误: {error:?}"
        );
        assert_eq!(mock.count(), 1, "没有试听片段时不该再发第二次请求");
    }

    #[tokio::test]
    async fn purchase_only_tracks_report_missing_entitlement() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/tracker/v5/url",
            &serde_json::json!({"status": 2, "fail_process": ["buy"], "priv_status": 0}),
        );
        let api = api_with_dfid(mock.clone(), "-");
        let error = song_url(&api, "ABC", 0, 0, "128", "").await.unwrap_err();
        assert!(
            matches!(error, Error::NotEntitled(_)),
            "只有购买路径时是权益问题: {error:?}"
        );
    }

    #[tokio::test]
    async fn trial_only_tracks_are_returned_as_a_playable_preview_clip() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/tracker/v5/url",
            &serde_json::json!({
                "status": 2, "priv_status": 0, "fail_process": ["pkg", "buy"],
                "hash_offset": {"start_byte": 0, "end_byte": 960157, "start_ms": 0, "end_ms": 60000,
                                 "offset_hash": "OFFSET"}
            }),
        );
        mock.on_json(
            "IsFreePart=1",
            &serde_json::json!({
                "status": 1,
                "url": ["http://fs.youthandroid2.kugou.com/x/p_0_960157/ap3116_qu128.mp3"],
                "extName": "mp3",
                "hash_offset": {"end_ms": 60000, "clip_hash": "CLIPHASH"}
            }),
        );
        let api = api_with_dfid(mock.clone(), "-");
        let url = song_url(&api, "ABC", 0, 7, "128", "").await.unwrap();

        assert_eq!(url.preview_ms, 60_000, "试听时长来自 hash_offset.end_ms");
        assert_eq!(url.duration_ms, 60_000, "进度条必须按片段时长走，而不是整首");
        assert_eq!(url.hash, "cliphash", "片段 hash 优先，保证缓存键与完整曲分开");
        assert_eq!(url.blob_key("abc"), "cliphash_preview");
        assert_eq!(mock.count(), 2, "先取完整曲，再取试听片段");
        let retry = mock.last_request().unwrap();
        assert!(
            retry.query.iter().any(|(k, v)| k == "IsFreePart" && v == "1"),
            "第二次请求必须带 IsFreePart=1"
        );
    }

    #[tokio::test]
    async fn registered_device_gets_a_stable_dfid_so_the_cache_works() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/tracker/v5/url",
            &serde_json::json!({"status": 1, "data": {"url": [{"url": "http://c/x.flac", "extname": "flac"}]}}),
        );
        let api = api_with_dfid(mock.clone(), "DfidValue");
        song_url(&api, "ABC", 0, 0, "flac", "").await.unwrap();
        song_url(&api, "ABC", 0, 0, "flac", "").await.unwrap();
        assert_eq!(mock.count(), 1, "60 秒内应命中缓存");
        assert_eq!(Freshness::SONG_URL.ttl(), Some(std::time::Duration::from_secs(60)));
        assert_eq!(
            mock.last_request().unwrap().get_header("dfid"),
            Some("DfidValue"),
            "已注册设备应沿用固定 dfid"
        );
    }

    #[tokio::test]
    async fn unregistered_device_randomizes_dfid_so_each_call_is_a_cache_miss() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/tracker/v5/url",
            &serde_json::json!({"status": 1, "data": {"url": [{"url": "http://c/x.flac", "extname": "flac"}]}}),
        );
        let api = api_with_dfid(mock.clone(), "-");
        song_url(&api, "ABC", 0, 0, "flac", "").await.unwrap();
        song_url(&api, "ABC", 0, 0, "flac", "").await.unwrap();
        // 未注册设备因 dfid 随机化将绕过缓存。
        assert_eq!(mock.count(), 2);
        let requests = mock.requests();
        let first = requests[0].query.iter().find(|(k, _)| k == "dfid").unwrap().1.clone();
        let second = requests[1].query.iter().find(|(k, _)| k == "dfid").unwrap().1.clone();
        assert_ne!(first, second);
    }
}
