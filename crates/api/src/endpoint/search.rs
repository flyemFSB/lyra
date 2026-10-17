//! 搜索业务端点，支持歌曲关键词检索。

use lyra_proto::consts::SEARCH_CLIENTVER;

use crate::cache::Freshness;
use crate::client::Api;
use crate::codec;
use crate::error::{Error, Result};
use crate::model::Song;
use crate::spec::RequestSpec;

/// 搜索网关路由主机
const SEARCH_ROUTER: &str = "complexsearch.kugou.com";

/// 依据关键词分页搜索单曲
pub async fn songs(api: &Api, keyword: &str, page: u32, pagesize: u32, tag: Option<&str>) -> Result<Vec<Song>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Err(Error::Decode("搜索关键词为空".to_owned()));
    }
    let mut spec = RequestSpec::get("/v2/search/song")
        .param("keyword", keyword)
        .param("page", if page == 0 { 1 } else { page })
        .param("pagesize", if pagesize == 0 { 30 } else { pagesize })
        .param("platform", "AndroidFilter")
        .param("iscorrection", 1)
        .param("privilegefilter", 0)
        .param("area_code", 1)
        .param("dopicfull", 1)
        // 概念版专享搜索客户端版本号
        .param("clientver", SEARCH_CLIENTVER)
        .header("x-router", SEARCH_ROUTER);
    if let Some(tag) = tag {
        spec = spec.param("tag", tag);
    }

    let value = api.call(spec, Freshness::CATALOG).await?;
    Ok(codec::find_array(&value, &["lists", "list", "songs", "info"])
        .map(|items| Song::list_from(items))
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::testing::anonymous_api as api;
    use crate::transport::MockTransport;

    #[tokio::test]
    async fn song_search_forces_clientver_201_and_parses_capitalised_fields() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v2/search/song",
            &serde_json::json!({"status": 1, "data": {"total": 1, "lists": [{
                "FileHash": "ABC123DEF", "SongName": "夜曲", "SingerName": "周杰伦",
                "AlbumID": "1234", "AlbumName": "十一月的萧邦", "Duration": 226000,
                "EMixSongID": "98765", "MixSongID": 424242, "FileID": 7,
                "Image": "http://imge.kugou.com/stdmusic/{size}/x.jpg"
            }]}}),
        );
        let api = api(mock.clone());
        let songs = songs(&api, "夜曲", 1, 30, None).await.unwrap();
        assert_eq!(songs.len(), 1);
        let song = &songs[0];
        assert_eq!(song.name, "夜曲");
        assert_eq!(song.singer, "周杰伦");
        assert_eq!(song.hash, "abc123def", "FileHash 必须被识别并归一化为小写");
        assert_eq!(song.duration_ms, 226_000);
        assert_eq!(song.album_audio_id, "98765");
        assert_eq!(song.mixsongid, 424242);
        assert_eq!(song.cover_at(240), "http://imge.kugou.com/stdmusic/240/x.jpg");

        let request = mock.last_request().unwrap();
        assert_eq!(request.url, "https://gateway.kugou.com/v2/search/song");
        assert_eq!(request.get_header("x-router"), Some(SEARCH_ROUTER));
        let get = |k: &str| request.query.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        assert_eq!(get("clientver").unwrap(), "201", "概念版必须用 201");
        assert_eq!(get("platform").unwrap(), "AndroidFilter");
        assert_eq!(get("dopicfull").unwrap(), "1");
        assert!(get("signature").is_some());
    }

    #[tokio::test]
    async fn empty_keyword_is_rejected_without_a_request() {
        let mock = Arc::new(MockTransport::new());
        let api = api(mock.clone());
        assert!(songs(&api, "   ", 1, 30, None).await.is_err());
        assert_eq!(mock.count(), 0);
    }

    #[tokio::test]
    async fn highlight_tag_is_passed_through() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v2/search/song",
            &serde_json::json!({"status": 1, "data": {"lists": []}}),
        );
        let api = api(mock.clone());
        songs(&api, "周杰伦", 1, 30, Some("em")).await.unwrap();
        let request = mock.last_request().unwrap();
        assert_eq!(request.query.iter().find(|(k, _)| k == "tag").unwrap().1, "em");
    }
}
