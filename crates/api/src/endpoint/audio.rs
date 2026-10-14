//! 音频播放地址解析与获取端点。

use lyra_proto::consts::{P_PAGE_ID, PAGE_ID, PID};
use lyra_proto::random_string;

use crate::cache::Freshness;
use crate::client::Api;
use crate::error::{Error, Result};
use crate::model::SongUrl;
use crate::spec::RequestSpec;

/// 请求指定歌曲与音质的实际播放地址。
pub async fn song_url(
    api: &Api,
    hash: &str,
    album_id: i64,
    album_audio_id: i64,
    quality: &str,
    auth: &str,
) -> Result<SongUrl> {
    let session = api.session();
    // 未注册设备使用随机设备标识生成签名。
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

    let value = api.call(spec, Freshness::SONG_URL).await?;
    SongUrl::from_value(&value).ok_or_else(|| Error::Decode("上游未返回可用播放地址（可能是版权/会员限制）".to_owned()))
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
        assert!(get("key").is_some(), "signKey 必须注入");
        let dfid = get("dfid").unwrap();
        assert_ne!(dfid, "-");
        assert_eq!(dfid.len(), 24);
        assert_eq!(request.get_header("dfid"), Some(dfid.as_str()));
    }

    #[tokio::test]
    async fn missing_url_is_a_decode_error() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/tracker/v5/url",
            &serde_json::json!({"status": 3, "data": {"url": []}}),
        );
        let api = api_with_dfid(mock.clone(), "-");
        assert!(song_url(&api, "ABC", 0, 0, "flac", "").await.is_err());
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
