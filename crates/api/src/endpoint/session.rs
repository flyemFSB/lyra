//! 会话与认证端点：验证码发送、短信登录、设备注册及二次校验。

use lyra_proto::consts::{APPID, CLIENTVER, GITVERSION, RSA_PUBLIC, T1_IV, T1_KEY, T2_IV, T2_KEY, T2_MIDDLE, UA_LOGIN};
use lyra_proto::{
    aes_decrypt_derived, aes_encrypt_derived, aes_encrypt_with_key_iv, rsa_raw_encrypt_hex, sign_params_key,
};
use serde_json::Value;

use crate::client::Api;
use crate::codec;
use crate::error::{Error, Result};
use crate::spec::RequestSpec;
use crate::spec::{CAPTCHA_BASE, LOGIN_GATEWAY, USER_SERVICE, VERIFY_BASE, now_millis};

/// 登录结果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginResult {
    /// 用户 ID
    pub userid: i64,
    /// 登录令牌
    pub token: String,
    /// 会员类型（概念版恒为 0，实际状态需查 union_vip）
    pub vip_type: i64,
    /// 会员令牌
    pub vip_token: String,
    /// 用户昵称
    pub nickname: String,
}

/// 发送短信登录验证码（基于 v8 协议加密号码与密钥）
pub async fn send_mobile_code(api: &Api, mobile: &str) -> Result<Value> {
    let date_time = now_millis();
    let encrypted = aes_encrypt_derived(&serde_json::json!({ "mobile": mobile }).to_string())?;
    // pk 与登录同构：RSA 无填充加密 {clienttime_ms,key} 并转为大写十六进制
    let pk_plain = serde_json::json!({ "clienttime_ms": date_time, "key": encrypted.key }).to_string();
    let spec = RequestSpec::post("/v8/send_mobile_code")
        .base(CAPTCHA_BASE)
        .body(serde_json::json!({
            "plat": "1",
            "businessid": 5,
            "clienttime_ms": date_time,
            "pk": rsa_raw_encrypt_hex(pk_plain.as_bytes(), RSA_PUBLIC)?.to_uppercase(),
            "mobile": mask_mobile(mobile),
            "params": encrypted.hex,
        }))
        // 使用与登录一致的 UA，避免客户端版本错位触发发码风控
        .header("User-Agent", UA_LOGIN)
        .timeout(std::time::Duration::from_secs(20));
    api.call_auth(spec).await
}

/// 使用手机号与短信验证码执行登录
pub async fn login_cellphone(api: &Api, mobile: &str, code: &str) -> Result<LoginResult> {
    let session = api.session();
    let date_time = now_millis();

    let encrypted = aes_encrypt_derived(
        &serde_json::json!({
            "mobile": mobile,
            "code": code,
        })
        .to_string(),
    )?;

    let t2_plain = format!(
        "{}|{T2_MIDDLE}|{}|{}|{date_time}",
        session.device.guid, session.device.mac, session.device.dev
    );
    let t2 = aes_encrypt_with_key_iv(&t2_plain, T2_KEY, T2_IV)?;
    let t1 = aes_encrypt_with_key_iv(&format!("|{date_time}"), T1_KEY, T1_IV)?;

    let pk_plain = serde_json::json!({
        "clienttime_ms": date_time,
        "key": encrypted.key,
    })
    .to_string();

    let body = serde_json::json!({
        "plat": 1,
        "support_multi": 1,
        "t1": t1,
        "t2": t2,
        "clienttime_ms": date_time,
        "mobile": mask_mobile(mobile),
        "key": sign_params_key(date_time, APPID, CLIENTVER),
        "pk": rsa_raw_encrypt_hex(pk_plain.as_bytes(), RSA_PUBLIC)?.to_uppercase(),
        "params": encrypted.hex,
        "dfid": session.dfid(),
        "dev": session.device.dev,
        "gitversion": GITVERSION,
    });

    let spec = RequestSpec::post("/v7/login_by_verifycode")
        .base(LOGIN_GATEWAY)
        .body(body)
        .header("support-calm", "1")
        .header("User-Agent", UA_LOGIN)
        .timeout(std::time::Duration::from_secs(30));

    let value = api.call_auth(spec).await?;
    parse_login(&value, &encrypted.key)
}

/// 解析登录响应体并解密敏感安全参数
pub fn parse_login(value: &Value, temp_key: &str) -> Result<LoginResult> {
    let data = codec::pick(value, &["data"]).cloned().unwrap_or(Value::Null);
    let mut result = LoginResult {
        userid: codec::pick_i64(&data, &["userid", "user_id", "kugouid"]),
        vip_type: codec::pick_i64(&data, &["vip_type"]),
        vip_token: codec::pick_str(&data, &["vip_token"]),
        nickname: codec::pick_str(&data, &["nickname", "name"]),
        token: codec::pick_str(&data, &["token"]),
    };

    if let Some(secu) = codec::pick(&data, &["secu_params"]).map(codec::as_str) {
        if secu.is_empty() {
            return finish(result);
        }
        let plain = aes_decrypt_derived(&secu, temp_key)?;
        let text = String::from_utf8_lossy(&plain).into_owned();
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => {
                for (key, item) in map {
                    match key.as_str() {
                        "token" => result.token = codec::as_str(&item),
                        "userid" => result.userid = codec::as_i64(&item),
                        "vip_type" => result.vip_type = codec::as_i64(&item),
                        "vip_token" => result.vip_token = codec::as_str(&item),
                        "nickname" => result.nickname = codec::as_str(&item),
                        _ => {}
                    }
                }
            }
            _ => result.token = text,
        }
    }
    finish(result)
}

fn finish(result: LoginResult) -> Result<LoginResult> {
    if result.token.is_empty() || result.userid == 0 {
        return Err(Error::Decode(format!(
            "登录响应缺少 token/userid（userid={}, token_empty={}）",
            result.userid,
            result.token.is_empty()
        )));
    }
    Ok(result)
}

/// 手机号掩码脱敏，保留前两位与末尾一位
#[must_use]
pub fn mask_mobile(mobile: &str) -> String {
    let chars: Vec<char> = mobile.chars().collect();
    if chars.len() <= 2 {
        return mobile.to_owned();
    }
    let head: String = chars[..2].iter().collect();
    let tail: String = chars.get(10..11).map_or(String::new(), |s| s.iter().collect());
    format!("{head}*****{tail}")
}

/// 向风控服务注册设备身份并获取 dfid 凭据
pub async fn register_dev(api: &Api, extra: Option<Value>) -> Result<String> {
    let session = api.session();
    let mut device_map = serde_json::json!({
        "availableRamSize": 4_983_533_568_i64,
        "availableRomSize": 48_114_719_i64,
        "availableSDSize": 48_114_717_i64,
        "basebandVer": "",
        "batteryLevel": 100,
        "batteryStatus": 3,
        "brand": "Redmi",
        "buildSerial": "unknown",
        "device": "marble",
        "imei": session.device.guid,
        "imsi": "",
        "manufacturer": "Xiaomi",
        "uuid": session.device.guid,
    });
    // 补齐上游必填的 9 组传感器默认字段
    if let Some(map) = device_map.as_object_mut() {
        for sensor in [
            "accelerometer",
            "gravity",
            "gyroscope",
            "light",
            "magnetic",
            "orientation",
            "pressure",
            "step_counter",
            "temperature",
        ] {
            map.insert(sensor.to_owned(), Value::Bool(false));
            map.insert(format!("{sensor}Value"), Value::String(String::new()));
        }
        if let Some(extra) = extra.as_ref().and_then(Value::as_object) {
            for (key, value) in extra {
                map.insert(key.clone(), value.clone());
            }
        }
    }

    let encrypted = lyra_proto::secret_encrypt(&device_map.to_string())?;
    let portrait = serde_json::json!({
        "aes": encrypted.key,
        "uid": session.userid(),
        "token": session.token(),
    })
    .to_string();
    let p = lyra_proto::rsa_pkcs1_encrypt_hex(portrait.as_bytes(), RSA_PUBLIC)?;

    // Base64 密文请求体须与查询参数共同参与签名
    let spec = RequestSpec::post("/risk/v2/r_register_dev")
        .base(USER_SERVICE)
        .param("part", 1)
        .param("platid", 1)
        .param("p", p)
        .raw_body(encrypted.base64.clone().into_bytes())
        .header("x-router", "userservice.kugou.com")
        .timeout(std::time::Duration::from_secs(30));
    let request = spec.build(&session)?;
    let response = api.transport().execute(request).await?;
    if !(200..300).contains(&response.status) {
        return Err(Error::Http {
            status: response.status,
            body: response.text(),
        });
    }
    // 上游仅在成功时加密，报错时返回明文 JSON
    let text = codec::decode_session_body(&response.body, &encrypted.key);
    let value: Value =
        serde_json::from_str(&text).map_err(|_| Error::Decode(format!("register_dev 响应不是合法 JSON: {text}")))?;
    codec::check_business(&value)?;
    let dfid = codec::pick_str_paths(&value, &[&["data", "dfid"], &["dfid"]]);
    if dfid.is_empty() {
        // 上游偶发返回空数据，包装为可重试错误
        return Err(Error::EmptyData(format!("register_dev 未返回 dfid: {text}")));
    }
    Ok(dfid)
}

/// 提交人机验证或二次校验验证码
pub async fn verify_user_info(api: &Api, sid: &str, edt: &str, verifycode: &str, v_type: i64) -> Result<Value> {
    let session = api.session();
    let encrypted = if v_type == 32 {
        aes_encrypt_derived(&serde_json::json!({ "code": verifycode }).to_string())?
    } else {
        aes_encrypt_derived("{}")?
    };
    let pk_plain = serde_json::json!({ "key": encrypted.key }).to_string();

    let body = serde_json::json!({
        "eventid": Value::Null,
        "userid": session.userid(),
        "platid": 2,
        "v_type": v_type,
        "wasm": 1,
        "i": "",
        "sid": sid,
        "edt": edt,
        "verifycode": verifycode,
        "code": verifycode,
        "pk": rsa_raw_encrypt_hex(pk_plain.as_bytes(), RSA_PUBLIC)?.to_uppercase(),
        "params": encrypted.hex,
    });

    let spec = RequestSpec::post("/v4/verify_user_info")
        .base(VERIFY_BASE)
        .param("clientver", 11_510)
        .body(body)
        .timeout(std::time::Duration::from_secs(30));
    api.call_auth(spec).await
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::client::Api;
    use crate::spec::Session;

    // 构造具备设备身份但未登录的测试会话
    fn session() -> Session {
        Session::new(crate::testing::device("DfidValue"))
    }
    use crate::transport::MockTransport;

    #[test]
    fn masks_mobile_like_the_client() {
        assert_eq!(mask_mobile("13800138000"), "13*****0");
        assert_eq!(mask_mobile("155"), "15*****");
    }

    #[tokio::test]
    async fn register_dev_reports_the_upstream_error_instead_of_a_crypto_error() {
        // 上游返回明文 JSON 错误时透传业务错误码
        let mock = Arc::new(MockTransport::new());
        mock.on(
            "/risk/v2/r_register_dev",
            crate::transport::Response::new(200, br#"{"data":null,"status":0,"error_code":20006}"#.to_vec()),
        );
        let api = Api::new(mock, session(), 64);
        let error = register_dev(&api, None).await.unwrap_err();
        assert!(
            matches!(error, Error::Upstream { code: 20_006, .. }),
            "应当暴露上游错误码：{error:?}"
        );
    }

    #[tokio::test]
    async fn register_dev_flags_empty_data_as_retryable() {
        // 空数据响应标记为可重试错误
        let mock = Arc::new(MockTransport::new());
        mock.on(
            "/risk/v2/r_register_dev",
            crate::transport::Response::new(200, br#"{"status":1,"error_code":0,"data":[]}"#.to_vec()),
        );
        let api = Api::new(mock, session(), 64);
        let error = register_dev(&api, None).await.unwrap_err();
        assert!(matches!(error, Error::EmptyData(_)), "{error:?}");
    }

    #[tokio::test]
    async fn register_dev_signs_the_base64_body() {
        // 验证 Base64 请求体参与安卓签名
        let mock = Arc::new(MockTransport::new());
        mock.on(
            "/risk/v2/r_register_dev",
            crate::transport::Response::new(200, br#"{"data":null,"status":0,"error_code":20006}"#.to_vec()),
        );
        let api = Api::new(mock.clone(), session(), 64);
        let _ = register_dev(&api, None).await;
        let request = mock.last_request().unwrap();
        let body = String::from_utf8(request.body.clone().unwrap()).unwrap();
        let mut params = lyra_proto::Params::new();
        for (key, value) in &request.query {
            if key != "signature" {
                params.set(key.clone(), value.clone());
            }
        }
        let actual = request
            .query
            .iter()
            .find(|(key, _)| key == "signature")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert_eq!(actual, lyra_proto::signature_android(&params, &body));
    }

    #[test]
    fn parses_login_with_json_secu_params() {
        let temp_key = "abcdefghijklmnop";
        let inner = serde_json::json!({"token": "TKN", "userid": 2043841403, "vip_token": "VT"});
        let secu = lyra_proto::aes_encrypt_derived_with(&inner.to_string(), temp_key).unwrap();
        let value = serde_json::json!({
            "status": 1,
            "data": {"secu_params": secu.hex, "userid": 2043841403, "vip_type": 0, "nickname": "lyra"}
        });
        let result = parse_login(&value, temp_key).unwrap();
        assert_eq!(result.token, "TKN");
        assert_eq!(result.userid, 2_043_841_403);
        assert_eq!(result.vip_token, "VT");
        assert_eq!(result.nickname, "lyra");
    }

    #[test]
    fn parses_login_with_plain_string_secu_params() {
        let temp_key = "abcdefghijklmnop";
        let secu = lyra_proto::aes_encrypt_derived_with("PLAINTOKEN", temp_key).unwrap();
        let value = serde_json::json!({
            "status": 1,
            "data": {"secu_params": secu.hex, "userid": 2043841403}
        });
        let result = parse_login(&value, temp_key).unwrap();
        assert_eq!(result.token, "PLAINTOKEN");
    }

    #[test]
    fn login_without_token_is_an_error() {
        let value = serde_json::json!({"status": 1, "data": {"userid": 2043841403}});
        assert!(parse_login(&value, "k").is_err());
    }

    #[tokio::test]
    async fn login_sends_the_lite_t1_t2_and_rsa_pk() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v7/login_by_verifycode", &serde_json::json!({"status": 1, "data": {}}));
        let api = Api::new(mock.clone(), session(), 8);
        // 验证必传参数完整组装发送
        let _ = login_cellphone(&api, "13800138000", "123456").await;
        let request = mock.last_request().unwrap();
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["plat"], 1);
        assert_eq!(body["mobile"], "13*****0");
        assert_eq!(body["gitversion"], GITVERSION);
        assert_eq!(body["dev"], "ABCDEFGHIJ");
        assert_eq!(body["dfid"], "DfidValue");
        // 校验 pk 为 256 位大写十六进制
        let pk = body["pk"].as_str().unwrap();
        assert_eq!(pk.len(), 256);
        assert!(pk.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
        // 校验 t1 与 t2 为合法的十六进制密文
        assert_eq!(body["t1"].as_str().unwrap().len(), 32);
        assert!(body["t2"].as_str().unwrap().len() > 32);
        assert!(body["params"].as_str().unwrap().len() > 32);
        assert_eq!(request.get_header("User-Agent"), Some(UA_LOGIN));
        assert_eq!(request.get_header("support-calm"), Some("1"));
        // 登录服务校验严格，不可携带扰动参数
        assert!(
            !request.query.iter().any(|(k, _)| k == "_lyra_t"),
            "{:?}",
            request.query
        );
    }

    #[tokio::test]
    async fn send_mobile_code_uses_the_v8_crypto_shape() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v8/send_mobile_code", &serde_json::json!({"status": 1, "count": 8}));
        let api = Api::new(mock.clone(), session(), 8);
        send_mobile_code(&api, "13800138000").await.unwrap();
        let request = mock.last_request().unwrap();
        assert!(
            request.url.starts_with("http://login.user.kugou.com/v8/"),
            "{}",
            request.url
        );
        let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["businessid"], 5);
        assert_eq!(body["plat"], "1", "官方 v8 的 plat 是字符串");
        assert!(
            !request.query.iter().any(|(k, _)| k == "_lyra_t"),
            "带扰动会被登录服务判失败（实测 20006）: {:?}",
            request.query
        );
        assert!(body["clienttime_ms"].is_number());
        assert_eq!(
            request.get_header("User-Agent"),
            Some(UA_LOGIN),
            "发码必须用登录模块的 UA"
        );
        assert!(body["pk"].as_str().unwrap().len() >= 128, "pk 应为 RSA 密文 hex");
        assert_ne!(body["mobile"], "13800138000", "外层 mobile 必须打码");
        // params 为加密后的手机号，长度应为 AES 块整数倍
        let params = body["params"].as_str().unwrap();
        assert!(!params.is_empty() && params.len().is_multiple_of(32), "{params}");
        assert!(params.bytes().all(|b| b.is_ascii_hexdigit()), "{params}");
    }
}
