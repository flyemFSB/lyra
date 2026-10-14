//! 歌单歌曲增删操作端点，支持批量加歌与删歌。

use crate::cache::Freshness;
use crate::client::Api;
use crate::error::Result;
use crate::model::Song;
use crate::spec::RequestSpec;
use crate::spec::{ensure_logged_in, now_secs};

/// 向指定歌单添加一批歌曲。
pub async fn add_songs(api: &Api, listid: &str, songs: &[Song]) -> Result<()> {
    let session = api.session();
    ensure_logged_in(&session)?;
    let userid = session.userid();
    let token = session.token().to_owned();

    let resources: Vec<serde_json::Value> = songs
        .iter()
        .map(|song| {
            serde_json::json!({
                "number": 1,
                "name": song.name,
                "hash": song.hash,
                "size": 0,
                "sort": 0,
                "timelen": 0,
                "bitrate": 0,
                "album_id": song.album_id,
                "mixsongid": song.mixsongid,
            })
        })
        .collect();

    let body = serde_json::json!({
        "userid": userid,
        "token": token,
        "listid": listid,
        "list_ver": 0,
        "type": 0,
        "slow_upload": 1,
        "scene": "false;null",
        "data": resources,
    });

    let spec = RequestSpec::post("/cloudlist.service/v6/add_song")
        .body(body)
        .param("last_time", now_secs())
        .param("last_area", "gztx")
        .param("userid", userid)
        .param("token", token);

    api.call(spec, Freshness::SideEffect).await?;
    crate::endpoint::library::invalidate_playlist_cache(api, listid);
    Ok(())
}

/// 根据 fileid 列表从指定歌单删除歌曲。
pub async fn delete_songs(api: &Api, listid: &str, fileids: &[i64]) -> Result<()> {
    let session = api.session();
    ensure_logged_in(&session)?;

    let resources: Vec<serde_json::Value> = fileids
        .iter()
        .map(|fileid| serde_json::json!({ "fileid": fileid }))
        .collect();

    let body = serde_json::json!({
        "listid": listid,
        "userid": session.userid(),
        "data": resources,
        "type": 0,
        "token": session.token(),
        "list_ver": 0,
    });

    let spec = RequestSpec::post("/v4/delete_songs")
        .body(body)
        .header("x-router", crate::cloud::CLOUD_ROUTER);

    api.call(spec, Freshness::SideEffect).await?;
    crate::endpoint::library::invalidate_playlist_cache(api, listid);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::testing::api;
    use crate::transport::MockTransport;

    #[tokio::test]
    async fn add_song_matches_the_reference_body() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/cloudlist.service/v6/add_song", &serde_json::json!({"status": 1}));
        let api = api(mock.clone());
        let song = Song {
            name: "夜曲".to_owned(),
            hash: "abc".to_owned(),
            album_id: "1234".to_owned(),
            mixsongid: 424242,
            ..Song::default()
        };
        add_songs(&api, "2", &[song]).await.unwrap();

        let request = mock.last_request().unwrap();
        let body: serde_json::Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["listid"], "2");
        assert_eq!(body["slow_upload"], 1);
        assert_eq!(body["scene"], "false;null");
        assert_eq!(body["data"][0]["number"], 1);
        assert_eq!(body["data"][0]["hash"], "abc");
        assert_eq!(body["data"][0]["mixsongid"], 424242);
        assert_eq!(body["data"][0]["timelen"], 0);
        assert_eq!(request.query.iter().find(|(k, _)| k == "last_area").unwrap().1, "gztx");
        assert!(request.query.iter().all(|(k, _)| k != "p"), "不该走双加密通道");
    }

    #[tokio::test]
    async fn delete_song_sends_fileids_and_routes_to_cloudlist() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v4/delete_songs", &serde_json::json!({"status": 1}));
        let api = api(mock.clone());
        delete_songs(&api, "2", &[11, 22]).await.unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.get_header("x-router"), Some(crate::cloud::CLOUD_ROUTER));
        let body: serde_json::Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["data"][0]["fileid"], 11);
        assert_eq!(body["data"][1]["fileid"], 22);
    }

    #[tokio::test]
    async fn side_effect_endpoints_are_never_cached() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v4/delete_songs", &serde_json::json!({"status": 1}));
        let api = api(mock.clone());
        delete_songs(&api, "2", &[11]).await.unwrap();
        delete_songs(&api, "2", &[11]).await.unwrap();
        assert_eq!(mock.count(), 2, "删歌不能被缓存吞掉");
    }
}
