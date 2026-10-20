//! 推荐流业务端点。

use lyra_proto::consts::{APPID, CLIENTVER};
use lyra_proto::sign_params_key;
use serde_json::Value;

use crate::cache::Freshness;
use crate::client::Api;
use crate::codec;
use crate::error::Result;
use crate::model::{Page, Song};
use crate::spec::RequestSpec;
use crate::spec::now_millis;

/// 首页发现接口客户端版本号
pub const DISCOVER_CLIENTVER: u32 = 20_809;

/// 获取每日推荐歌曲列表
pub async fn daily(api: &Api, platform: &str) -> Result<Page<Song>> {
    let body = serde_json::json!({
        "platform": if platform.is_empty() { "android" } else { platform },
        // 上游要求 int64：字符串会被判为类型错误（20002）
        "userid": api.session().userid(),
    });
    let spec = RequestSpec::post("/everyday_song_recommend")
        .body(body)
        .header("x-router", "everydayrec.service.kugou.com");
    let value = api.call(spec, Freshness::FLOW).await?;
    Ok(Page::single(songs_of(&value)))
}

/// 分页获取私人 FM 电台歌曲
pub async fn personal_fm(
    api: &Api,
    action: &str,
    mode: &str,
    cursor: Option<&str>,
    previous: Option<&Song>,
) -> Result<Page<Song>> {
    let session = api.session();
    let date_time = now_millis();
    let mut body = serde_json::json!({
        "appid": APPID,
        "clienttime": date_time,
        "mid": session.device.mid,
        "action": if action.is_empty() { "play" } else { action },
        "recommend_source_locked": 0,
        "song_pool_id": 0,
        "callerid": 0,
        "m_type": 1,
        "platform": "ios",
        "area_code": 1,
        "remain_songcnt": 0,
        "clientver": CLIENTVER,
        "is_overplay": 0,
        "mode": if mode.is_empty() { "normal" } else { mode },
        "fakem": "ca981cfc583a4c37f28d2d49000013c16a0a",
        "key": sign_params_key(date_time, APPID, CLIENTVER),
    });
    if session.is_logged_in() {
        body["userid"] = serde_json::json!(session.userid());
        body["kguid"] = serde_json::json!(session.userid());
        body["token"] = serde_json::json!(session.token());
        if session.vip_type() != 0 {
            body["vip_type"] = serde_json::json!(session.vip_type());
        }
    }
    if let Some(song) = previous {
        body["hash"] = serde_json::json!(song.hash);
        body["songid"] = serde_json::json!(song.mixsongid);
    }
    if let Some(cursor) = cursor {
        body["cur_mark"] = serde_json::json!(cursor);
    }

    let spec = RequestSpec::post("/v2/personal_recommend")
        .body(body)
        .header("x-router", "persnfm.service.kugou.com");
    let value = api.call(spec, Freshness::FLOW).await?;
    let cursor = codec::find_str(&value, &["sync_point", "cur_mark", "syncpoint"]);
    Ok(Page {
        items: songs_of(&value),
        cursor: if cursor.is_empty() { None } else { Some(cursor) },
        meta: codec::find(&value, &["fresh_mode", "mode"])
            .cloned()
            .unwrap_or(Value::Null),
    })
}

/// 获取首页发现推荐歌曲
pub async fn home_discover(api: &Api, pagesize: u32, today_play_num: u32) -> Result<Page<Song>> {
    let userid = api.session().userid();
    let body = serde_json::json!({
        "support": "only_song",
        "userid": userid,
        "recall_type": "song",
        "today_play_num": today_play_num,
        "pagesize": if pagesize == 0 { 4 } else { pagesize },
        "go_ky_extra": [
            {"key": "network", "val": "2"},
            {"key": "play_mode", "val": "2"},
            {"key": "no_mv_ret", "val": "1"},
        ],
    });
    let spec = RequestSpec::post("/homediscoverrec/v1/client/home_discover_rec")
        .body(body)
        .param("module_key", "home_discover_rec")
        .param("module_id", 1)
        .param("area_code", 1)
        .param("platform", "android")
        .param("userid", userid)
        .param("clientver", DISCOVER_CLIENTVER);
    let value = api.call(spec, Freshness::FLOW).await?;
    Ok(Page::single(songs_of(&value)))
}

/// 从响应体多变结构中解析歌曲列表
fn songs_of(value: &Value) -> Vec<Song> {
    codec::find_array(value, &["song_list", "songs", "items", "list", "data"])
        .map(|items| Song::list_from(items))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::testing::api;
    use crate::transport::MockTransport;

    #[tokio::test]
    async fn daily_reads_song_list() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/everyday_song_recommend",
            &serde_json::json!({"status": 1, "data": {"song_list": [
                {"name": "A", "hash": "AA"}, {"name": "B", "hash": "BB"}
            ]}}),
        );
        let api = api(mock.clone());
        let page = daily(&api, "").await.unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(!page.has_more(), "每日推荐无游标");
        assert_eq!(
            mock.last_request().unwrap().get_header("x-router"),
            Some("everydayrec.service.kugou.com")
        );
    }

    #[tokio::test]
    async fn daily_forces_post_numeric_userid() {
        // 上游把请求体的 userid 解析为 int64：传字符串会被判类型错误（20002）。
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/everyday_song_recommend",
            &serde_json::json!({"status": 1, "data": {"song_list": []}}),
        );
        let api = api(mock.clone());
        daily(&api, "").await.unwrap();
        let request = mock.last_request().unwrap();
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert!(body["userid"].is_number(), "userid 必须是数字，不能是字符串");
        assert_eq!(body["userid"], 2_043_841_403);
    }

    #[tokio::test]
    async fn personal_fm_produces_a_cursor() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v2/personal_recommend",
            &serde_json::json!({"status": 1, "data": {"song_list": [{"name": "A", "hash": "AA"}], "sync_point": "s=5", "mode": "normal"}}),
        );
        let api = api(mock.clone());
        let page = personal_fm(&api, "play", "normal", None, None).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.cursor.as_deref(), Some("s=5"));
        assert!(page.has_more());

        let body: Value = serde_json::from_slice(mock.last_request().unwrap().body.as_deref().unwrap()).unwrap();
        assert_eq!(body["callerid"], 0);
        assert_eq!(body["m_type"], 1);
        assert_eq!(body["fakem"], "ca981cfc583a4c37f28d2d49000013c16a0a");
        assert!(body["key"].is_string(), "个人电台需要 signParamsKey");
        assert_eq!(body["kguid"], 2_043_841_403);
    }

    #[tokio::test]
    async fn home_discover_forces_post_numeric_userid_and_clientver_20809() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/homediscoverrec/v1/client/home_discover_rec",
            &serde_json::json!({"status": 1, "data": {"items": [{"name": "A", "hash": "AA"}]}}),
        );
        let api = api(mock.clone());
        let page = home_discover(&api, 4, 0).await.unwrap();
        assert_eq!(page.items.len(), 1);

        let request = mock.last_request().unwrap();
        assert_eq!(request.method, crate::transport::Method::Post, "GET 必失败");
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert!(body["userid"].is_number(), "userid 必须是数字，不能是字符串");
        assert_eq!(body["userid"], 2_043_841_403);
        let clientver = request.query.iter().find(|(k, _)| k == "clientver").unwrap();
        assert_eq!(clientver.1, "20809", "必须覆盖 lite 的 11440");
        let platform = request.query.iter().find(|(k, _)| k == "platform").unwrap();
        assert_eq!(platform.1, "android");
    }
}
