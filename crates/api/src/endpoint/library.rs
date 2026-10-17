//! 音乐库与用户信息端点。

use lyra_proto::consts::RSA_PUBLIC;
use lyra_proto::rsa_raw_encrypt_hex;
use serde_json::Value;

use crate::cache::Freshness;
use crate::client::Api;
use crate::codec;
use crate::error::Result;
use crate::model::{Page, Playlist, Song};
use crate::spec::RequestSpec;
use crate::spec::now_millis;

/// 获取当前登录用户歌单列表
pub async fn playlists(api: &Api) -> Result<Vec<Playlist>> {
    let session = api.session();
    crate::spec::ensure_logged_in(&session)?;
    let userid = session.userid();

    let body = serde_json::json!({
        "userid": userid,
        "token": session.token(),
        "total_ver": 979,
        "type": 2,
        "page": 1,
        "pagesize": 30,
    });
    let spec = RequestSpec::post("/v7/get_all_list")
        .body(body)
        .param("plat", 1)
        .param("userid", userid)
        .param("token", session.token())
        .header("x-router", crate::cloud::CLOUD_ROUTER);

    let value = api.call(spec, Freshness::PLAYLIST).await?;
    Ok(codec::find_array(&value, &["info", "info_list", "list", "playlists"])
        .map(|items| items.iter().filter_map(Playlist::from_value).collect())
        .unwrap_or_default())
}

/// 分页获取指定歌单内的歌曲列表
pub async fn playlist_tracks(api: &Api, global_collection_id: &str, page: u32, pagesize: u32) -> Result<Page<Song>> {
    let begin_idx = u64::from(page.saturating_sub(1)) * u64::from(pagesize);
    let spec = RequestSpec::get("/pubsongs/v2/get_other_list_file_nofilt")
        .param("area_code", 1)
        .param("begin_idx", begin_idx)
        .param("plat", 1)
        .param("type", 1)
        .param("mode", 1)
        .param("personal_switch", 1)
        .param("extend_fields", "abtags,hot_cmt,popularization")
        .param("pagesize", pagesize)
        .param("global_collection_id", global_collection_id);

    let value = api.call(spec, Freshness::PLAYLIST).await?;
    let songs = codec::find_array(&value, &["songs", "song_list", "info", "list"])
        .map(|items| Song::list_from(items))
        .unwrap_or_default();
    let has_next = codec::find_i64(&value, &["has_next"]);
    Ok(Page {
        items: songs,
        cursor: if has_next == 1 {
            Some((page + 1).to_string())
        } else {
            None
        },
        meta: Value::Null,
    })
}

/// 失效歌单相关内存缓存
pub fn invalidate_playlist_cache(api: &Api, _global_collection_id: &str) {
    api.cache().invalidate_prefix("/pubsongs/v2/get_other_list_file_nofilt");
    api.cache().invalidate_prefix("/v7/get_all_list");
}

/// 获取当前登录用户个人资料
pub async fn user_detail(api: &Api) -> Result<Value> {
    let session = api.session();
    crate::spec::ensure_logged_in(&session)?;
    let clienttime = now_millis() / 1000;
    let p = rsa_raw_encrypt_hex(
        serde_json::json!({ "token": session.token(), "clienttime": clienttime })
            .to_string()
            .as_bytes(),
        RSA_PUBLIC,
    )?
    .to_uppercase();

    let body = serde_json::json!({
        "visit_time": clienttime,
        "usertype": 1,
        "p": p,
        "userid": session.userid(),
    });
    let spec = RequestSpec::post("/v3/get_my_info")
        .body(body)
        .param("plat", 1)
        .header("x-router", "usercenter.kugou.com");
    api.call(spec, Freshness::ACCOUNT).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::testing::api;
    use crate::transport::MockTransport;

    #[tokio::test]
    async fn playlists_are_parsed_and_normalized() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v7/get_all_list",
            &serde_json::json!({"status": 1, "data": {"info": [
                {"listid": 2, "name": "我喜欢", "count": 102, "type": 0, "global_collection_id": "collection_3_2043841403_2"},
                {"listid": 55, "name": "藏起来", "count": 12, "type": 1}
            ]}}),
        );
        let api = api(mock.clone());
        let lists = playlists(&api).await.unwrap();
        assert_eq!(lists.len(), 2);
        assert!(lists[0].is_liked());
        assert_eq!(lists[0].count, 102);
        assert_eq!(lists[1].kind, 1);
        let request = mock.last_request().unwrap();
        assert_eq!(request.get_header("x-router"), Some(crate::cloud::CLOUD_ROUTER));
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["total_ver"], 979);
        assert_eq!(body["type"], 2);
        assert!(request.query.iter().any(|(k, v)| k == "userid" && v == "2043841403"));
    }

    #[tokio::test]
    async fn playlist_tracks_computes_begin_idx() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/pubsongs/v2/get_other_list_file_nofilt",
            &serde_json::json!({"status": 1, "data": {"songs": [{"name": "A", "hash": "AB"}], "has_next": 1}}),
        );
        let api = api(mock.clone());
        let page = playlist_tracks(&api, "gcid", 3, 30).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.cursor.as_deref(), Some("4"));
        let request = mock.last_request().unwrap();
        assert_eq!(request.query.iter().find(|(k, _)| k == "begin_idx").unwrap().1, "60");
        assert_eq!(
            request.query.iter().find(|(k, _)| k == "extend_fields").unwrap().1,
            "abtags,hot_cmt,popularization"
        );
    }

    #[tokio::test]
    async fn anonymous_call_to_playlists_is_rejected() {
        let mock = Arc::new(MockTransport::new());
        let api = crate::testing::anonymous_api(mock.clone());
        assert!(playlists(&api).await.is_err());
        assert_eq!(mock.count(), 0);
    }
}
