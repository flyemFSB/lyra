//! 电台与分类目录端点，支持获取分类列表、推荐频道及频道歌曲。

use lyra_proto::consts::{APPID, CLIENTVER};
use lyra_proto::sign_params_key;

use crate::cache::Freshness;
use crate::client::Api;
use crate::codec;
use crate::error::Result;
use crate::model::{Song, Station, StationClass};
use crate::spec::RequestSpec;
use crate::spec::now_millis;

const FM_ROUTER: &str = "fm.service.kugou.com";

/// 获取电台全部分类与各分类下的频道。
pub async fn fm_classes(api: &Api) -> Result<Vec<StationClass>> {
    let session = api.session();
    let date_time = now_millis();
    let body = serde_json::json!({
        "kguid": session.userid(),
        "clienttime": date_time,
        "mid": session.device.mid,
        "platform": "android",
        "clientver": CLIENTVER,
        "uid": session.userid(),
        "get_tracker": 1,
        "key": sign_params_key(date_time, APPID, CLIENTVER),
        "appid": APPID,
    });
    let spec = RequestSpec::post("/v1/class_fm_song")
        .body(body)
        .header("x-router", FM_ROUTER);
    let value = api.call(spec, Freshness::CATALOG).await?;
    Ok(codec::find_array(&value, &["class_list", "classlist", "list"])
        .map(|items| items.iter().filter_map(StationClass::from_value).collect())
        .unwrap_or_default())
}

/// 获取系统推荐电台频道列表。
pub async fn fm_recommend(api: &Api) -> Result<Vec<Station>> {
    let session = api.session();
    let date_time = now_millis();
    let body = serde_json::json!({
        "appid": APPID,
        "clientver": CLIENTVER,
        "clienttime": date_time,
        "mid": session.device.mid,
        "key": sign_params_key(date_time, APPID, CLIENTVER),
        "rcmdsongcount": 1,
        "level": 0,
        "area_code": 1,
        "get_tracker": 1,
        "uid": 0,
    });
    let spec = RequestSpec::post("/v1/rcmd_list")
        .body(body)
        .header("x-router", FM_ROUTER);
    let value = api.call(spec, Freshness::CATALOG).await?;
    Ok(codec::find_array(&value, &["fm_list", "fmlist", "list", "data"])
        .map(|items| items.iter().filter_map(Station::from_value).collect())
        .unwrap_or_default())
}

/// 分页获取指定电台频道下的歌曲列表。
pub async fn fm_songs(api: &Api, fmid: &str, fmtype: u32, offset: i64, size: u32) -> Result<Vec<Song>> {
    let session = api.session();
    let date_time = now_millis();
    let entries: Vec<serde_json::Value> = fmid
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            serde_json::json!({
                "fmid": part.trim(),
                "fmtype": if fmtype == 0 { 2 } else { fmtype },
                "offset": if offset < 0 { -1 } else { offset },
                "size": if size == 0 { 20 } else { size },
                "singername": "",
            })
        })
        .collect();

    let body = serde_json::json!({
        "appid": APPID,
        "area_code": 1,
        "clienttime": date_time,
        "clientver": CLIENTVER,
        "data": entries,
        "get_tracker": 1,
        "key": sign_params_key(date_time, APPID, CLIENTVER),
        "mid": session.device.mid,
        "uid": session.userid(),
    });
    let spec = RequestSpec::post("/v1/app_song_list_offset")
        .body(body)
        .header("x-router", FM_ROUTER)
        .header("Content-Type", "application/json");
    let value = api.call(spec, Freshness::CATALOG).await?;
    // 响应是 `{"data":[{"fmid":…,"songs":[…]}]}`：**不能**把顶层 `data` 当候选键，
    // 它本身就是个数组且在 BFS 里先被命中，于是每首歌都解析失败 → 永远 0 首。
    Ok(codec::find_array(&value, &["songs", "song_list"])
        .map(|items| Song::list_from(items))
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::testing::api;
    use crate::transport::MockTransport;

    #[tokio::test]
    async fn classes_are_parsed_from_the_two_stage_shape() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v1/class_fm_song",
            &serde_json::json!({"status": 1, "data": {"class_list": [
                {"classid": 1, "classname": "伤感", "class_count": 12, "fmlist": [{"fmid": 100, "fmname": "夜听"}]},
                {"classid": 2, "classname": "摇滚", "class_count": 8, "fmlist": []}
            ]}}),
        );
        let api = api(mock.clone());
        let classes = fm_classes(&api).await.unwrap();
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0].classid, "1");
        assert_eq!(classes[0].name, "伤感");
        assert_eq!(classes[0].count, 12);
        assert_eq!(classes[0].stations.len(), 1);
        assert_eq!(classes[0].stations[0].fmid, "100");
        let request = mock.last_request().unwrap();
        assert_eq!(request.method, crate::transport::Method::Post);
        assert_eq!(request.get_header("x-router"), Some(FM_ROUTER));
    }

    #[tokio::test]
    async fn recommend_returns_stations() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v1/rcmd_list",
            &serde_json::json!({"status": 1, "data": {"fm_list": [
                {"fmid": 7, "fmname": "独立", "classid": 3, "imgurl": "http://i/1.jpg", "heat": 99}
            ]}}),
        );
        let api = api(mock.clone());
        let stations = fm_recommend(&api).await.unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].name, "独立");
        assert_eq!(stations[0].heat, 99);
        let body: serde_json::Value =
            serde_json::from_slice(mock.last_request().unwrap().body.as_deref().unwrap()).unwrap();
        assert_eq!(body["uid"], 0, "fm_recommend 的 uid 固定为 0");
        assert_eq!(body["level"], 0);
    }

    #[tokio::test]
    async fn songs_builds_the_fm_data_array() {
        let mock = Arc::new(MockTransport::new());
        // 上游真实形态：`data` 是数组，歌曲在它的元素的 `songs` 里。
        mock.on_json(
            "/v1/app_song_list_offset",
            &serde_json::json!({"status": 1, "data": [
                {"fmid": 1048, "fmtype": 2, "offset": 20, "size": 20, "songs": [{"name": "A", "hash": "AA"}]}
            ]}),
        );
        let api = api(mock.clone());
        let songs = fm_songs(&api, "100,200", 2, 20, 20).await.unwrap();
        assert_eq!(songs.len(), 1, "必须取到内层 songs，而不是顶层 data");
        assert_eq!(songs[0].hash, "aa");
        let body: serde_json::Value =
            serde_json::from_slice(mock.last_request().unwrap().body.as_deref().unwrap()).unwrap();
        assert_eq!(body["data"][0]["fmid"], "100");
        assert_eq!(body["data"][1]["fmid"], "200");
        assert_eq!(body["data"][0]["offset"], 20);
        assert_eq!(body["data"][0]["fmtype"], 2);
        assert_eq!(body["data"][0]["size"], 20);
    }

    #[tokio::test]
    async fn songs_defaults_offset_to_minus_one() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v1/app_song_list_offset",
            &serde_json::json!({"status": 1, "data": [{"fmid": 100, "songs": []}]}),
        );
        let api = api(mock.clone());
        fm_songs(&api, "100", 0, -5, 0).await.unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(mock.last_request().unwrap().body.as_deref().unwrap()).unwrap();
        assert_eq!(body["data"][0]["offset"], -1);
        assert_eq!(body["data"][0]["fmtype"], 2);
        assert_eq!(body["data"][0]["size"], 20);
    }
}
