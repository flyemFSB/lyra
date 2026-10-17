//! 应用层集成测试：基于 MockTransport 验证端到端业务流。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use lyra_api::transport::{MockTransport, Response};
use lyra_app::{Kugou, PlayMode};

/// 构造测试登录响应桩。
fn login_stub(mock: &MockTransport) {
    mock.on_json(
        "/v7/login_by_verifycode",
        &serde_json::json!({
            "status": 1,
            "data": {"token": "TOKEN-ABC", "userid": 2043841403, "vip_type": 0, "vip_token": "VT"}
        }),
    );
}

/// 构造测试歌单与歌曲列表响应桩。
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

/// 构造测试音质权益与播放直链响应桩。
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

/// 创建装配测试桩的应用门面与临时目录。
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

    // 未登录时访问受限且不触发网络请求。
    assert!(app.playlists().await.is_err());
    assert_eq!(mock.count(), 0, "未登录不应发请求");

    // 执行短信登录。
    let account = app.login("13800138000", "123456").await.unwrap();
    assert_eq!(account.userid, 2_043_841_403);
    assert!(app.is_logged_in());

    // 验证敏感令牌已安全持久化并可往返读取。
    let persisted = app.store().account().unwrap().expect("账号应已落盘");
    assert_eq!(persisted.token, "TOKEN-ABC");
    assert_eq!(persisted.vip_token, "VT");

    // 获取歌单列表。
    let lists = app.playlists().await.unwrap();
    assert_eq!(lists.len(), 1);
    assert!(lists[0].is_liked());
    assert_eq!(lists[0].global_collection_id, "gcid-2");

    // 获取歌单歌曲列表。
    let page = app.tracks(&lists[0].global_collection_id, 1).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].name, "夜曲");
    assert_eq!(page.items[0].hash, "abc123", "hash 必须归一化为小写");
    assert_eq!(page.items[0].duration_ms, 226_000);
    assert_eq!(page.items[0].fileid, 7);

    // 获取播放直链并校验音质选择逻辑。
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

    // 校验请求携带授权票据与签名密钥。
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

    // 请求未开放音质时应自动回落至最佳可用音质。
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
    // 模拟上游返回空数据触发重试机制。
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

    // 重启数据库验证设备身份标识保持不变。
    let second = {
        let store =
            lyra_store::open_with_secrets(path.join("lyra.redb"), Box::new(lyra_store::MemorySecrets::new())).unwrap();
        Kugou::with_transport(mock, store, &path).unwrap()
    };
    assert_eq!(second.session().device.mid, mid);
    assert_eq!(second.session().device.guid, guid);
}

/// 创建已带 dfid 的应用门面：跳过前置的设备注册请求，让 `last_request` 就是待断言的那一次。
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

/// 断言发往 `path` 的请求体中，外层号码掩码按 trim 后的号码计算。
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

/// 播放模式必须跨启动沿用（旧实现每次开机都回到列表循环）。
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

    // 重开客户端（同一数据目录）：模式必须是上次选的随机播放。
    let reopened = Kugou::with_transport(
        mock,
        lyra_store::open_with_secrets(&path, Box::new(lyra_store::MemorySecrets::new())).unwrap(),
        dir.path(),
    )
    .unwrap();
    assert_eq!(reopened.play_mode(), PlayMode::Shuffle);
}
