//! HTTP 请求装配：管理公共参数、协议头与防篡改签名。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lyra_proto::consts::{APPID, CLIENTVER, KG_RC, KG_REC, KG_RF, KG_THASH, UA_DEFAULT};
use lyra_proto::{Params, Val, sign_key, signature_android, signature_web};

use crate::error::{Error, Result};
use crate::transport::{Method, Request};

/// 默认 API 网关
pub const GATEWAY: &str = "https://gateway.kugou.com";
/// 短信登录重试网关
pub const LOGIN_GATEWAY: &str = "https://loginserviceretry.kugou.com";
/// 验证码下发服务（明文 HTTP 协议）
pub const CAPTCHA_BASE: &str = "http://login.user.kugou.com";
/// 设备注册服务
pub const USER_SERVICE: &str = "https://userservice.kugou.com";
/// 歌词服务
pub const LYRICS_BASE: &str = "https://lyrics.kugou.com";
/// 会员服务
pub const VIP_BASE: &str = "https://kugouvip.kugou.com";
/// 行为验证服务
pub const VERIFY_BASE: &str = "https://verifyservice.kugou.com";
/// 用户关系服务
pub const RELATION_BASE: &str = "http://relation.user.kugou.com";

/// 签名模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignMode {
    /// Android 客户端签名（默认）
    Android,
    /// Web / H5 签名
    Web,
    /// 设备注册签名
    Register,
    /// 不签名
    None,
}

/// 获取当前 Unix 毫秒时间戳
#[must_use]
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// 获取当前 Unix 秒级时间戳
#[must_use]
pub fn now_secs() -> i64 {
    now_millis() / 1000
}

/// 会话上下文（包含设备标识与登录态）
#[derive(Debug, Clone)]
pub struct Session {
    /// 设备指纹信息
    pub device: lyra_proto::Device,
    /// 账号信息（未登录为 None）
    pub account: Option<Account>,
}

/// 账号登录凭据
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// 用户 ID
    pub userid: i64,
    /// 登录令牌
    pub token: String,
    /// VIP 会员类型
    pub vip_type: i64,
    /// VIP 令牌
    pub vip_token: String,
}

impl Session {
    /// 根据设备指纹创建未登录会话
    #[must_use]
    pub fn new(device: lyra_proto::Device) -> Self {
        Self { device, account: None }
    }

    /// 附加账号登录凭据
    #[must_use]
    pub fn with_account(mut self, account: Account) -> Self {
        self.account = Some(account);
        self
    }

    /// 更新或清除登录态
    pub fn set_account(&mut self, account: Option<Account>) {
        self.account = account;
    }

    /// 获取当前用户 ID（未登录返回 0）
    #[must_use]
    pub fn userid(&self) -> i64 {
        self.account.as_ref().map_or(0, |a| a.userid)
    }

    /// 获取当前登录 Token（未登录返回空串）
    #[must_use]
    pub fn token(&self) -> &str {
        self.account.as_ref().map_or("", |a| a.token.as_str())
    }

    /// 获取会员类型
    #[must_use]
    pub fn vip_type(&self) -> i64 {
        self.account.as_ref().map_or(0, |a| a.vip_type)
    }

    /// 检查是否处于已登录状态
    #[must_use]
    pub fn is_logged_in(&self) -> bool {
        self.account
            .as_ref()
            .is_some_and(|a| a.userid != 0 && !a.token.is_empty())
    }

    /// 获取设备指纹 `dfid`（未注册时返回 `"-"`）
    #[must_use]
    pub fn dfid(&self) -> &str {
        self.device.dfid_or_dash()
    }
}

/// 请求构建规格
#[derive(Debug, Clone)]
pub struct RequestSpec {
    /// HTTP 请求方法
    pub method: Method,
    /// 基础 URL
    pub base_url: String,
    /// 相对路径
    pub path: String,
    /// 业务请求参数
    pub params: Params,
    /// JSON 请求体
    pub body: Option<serde_json::Value>,
    /// 原始二进制请求体（参与签名计算）
    pub raw_body: Option<Vec<u8>>,
    /// 自定义请求头
    pub headers: Vec<(String, String)>,
    /// 是否禁用公共默认参数
    pub clear_default_params: bool,
    /// 是否禁用默认请求头
    pub clear_default_headers: bool,
    /// 签名算法模式
    pub sign: SignMode,
    /// 是否追加音源验证密钥 `signKey`
    pub encrypt_key: bool,
    /// 透传客户端 IP
    pub ip: Option<String>,
    /// 覆盖本次请求的 `dfid`
    pub dfid_override: Option<String>,
    /// 签名后追加的额外查询参数
    pub extra_query: Vec<(String, String)>,
    /// 请求超时时长
    pub timeout: Option<Duration>,
}

impl RequestSpec {
    /// 创建 GET 请求规格
    #[must_use]
    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::Get, path)
    }

    /// 创建 POST 请求规格
    #[must_use]
    pub fn post(path: impl Into<String>) -> Self {
        Self::new(Method::Post, path)
    }

    fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            base_url: GATEWAY.to_owned(),
            path: path.into(),
            params: Params::new(),
            body: None,
            raw_body: None,
            headers: Vec::new(),
            clear_default_params: false,
            clear_default_headers: false,
            sign: SignMode::Android,
            encrypt_key: false,
            dfid_override: None,
            ip: None,
            extra_query: Vec::new(),
            timeout: Some(Duration::from_secs(15)),
        }
    }

    /// 覆盖基础 URL
    #[must_use]
    pub fn base(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }

    /// 添加单个查询参数
    #[must_use]
    pub fn param(mut self, key: impl Into<String>, value: impl Into<Val>) -> Self {
        self.params.set(key, value);
        self
    }

    /// 设置 JSON 请求体
    #[must_use]
    pub fn body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }

    /// 设置原始二进制请求体
    #[must_use]
    pub fn raw_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.raw_body = Some(body.into());
        self
    }

    /// 添加自定义请求头
    #[must_use]
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// 禁用公共默认参数
    #[must_use]
    pub fn clear_default_params(mut self) -> Self {
        self.clear_default_params = true;
        self
    }

    /// 禁用默认请求头
    #[must_use]
    pub fn clear_default_headers(mut self) -> Self {
        self.clear_default_headers = true;
        self
    }

    /// 禁用请求签名
    #[must_use]
    pub fn no_signature(mut self) -> Self {
        self.sign = SignMode::None;
        self
    }

    /// 启用音源验证密钥追加
    #[must_use]
    pub fn encrypt_key(mut self) -> Self {
        self.encrypt_key = true;
        self
    }

    /// 覆盖本次请求的设备指纹 `dfid`
    #[must_use]
    pub fn dfid(mut self, dfid: impl Into<String>) -> Self {
        self.dfid_override = Some(dfid.into());
        self
    }

    /// 设置透传的客户端 IP
    #[must_use]
    pub fn ip(mut self, ip: impl Into<String>) -> Self {
        let ip = ip.into();
        if !ip.is_empty() {
            self.ip = Some(ip);
        }
        self
    }

    /// 添加不参与签名的查询参数
    #[must_use]
    pub fn extra_query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_query.push((key.into(), value.into()));
        self
    }

    /// 设置请求超时时长
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// 检查请求是否包含登录 Token
    #[must_use]
    pub fn requires_login(&self) -> bool {
        self.params.contains("token") || self.body.as_ref().is_some_and(|body| body.get("token").is_some())
    }

    /// 装配并生成可发送的 HTTP 请求
    pub fn build(&self, session: &Session) -> Result<Request> {
        let mut params = Params::new();
        let dfid = self.dfid_override.as_deref().unwrap_or_else(|| session.dfid());

        if !self.clear_default_params {
            params
                .set("dfid", dfid)
                .set("mid", session.device.mid.clone())
                .set("uuid", "-")
                .set("appid", APPID)
                .set("clientver", CLIENTVER)
                .set("clienttime", now_secs());
            let token = session.token();
            if !token.is_empty() {
                params.set("token", token);
            }
            let userid = session.userid();
            if userid != 0 {
                params.set("userid", userid);
            }
        }

        for (key, value) in self.params.iter() {
            params.set(key, value.clone());
        }

        // 注入 signKey 参与签名计算
        if self.encrypt_key {
            let key = sign_key(
                params.get_str("hash").unwrap_or_default(),
                params.get_str("mid").unwrap_or_default(),
                param_i64(&params, "userid", 0),
                u32::try_from(param_i64(&params, "appid", i64::from(APPID))).unwrap_or(APPID),
            );
            params.set("key", key);
        }

        // 统一请求体序列化以参与签名
        let body_bytes: Option<Vec<u8>> = match (&self.body, &self.raw_body) {
            (Some(value), _) => Some(value.to_string().into_bytes()),
            (None, Some(raw)) => Some(raw.clone()),
            (None, None) => None,
        };
        let body_string = body_bytes
            .as_ref()
            .map_or_else(String::new, |bytes| String::from_utf8_lossy(bytes).into_owned());

        if !params.contains("signature") {
            let signature = match self.sign {
                SignMode::Android => Some(signature_android(&params, &body_string)),
                SignMode::Web => Some(signature_web(&params, &body_string)),
                SignMode::Register => Some(lyra_proto::signature_register(&params)),
                SignMode::None => None,
            };
            if let Some(signature) = signature {
                params.set("signature", signature);
            }
        }

        let clienttime_header = params.get("clienttime").map(Val::render);
        let mut protocol_headers: Vec<(String, String)> = vec![
            ("dfid".to_owned(), dfid.to_owned()),
            ("mid".to_owned(), session.device.mid.clone()),
            ("kg-rc".to_owned(), KG_RC.to_owned()),
            ("kg-thash".to_owned(), KG_THASH.to_owned()),
            ("kg-rec".to_owned(), KG_REC.to_owned()),
            ("kg-rf".to_owned(), KG_RF.to_owned()),
        ];
        if let Some(clienttime) = clienttime_header {
            protocol_headers.push(("clienttime".to_owned(), clienttime));
        }
        if let Some(ip) = &self.ip {
            protocol_headers.push(("X-Real-IP".to_owned(), ip.clone()));
            protocol_headers.push(("X-Forwarded-For".to_owned(), ip.clone()));
        }

        let mut layers: Vec<(String, String)> = Vec::new();
        if !self.clear_default_headers {
            layers.push(("User-Agent".to_owned(), UA_DEFAULT.to_owned()));
        }
        // 存在 JSON 体时自动设置 Content-Type
        if self.body.is_some() {
            layers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        }
        layers.extend(self.headers.iter().cloned());
        layers.extend(protocol_headers);

        let mut query = params.to_pairs();
        query.extend(self.extra_query.iter().cloned());

        let mut request = Request::new(self.method, combine_urls(&self.base_url, &self.path));
        request.query = query;
        request.headers = merge_headers(layers);
        request.body = body_bytes;
        request.timeout = self.timeout;
        Ok(request)
    }
}

/// 从参数表中解析 64 位整数
fn param_i64(params: &Params, key: &str, fallback: i64) -> i64 {
    params.get(key).map_or(fallback, |value| match value {
        Val::Raw(text) => text.trim().parse::<i64>().unwrap_or(fallback),
        Val::Json(json) => crate::codec::as_i64(json),
    })
}

/// 拼接 URL 基础路径与子路径并规范化斜杠
#[must_use]
pub fn combine_urls(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        base.to_owned()
    } else {
        format!("{base}/{path}")
    }
}

/// 合并请求头并消除大小写重复
#[must_use]
pub fn merge_headers(layers: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(layers.len());
    for (key, value) in layers {
        match out.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            Some(slot) => slot.1 = value,
            None => out.push((key, value)),
        }
    }
    out
}

/// 校验当前会话已登录
pub fn ensure_logged_in(session: &Session) -> Result<()> {
    if session.is_logged_in() {
        Ok(())
    } else {
        Err(Error::NotLoggedIn)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn session() -> Session {
        Session::new(lyra_proto::Device {
            guid: "e8456cee7fd2348cfa239342b10a23bd".to_owned(),
            mid: "12345678901234567890".to_owned(),
            mac: "02:00:00:00:00:00".to_owned(),
            dev: "ABCDEFGHIJ".to_owned(),
            webgl: "1".to_owned(),
            dfid: "DfidValue".to_owned(),
        })
        .with_account(Account {
            userid: 2_043_841_403,
            token: "TOKEN123".to_owned(),
            vip_type: 0,
            vip_token: String::new(),
        })
    }

    #[test]
    fn combine_urls_matches_axios() {
        assert_eq!(combine_urls("https://a.com/", "/v1/x"), "https://a.com/v1/x");
        assert_eq!(combine_urls("https://a.com", "v1/x"), "https://a.com/v1/x");
        assert_eq!(
            combine_urls("https://a.com", "youth/v1/song/single_card_recommend"),
            "https://a.com/youth/v1/song/single_card_recommend"
        );
    }

    #[test]
    fn default_params_and_headers_are_injected() {
        let request = RequestSpec::get("/v1/x").build(&session()).unwrap();
        let names: Vec<&str> = request.query.iter().map(|(k, _)| k.as_str()).collect();
        for expected in [
            "dfid",
            "mid",
            "uuid",
            "appid",
            "clientver",
            "clienttime",
            "token",
            "userid",
            "signature",
        ] {
            assert!(names.contains(&expected), "缺少 {expected}: {names:?}");
        }
        assert_eq!(request.get_header("kg-rf"), Some("B9EDA08A64250DEFFBCADDEE00F8F25F"));
        assert_eq!(request.get_header("User-Agent"), Some(lyra_proto::consts::UA_DEFAULT));
        assert_eq!(request.get_header("dfid"), Some("DfidValue"));
    }

    #[test]
    fn raw_body_enters_the_signature_and_the_request() {
        // 设备注册的体是 Base64 密文（不是 JSON），漏出签名会让上游直接判签名错误 20006。
        let raw = "QUJD";
        let request = RequestSpec::post("/risk/v2/r_register_dev")
            .param("part", 1)
            .raw_body(raw.as_bytes().to_vec())
            .build(&session())
            .unwrap();
        assert_eq!(request.body.as_deref(), Some(raw.as_bytes()));
        let signature = request
            .query
            .iter()
            .find(|(key, _)| key == "signature")
            .map(|(_, value)| value.clone())
            .unwrap();
        // 用该请求自己的参数重算契约：salt + 排序后的 k=v + body + salt。
        let params = |body: &str| {
            let mut params = lyra_proto::Params::new();
            for (key, value) in &request.query {
                if key != "signature" {
                    params.set(key.clone(), value.clone());
                }
            }
            lyra_proto::signature_android(&params, body)
        };
        assert_eq!(signature, params(raw), "签名必须包含原始请求体");
        assert_ne!(signature, params(""), "不含 body 的签名必须不同（否则本测试无效）");
    }

    #[test]
    fn anonymous_session_omits_token_and_userid() {
        let anonymous = Session::new(lyra_proto::Device {
            guid: "g".to_owned(),
            mid: "1".to_owned(),
            mac: String::new(),
            dev: String::new(),
            webgl: String::new(),
            dfid: "-".to_owned(),
        });
        let request = RequestSpec::get("/v1/x").build(&anonymous).unwrap();
        assert!(!request.query.iter().any(|(k, _)| k == "token"));
        assert!(!request.query.iter().any(|(k, _)| k == "userid"));
    }

    #[test]
    fn clear_default_params_keeps_only_module_params() {
        let request = RequestSpec::get("/v1/search")
            .clear_default_params()
            .no_signature()
            .param("hash", "abc")
            .build(&session())
            .unwrap();
        let names: Vec<&str> = request.query.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, vec!["hash"]);
        // 无时间戳参数时请求头中亦不携带 clienttime。
        assert_eq!(request.get_header("clienttime"), None);
        assert_eq!(request.get_header("dfid"), Some("DfidValue"));
    }

    #[test]
    fn no_signature_omits_the_signature_but_keeps_params() {
        let request = RequestSpec::get("/v1/search")
            .clear_default_params()
            .no_signature()
            .build(&session())
            .unwrap();
        assert!(!request.query.iter().any(|(k, _)| k == "signature"));
    }

    #[test]
    fn clear_default_headers_drops_the_ua() {
        let request = RequestSpec::get("/v1/x")
            .clear_default_headers()
            .header("x-router", "svc.kugou.com")
            .build(&session())
            .unwrap();
        assert_eq!(request.get_header("User-Agent"), None);
        assert_eq!(request.get_header("x-router"), Some("svc.kugou.com"));
    }

    #[test]
    fn explicit_ua_overrides_default_but_kg_headers_win() {
        let request = RequestSpec::get("/v1/x")
            .header("User-Agent", "custom")
            .header("mid", "OVERRIDDEN")
            .build(&session())
            .unwrap();
        assert_eq!(request.get_header("User-Agent"), Some("custom"));
        assert_eq!(request.get_header("mid"), Some("12345678901234567890"));
    }

    #[test]
    fn encrypt_key_injects_sign_key_into_the_signature() {
        let without = RequestSpec::get("/tracker/v5/url")
            .param("hash", "ABC")
            .build(&session())
            .unwrap();
        let with = RequestSpec::get("/tracker/v5/url")
            .param("hash", "ABC")
            .encrypt_key()
            .build(&session())
            .unwrap();
        let key_of = |r: &Request| r.query.iter().find(|(k, _)| k == "key").map(|(_, v)| v.clone());
        assert!(key_of(&without).is_none());
        assert_eq!(
            key_of(&with).unwrap(),
            lyra_proto::sign_key("ABC", "12345678901234567890", 2_043_841_403, APPID)
        );
        let sig_of = |r: &Request| {
            r.query
                .iter()
                .find(|(k, _)| k == "signature")
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_ne!(sig_of(&without), sig_of(&with), "key 必须进入签名串");
    }

    #[test]
    fn extra_query_is_not_signed() {
        let plain = RequestSpec::get("/v1/x").build(&session()).unwrap();
        let perturbed = RequestSpec::get("/v1/x")
            .extra_query("_t", "12345")
            .build(&session())
            .unwrap();
        let sig = |r: &Request| {
            r.query
                .iter()
                .find(|(k, _)| k == "signature")
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(sig(&plain), sig(&perturbed));
    }

    #[test]
    fn body_is_signed_exactly_as_it_is_sent() {
        let body = serde_json::json!({"listid": 2, "userid": 2043841403});
        let request = RequestSpec::post("/v1/x").body(body.clone()).build(&session()).unwrap();
        assert_eq!(request.body.as_deref().unwrap(), body.to_string().as_bytes());
    }

    #[test]
    fn login_style_headers_are_the_ones_the_reference_sends() {
        let request = RequestSpec::get("/v1/x").build(&session()).unwrap();
        assert_eq!(request.get_header("kg-rc"), Some("1"));
        assert_eq!(request.get_header("kg-thash"), Some("5d816a0"));
        assert_eq!(request.get_header("kg-rec"), Some("1"));
    }
}
