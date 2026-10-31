//! 搜索业务端点，支持歌曲关键词检索。

use lyra_proto::consts::SEARCH_CLIENTVER;

use crate::cache::Freshness;
use crate::client::Api;
use crate::codec;
use crate::error::{Error, Result};
use crate::model::{Album, Artist, Page, Playlist, Song};
use crate::spec::RequestSpec;
use serde_json::Value;

/// 搜索网关路由主机
const SEARCH_ROUTER: &str = "complexsearch.kugou.com";

/// 专辑与歌手曲目所在的移动端路由
const MOBILE_ROUTER: &str = "mobilecdn.kugou.com";

/// 依据关键词分页搜索单曲
pub async fn songs(api: &Api, keyword: &str, page: u32, pagesize: u32, tag: Option<&str>) -> Result<Page<Song>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Err(Error::Decode("搜索关键词为空".to_owned()));
    }
    let page = if page == 0 { 1 } else { page };
    let pagesize = if pagesize == 0 { 30 } else { pagesize };
    let mut spec = RequestSpec::get("/v2/search/song")
        .param("keyword", keyword)
        .param("page", page)
        .param("pagesize", pagesize)
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
    let hits = codec::find_array(&value, &["lists", "list", "songs", "info"])
        .map(|items| Song::list_from(items))
        .unwrap_or_default();
    // total 是命中的全量条数；拿不到时退化为「满页即可能还有」
    let total = codec::pick_i64_paths(&value, &[&["data", "total"], &["total"]]);
    Ok(paged(hits, page, pagesize, total))
}

/// 依据关键词分页搜索专辑
pub async fn albums(api: &Api, keyword: &str, page: u32, pagesize: u32) -> Result<Page<Album>> {
    let (value, page, pagesize) = kind_search(api, "album", keyword, page, pagesize).await?;
    let items = codec::find_array(&value, &["lists", "list"])
        .map(|items| items.iter().filter_map(Album::from_value).collect())
        .unwrap_or_default();
    Ok(paged(items, page, pagesize, kind_total(&value)))
}

/// 依据关键词分页搜索歌手
pub async fn artists(api: &Api, keyword: &str, page: u32, pagesize: u32) -> Result<Page<Artist>> {
    let (value, page, pagesize) = kind_search(api, "author", keyword, page, pagesize).await?;
    let items = codec::find_array(&value, &["lists", "list"])
        .map(|items| items.iter().filter_map(Artist::from_value).collect())
        .unwrap_or_default();
    Ok(paged(items, page, pagesize, kind_total(&value)))
}

/// 依据关键词分页搜索歌单
pub async fn playlists(api: &Api, keyword: &str, page: u32, pagesize: u32) -> Result<Page<Playlist>> {
    let (value, page, pagesize) = kind_search(api, "special", keyword, page, pagesize).await?;
    let items = codec::find_array(&value, &["lists", "list"])
        .map(|items| items.iter().filter_map(Playlist::from_value).collect())
        .unwrap_or_default();
    Ok(paged(items, page, pagesize, kind_total(&value)))
}

/// 分页获取专辑曲目
///
/// 专辑详情没有独立的「全部曲目」端点：只能按页取，由上层循环拼齐。
pub async fn album_tracks(api: &Api, album_id: &str, page: u32, pagesize: u32) -> Result<Vec<Song>> {
    let spec = RequestSpec::get("/api/v3/album/song")
        .header("x-router", MOBILE_ROUTER)
        .param("albumid", album_id)
        .param("page", if page == 0 { 1 } else { page })
        .param("pagesize", if pagesize == 0 { 30 } else { pagesize });
    let value = api.call(spec, Freshness::CATALOG).await?;
    Ok(codec::find_array(&value, &["info", "songs", "list"])
        .map(|items| Song::list_from(items))
        .unwrap_or_default())
}

/// 分页获取歌手曲目（按热度排序）
pub async fn artist_tracks(api: &Api, artist_id: &str, page: u32, pagesize: u32) -> Result<Vec<Song>> {
    let spec = RequestSpec::get("/api/v3/singer/song")
        .header("x-router", MOBILE_ROUTER)
        .param("singerid", artist_id)
        .param("page", if page == 0 { 1 } else { page })
        .param("pagesize", if pagesize == 0 { 30 } else { pagesize });
    let value = api.call(spec, Freshness::CATALOG).await?;
    Ok(codec::find_array(&value, &["info", "songs", "list"])
        .map(|items| Song::list_from(items))
        .unwrap_or_default())
}

/// 专辑/歌手/歌单搜索共用同一套 v1 端点与参数
async fn kind_search(api: &Api, kind: &str, keyword: &str, page: u32, pagesize: u32) -> Result<(Value, u32, u32)> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Err(Error::Decode("搜索关键词为空".to_owned()));
    }
    let page = if page == 0 { 1 } else { page };
    let pagesize = if pagesize == 0 { 30 } else { pagesize };
    let spec = RequestSpec::get(format!("/v1/search/{kind}"))
        .param("keyword", keyword)
        .param("page", page)
        .param("pagesize", pagesize)
        .param("platform", "AndroidFilter")
        .param("iscorrection", 1)
        .param("area_code", 1)
        .param("clientver", SEARCH_CLIENTVER)
        .header("x-router", SEARCH_ROUTER);
    let value = api.call(spec, Freshness::CATALOG).await?;
    Ok((value, page, pagesize))
}

/// 读取搜索结果的命中总数
fn kind_total(value: &Value) -> i64 {
    codec::pick_i64_paths(value, &[&["data", "total"], &["total"]])
}

/// 按页码与命中总数计算下一页游标
fn paged<T>(items: Vec<T>, page: u32, pagesize: u32, total: i64) -> Page<T> {
    let loaded = u64::from(page.saturating_sub(1)) * u64::from(pagesize) + items.len() as u64;
    let has_next = if total > 0 {
        loaded < u64::try_from(total).unwrap_or(u64::MAX)
    } else {
        items.len() == pagesize as usize && !items.is_empty()
    };
    Page {
        items,
        cursor: if has_next {
            Some(page.saturating_add(1).to_string())
        } else {
            None
        },
        meta: Value::Null,
    }
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
        let page = songs(&api, "夜曲", 1, 30, None).await.unwrap();
        assert_eq!(page.items.len(), 1);
        let song = &page.items[0];
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
    async fn pagination_follows_the_reported_total() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v2/search/song",
            &serde_json::json!({"status": 1, "data": {"total": 75, "lists": [{"FileHash": "AB", "SongName": "x"}]}}),
        );
        let api = api(mock.clone());
        let first = songs(&api, "夜曲", 1, 30, None).await.unwrap();
        assert_eq!(first.cursor.as_deref(), Some("2"), "1 < 75，还有下一页");

        let last = Arc::new(MockTransport::new());
        last.on_json(
            "/v2/search/song",
            &serde_json::json!({"status": 1, "data": {"total": 61, "lists": [{"FileHash": "AB", "SongName": "x"}]}}),
        );
        let api = crate::testing::anonymous_api(last);
        assert!(
            songs(&api, "夜曲", 3, 30, None).await.unwrap().cursor.is_none(),
            "60 + 1 >= 61，无更多"
        );
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

    #[tokio::test]
    async fn kind_searches_hit_the_v1_endpoints_and_strip_highlight_tags() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v1/search/album",
            &serde_json::json!({"status": 1, "data": {"total": 1, "lists": [{
                "albumid": 958909, "albumname": "十一月的萧邦", "singer": "周杰伦",
                "songcount": 12, "publish_time": "2005-11-01",
                "img": "http://imge.kugou.com/stdmusic/240/x.jpg"
            }]}}),
        );
        let api = api(mock.clone());
        let page = albums(&api, "夜曲", 1, 30).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].album_id, "958909");
        assert_eq!(page.items[0].song_count, 12);
        assert_eq!(page.items[0].cover_at(400), "http://imge.kugou.com/stdmusic/240/x.jpg");
        let request = mock.last_request().unwrap();
        assert_eq!(request.get_header("x-router"), Some(SEARCH_ROUTER));
        assert_eq!(request.query.iter().find(|(k, _)| k == "clientver").unwrap().1, "201");

        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v1/search/author",
            &serde_json::json!({"status": 1, "data": {"total": 1, "lists": [{
                "AuthorId": 3520, "AuthorName": "<em>周杰伦</em>", "AudioCount": 1856,
                "AlbumCount": 49, "FansNum": 25860367,
                "Avatar": "http://singerimg.kugou.com/uploadpic/softhead/240/x.jpg"
            }]}}),
        );
        let api = crate::testing::anonymous_api(mock);
        let page = artists(&api, "周杰伦", 1, 30).await.unwrap();
        assert_eq!(page.items[0].name, "周杰伦", "<em> 高亮标签必须去掉");
        assert_eq!(page.items[0].song_count, 1856);
        assert_eq!(page.items[0].album_count, 49);

        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v1/search/special",
            &serde_json::json!({"status": 1, "data": {"total": 1, "lists": [{
                "specialid": 3435104, "specialname": "抖音歌<em>曲</em>", "song_count": 146,
                "gid": "collection_3_918745899_1382_0", "img": "http://c1.kgimg.com/custom/150/x.jpg"
            }]}}),
        );
        let api = crate::testing::anonymous_api(mock);
        let page = playlists(&api, "夜曲", 1, 30).await.unwrap();
        assert_eq!(page.items[0].name, "抖音歌曲");
        assert_eq!(page.items[0].listid, "3435104");
        assert_eq!(page.items[0].global_collection_id, "collection_3_918745899_1382_0");
        assert_eq!(page.items[0].count, 146);
    }

    #[tokio::test]
    async fn album_and_artist_tracks_go_through_the_mobile_router() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/api/v3/album/song",
            &serde_json::json!({"status": 1, "data": {"total": 12, "info": [{
                "hash": "16C8AB298231370293D16BCF9E5FF9B6", "filename": "周杰伦 - 夜曲",
                "album_id": "958909", "album_audio_id": 32029511, "duration": 226
            }]}}),
        );
        let api = api(mock.clone());
        let items = album_tracks(&api, "958909", 1, 100).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "夜曲");
        assert_eq!(items[0].singer, "周杰伦");
        assert_eq!(items[0].duration_ms, 226_000);
        let request = mock.last_request().unwrap();
        assert_eq!(request.get_header("x-router"), Some(MOBILE_ROUTER));
        assert_eq!(request.query.iter().find(|(k, _)| k == "albumid").unwrap().1, "958909");

        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/api/v3/singer/song",
            &serde_json::json!({"status": 1, "data": {"total": 1856, "info": [{
                "hash": "B3A52A7A958BF0AED0EBFBA2E9A818B7", "filename": "周杰伦 - 晴天",
                "album_id": "966846", "album_audio_id": 32100650, "duration": 269
            }]}}),
        );
        let api = crate::testing::anonymous_api(mock.clone());
        let items = artist_tracks(&api, "3520", 1, 50).await.unwrap();
        assert_eq!(items[0].name, "晴天");
        assert_eq!(
            mock.last_request()
                .unwrap()
                .query
                .iter()
                .find(|(k, _)| k == "singerid")
                .unwrap()
                .1,
            "3520"
        );
    }
}
