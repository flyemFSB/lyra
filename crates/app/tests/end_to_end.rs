//! 应用层集成测试：基于 MockTransport 验证端到端业务流

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use lyra_api::transport::{MockTransport, Response};
use lyra_app::{Kugou, PlayMode};

// 构造测试登录响应桩
fn login_stub(mock: &MockTransport) {
    mock.on_json(
        "/v7/login_by_verifycode",
        &serde_json::json!({
            "status": 1,
            "data": {"token": "TOKEN-ABC", "userid": 2043841403, "vip_type": 0, "vip_token": "VT"}
        }),
    );
}

// 构造测试歌单与歌曲列表响应桩
fn library_stubs(mock: &MockTransport) {
    mock.on_json(
        "/v7/get_all_list",
        &serde_json::json!({"status": 1, "data": {"info": [
            {"listid": 2, "name": "我喜欢", "count": 1, "type": 0, "global_collection_id": "gcid-2"}
        ]}}),
    );
    mock.on_json(
        "/pubsongs/v2/get_other_list_file_nofilt",
        &serde_json::json!({"status": 1, "data": {"songs": [
            {"name": "夜曲", "singername": "周杰伦", "hash": "ABC123", "album_id": "1234",
             "audio_id": 98765, "timelen": 226000, "fileid": 7}
        ], "has_next": 0}}),
    );
}

// 构造测试音质权益与播放直链响应桩
fn playback_stubs(mock: &MockTransport) {
    mock.on_json(
        "/v2/get_res_privilege/lite",
        &serde_json::json!({"status": 1, "data": [{
            "hash": "abc123", "name": "夜曲", "singername": "周杰伦", "albumname": "十一月的萧邦",
            "auth": "AUTH-TICKET",
            "trans_param": {"ogg_128_hash": "OGG128", "ogg_128_filesize": 3_000_000},
            "relate_goods": [
                {"level": 2, "quality": "128", "privilege": 10, "status": 1, "info": {"filesize": 2_000_000}},
                {"level": 5, "quality": "flac", "privilege": 0, "status": 1, "hash": "FLACHASH",
                 "info": {"filesize": 40_000_000, "extname": "flac"}}
            ]
        }]}),
    );
    mock.on_json(
        "/tracker/v5/url",
        &serde_json::json!({"status": 1, "data": {"url": [
            {"url": "http://fs.example/ap3116_us1_quflac_pa967177915.flac", "extname": "flac",
             "filesize": 40_000_000, "timelength": 226000, "hash": "FLACHASH"}
        ]}}),
    );
}

// 创建装配测试桩的应用门面与临时目录
fn app(mock: Arc<MockTransport>) -> (Kugou, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = lyra_store::open_temp_with_secrets(Box::new(lyra_store::MemorySecrets::new())).unwrap();
    let app = Kugou::with_transport(mock, store, dir.path()).unwrap();
    (app, dir)
}

#[tokio::test]
async fn full_flow_login_playlists_tracks_song_url() {
    let mock = Arc::new(MockTransport::new());
    login_stub(&mock);
    library_stubs(&mock);
    playback_stubs(&mock);
    let (app, _dir) = app(mock.clone());

    // 未登录时访问受限且不发出请求
    assert!(app.playlists().await.is_err());
    assert_eq!(mock.count(), 0, "未登录不应发请求");

    // 执行短信登录
    let account = app.login("13800138000", "123456").await.unwrap();
    assert_eq!(account.userid, 2_043_841_403);
    assert!(app.is_logged_in());

    // 验证令牌持久化成功
    let persisted = app.store().account().unwrap().expect("账号应已落盘");
    assert_eq!(persisted.token, "TOKEN-ABC");
    assert_eq!(persisted.vip_token, "VT");

    // 获取歌单列表
    let lists = app.playlists().await.unwrap();
    assert_eq!(lists.len(), 1);
    assert!(lists[0].is_liked());
    assert_eq!(lists[0].global_collection_id, "gcid-2");

    // 获取歌单歌曲列表
    let page = app.tracks(&lists[0].global_collection_id, 1).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].name, "夜曲");
    assert_eq!(page.items[0].hash, "abc123", "hash 必须归一化为小写");
    assert_eq!(page.items[0].duration_ms, 226_000);
    assert_eq!(page.items[0].fileid, 7);

    // 获取播放直链并校验音质选择
    let url = app.song_url(&page.items[0], "").await.unwrap();
    assert_eq!(url.ext, "flac");
    assert_eq!(url.ext_or_guess(), "flac");
    assert_eq!(url.duration_ms, 226_000);
    assert_eq!(
        std::path::Path::new(&url.url)
            .extension()
            .and_then(std::ffi::OsStr::to_str),
        Some("flac")
    );

    // 校验请求携带授权票据与签名密钥
    let request = mock.last_request().unwrap();
    let get = |key: &str| request.query.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    assert_eq!(get("auth").unwrap(), "AUTH-TICKET");
    assert_eq!(get("quality").unwrap(), "flac");
    assert!(get("key").is_some(), "song_url 必须带 signKey");
}

#[tokio::test]
async fn quality_falls_back_when_the_preferred_one_is_locked() {
    let mock = Arc::new(MockTransport::new());
    login_stub(&mock);
    library_stubs(&mock);
    playback_stubs(&mock);
    let (app, _dir) = app(mock.clone());
    app.login("13800138000", "123456").await.unwrap();
    let song = app.tracks("gcid-2", 1).await.unwrap().items.remove(0);

    // 请求未开放音质时回落至最佳可用音质
    let url = app.song_url(&song, "viper_tape").await.unwrap();
    assert_eq!(url.ext, "flac");
    let request = mock.last_request().unwrap();
    let quality = request.query.iter().find(|(k, _)| k == "quality").unwrap();
    assert_eq!(quality.1, "flac", "锁定的音质应回落到 flac");
}

#[tokio::test]
async fn logout_clears_the_persisted_token() {
    let mock = Arc::new(MockTransport::new());
    login_stub(&mock);
    library_stubs(&mock);
    let (app, _dir) = app(mock);
    app.login("13800138000", "123456").await.unwrap();
    assert!(app.is_logged_in());

    app.logout().unwrap();
    assert!(!app.is_logged_in());
    assert!(app.store().account().unwrap().is_none(), "登出必须清掉凭据");
    assert!(app.playlists().await.is_err());
}

#[tokio::test]
async fn upstream_business_error_surfaces_instead_of_panicking() {
    let mock = Arc::new(MockTransport::new());
    login_stub(&mock);
    mock.on_json("/v7/get_all_list", &serde_json::json!({"status": 0, "msg": "上游炸了"}));
    let (app, _dir) = app(mock);
    app.login("13800138000", "123456").await.unwrap();
    let error = app.playlists().await.unwrap_err();
    assert!(format!("{error}").contains("上游炸了"), "{error}");
}

#[tokio::test(start_paused = true)]
async fn device_registration_retries_while_upstream_returns_empty_data() {
    let mock = Arc::new(MockTransport::new());
    // 模拟上游返回空数据触发重试机制
    mock.on(
        "/risk/v2/r_register_dev",
        Response::new(200, br#"{"status":1,"error_code":0,"data":[]}"#.to_vec()),
    );
    let (app, _dir) = app(mock.clone());
    let error = app.register_device().await.unwrap_err();
    assert_eq!(mock.count(), 3, "空表应答应重试到上限");
    assert!(format!("{error}").contains("未返回 dfid"), "{error}");
}

#[tokio::test]
async fn device_identity_is_generated_once_and_reused() {
    let mock = Arc::new(MockTransport::new());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();

    let first = {
        let store =
            lyra_store::open_with_secrets(path.join("lyra.redb"), Box::new(lyra_store::MemorySecrets::new())).unwrap();
        Kugou::with_transport(mock.clone(), store, &path).unwrap()
    };
    let mid = first.session().device.mid.clone();
    let guid = first.session().device.guid.clone();
    drop(first);

    // 重启数据库验证设备身份标识保持不变
    let second = {
        let store =
            lyra_store::open_with_secrets(path.join("lyra.redb"), Box::new(lyra_store::MemorySecrets::new())).unwrap();
        Kugou::with_transport(mock, store, &path).unwrap()
    };
    assert_eq!(second.session().device.mid, mid);
    assert_eq!(second.session().device.guid, guid);
}

// 创建预置 dfid 的应用门面，跳过设备注册阶段
fn registered_app(mock: Arc<MockTransport>) -> (Kugou, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = lyra_store::open_temp_with_secrets(Box::new(lyra_store::MemorySecrets::new())).unwrap();
    let device = lyra_proto::Device {
        dfid: "DfidTest".to_owned(),
        ..lyra_proto::Device::generate()
    };
    store.set_device(&device).unwrap();
    let app = Kugou::with_transport(mock, store, dir.path()).unwrap();
    (app, dir)
}

// 校验请求体中的外层手机号掩码按裁剪后的号码生成
fn assert_body_mobile(mock: &MockTransport, path: &str) {
    let request = mock
        .requests()
        .into_iter()
        .rev()
        .find(|request| request.url.contains(path))
        .expect("应已发出该请求");
    let body: serde_json::Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["mobile"], "13*****0", "外层掩码必须按 trim 后的号码计算");
}

#[tokio::test]
async fn send_code_trims_pasted_whitespace_before_encrypting_the_number() {
    let mock = Arc::new(MockTransport::new());
    mock.on_json(
        "/v8/send_mobile_code",
        &serde_json::json!({"status": 1, "data": {"count": 8}}),
    );
    let (app, _dir) = registered_app(mock.clone());

    assert_eq!(app.send_code("  13800138000 ").await.unwrap(), 8);
    assert_body_mobile(&mock, "/v8/send_mobile_code");
}

#[tokio::test]
async fn login_trims_pasted_whitespace_before_encrypting_the_credentials() {
    let mock = Arc::new(MockTransport::new());
    login_stub(&mock);
    let (app, _dir) = registered_app(mock.clone());

    app.login(" 13800138000 ", " 123456 ").await.unwrap();
    assert_body_mobile(&mock, "/v7/login_by_verifycode");
}

// 验证播放模式支持跨启动持久化
#[tokio::test]
async fn play_mode_survives_a_restart() {
    let mock = Arc::new(MockTransport::new());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lyra.redb");

    {
        let store = lyra_store::open_with_secrets(&path, Box::new(lyra_store::MemorySecrets::new())).unwrap();
        let app = Kugou::with_transport(mock.clone(), store, dir.path()).unwrap();
        assert_eq!(app.play_mode(), PlayMode::ListLoop, "默认列表循环");
        app.set_settings(&lyra_store::Settings {
            play_mode: PlayMode::Shuffle,
            ..Default::default()
        })
        .unwrap();
    }

    // 重启后保持上次配置的播放模式
    let reopened = Kugou::with_transport(
        mock,
        lyra_store::open_with_secrets(&path, Box::new(lyra_store::MemorySecrets::new())).unwrap(),
        dir.path(),
    )
    .unwrap();
    assert_eq!(reopened.play_mode(), PlayMode::Shuffle);
}

// 模拟无版权歌曲与可选重传版响应
fn no_copyright_stubs(mock: &MockTransport, with_replacement: bool) {
    let mut data = vec![serde_json::json!({
        "hash": "f78de2e4", "name": "阴天", "singername": "汪苏泷、莫文蔚",
        "privilege": 5, "status": 0,
        "_msg": "Deny: the current behavior of the audio is forbidden.",
        "trans_param": {"cpy_grade": 75, "cpy_map": "2"},
        "relate_goods": [{"level": 2, "quality": "128", "privilege": 5, "status": 0}]
    })];
    if with_replacement {
        // 模拟同名 UGC 重传条目（免费且时长相近）
        data.push(serde_json::json!({
            "hash": "313c11d5", "name": "阴天", "singername": "汪苏泷、莫文蔚", "privilege": 0,
            "relate_goods": [{"level": 2, "quality": "128", "privilege": 0, "status": 1,
                                "info": {"extname": "mp3", "filesize": 3_000_000}}]
        }));
    }
    mock.on_json(
        "/v2/get_res_privilege/lite",
        &serde_json::json!({"status": 1, "data": data}),
    );
    let lists = if with_replacement {
        serde_json::json!([{
            "FileHash": "313C11D5", "MixSongID": 974_503_310,
            "SongName": "汪苏泷、莫文蔚《阴天 (2026明日世界巡回演唱会北京站)》",
            "SingerName": "汪苏泷、莫文蔚", "Duration": 230_000, "Privilege": 0
        }])
    } else {
        serde_json::json!([])
    };
    mock.on_json(
        "/v2/search/song",
        &serde_json::json!({"status": 1, "data": {"lists": lists}}),
    );
}

fn denied_song() -> lyra_api::model::Song {
    lyra_api::model::Song {
        name: "阴天 (2026明日世界巡回演唱会北京站)".to_owned(),
        singer: "汪苏泷、莫文蔚".to_owned(),
        hash: "f78de2e4".to_owned(),
        duration_ms: 225_000,
        privilege: 5,
        ..lyra_api::model::Song::default()
    }
}

#[tokio::test]
async fn no_copyright_track_is_swapped_for_a_playable_reupload() {
    let mock = Arc::new(MockTransport::new());
    no_copyright_stubs(&mock, true);
    mock.on_json(
        "/tracker/v5/url",
        &serde_json::json!({"status": 1, "data": {"url": [
            {"url": "http://fs.example/ap3116_qu128.ogg", "extname": "ogg", "hash": "313C11D5"}
        ]}}),
    );
    let (app, _dir) = app(mock.clone());

    let url = app.song_url(&denied_song(), "128").await.unwrap();
    assert_eq!(url.ext, "ogg");

    // 取址应使用换源后的新哈希
    let tracker = mock
        .requests()
        .into_iter()
        .find(|request| request.url.contains("/tracker/v5/url"))
        .expect("应发出取址请求");
    let hash = tracker
        .query
        .iter()
        .find(|(key, _)| key == "hash")
        .map(|(_, value)| value.clone());
    assert_eq!(hash.as_deref(), Some("313c11d5"), "必须换源后再取址");
}

#[tokio::test]
async fn no_copyright_without_a_replacement_is_reported_as_such() {
    let mock = Arc::new(MockTransport::new());
    no_copyright_stubs(&mock, false);
    let (app, _dir) = app(mock.clone());

    let error = app.song_url(&denied_song(), "128").await.unwrap_err();
    assert!(matches!(error, lyra_app::Error::NoCopyright), "{error:?}");
    assert_eq!(
        format!("{error}"),
        "这首歌在概念版没有版权（已下架）",
        "错误文案要能看懂，不能再是「上游未返回播放地址」"
    );
    assert!(
        !mock
            .requests()
            .iter()
            .any(|request| request.url.contains("/tracker/v5/url")),
        "既然上游说没版权，就不该再去取址（白白多一次必失败请求）"
    );
}

#[tokio::test]
async fn trial_only_tracks_expose_the_preview_clip_length() {
    let mock = Arc::new(MockTransport::new());
    mock.on_json(
        "/v2/get_res_privilege/lite",
        &serde_json::json!({"status": 1, "data": [{
            "hash": "abc123", "name": "Renegades", "privilege": 10,
            "relate_goods": [{"level": 2, "quality": "128", "privilege": 10, "status": 0,
                               "info": {"extname": "mp3", "filesize": 3_000_000}}]
        }]}),
    );
    mock.on_json(
        "/tracker/v5/url",
        &serde_json::json!({"status": 2, "fail_process": ["pkg", "buy"],
            "hash_offset": {"start_ms": 0, "end_ms": 60000, "offset_hash": "OFFSET"}}),
    );
    mock.on_json(
        "IsFreePart=1",
        &serde_json::json!({"status": 1, "extName": "mp3",
            "url": ["http://fs.example/p_0_960157/ap3116_qu128.mp3"],
            "hash_offset": {"end_ms": 60000, "clip_hash": "CLIPHASH"}}),
    );
    let (app, _dir) = app(mock.clone());
    let song = lyra_api::model::Song {
        name: "Renegades".to_owned(),
        hash: "abc123".to_owned(),
        ..lyra_api::model::Song::default()
    };

    let url = app.song_url(&song, "").await.unwrap();
    assert_eq!(url.preview_ms, 60_000, "只授权试听时要如实告知时长");
    assert_eq!(url.duration_ms, 60_000, "进度条必须按片段时长走");
    assert_eq!(
        url.blob_key(&song.hash),
        "cliphash_preview",
        "试听片段不能污染完整曲缓存"
    );
}
