//! API 调用门面：整合缓存、单飞合并、网络传输与二次验证。

use std::sync::Arc;

use parking_lot::RwLock;
use serde_json::Value;

use crate::cache::{Freshness, TtlCache};
use crate::codec;
use crate::error::{Code, Error, Result};
use crate::single_flight::SingleFlight;
use crate::spec::{RequestSpec, Session, now_millis};
use crate::transport::Transport;

/// API 统一调用客户端
#[derive(Clone)]
pub struct Api {
    transport: Arc<dyn Transport>,
    session: Arc<RwLock<Session>>,
    cache: TtlCache,
    flight: Arc<SingleFlight<Arc<Value>>>,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("logged_in", &self.session.read().is_logged_in())
            .finish_non_exhaustive()
    }
}

impl Api {
    /// 创建 API 客户端实例
    #[must_use]
    pub fn new(transport: Arc<dyn Transport>, session: Session, cache_capacity: u64) -> Self {
        Self {
            transport,
            session: Arc::new(RwLock::new(session)),
            cache: TtlCache::new(cache_capacity),
            flight: Arc::new(SingleFlight::new()),
        }
    }

    /// 获取当前会话快照
    #[must_use]
    pub fn session(&self) -> Session {
        self.session.read().clone()
    }

    /// 替换当前会话
    pub fn set_session(&self, session: Session) {
        *self.session.write() = session;
    }

    /// 更新账号登录态
    pub fn set_account(&self, account: Option<crate::spec::Account>) {
        self.session.write().set_account(account);
    }

    /// 获取底层传输实现句柄
    #[must_use]
    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    /// 获取内存缓存句柄
    #[must_use]
    pub fn cache(&self) -> &TtlCache {
        &self.cache
    }

    /// 获取被单飞合并拦截的请求总数
    #[must_use]
    pub fn coalesced(&self) -> u64 {
        self.flight.coalesced()
    }

    /// 执行 API 请求（整合缓存、单飞与自动重试）
    pub async fn call(&self, spec: RequestSpec, freshness: Freshness) -> Result<Value> {
        let session = self.session();
        if spec.requires_login() && !session.is_logged_in() {
            return Err(Error::NotLoggedIn);
        }

        // 具有副作用的请求追加时间戳防重
        let spec = if matches!(freshness, Freshness::SideEffect) {
            spec.extra_query("_lyra_t", now_millis().to_string())
        } else {
            spec
        };

        match self.dispatch(&spec, &session, freshness).await {
            Err(error) if needs_verify(&error) => {
                // 上游要求行为验证时生成指纹重试
                let (edt, sid) = self.fingerprint()?;
                let retry = spec
                    .clone()
                    .param("edt", edt)
                    .param("sid", sid)
                    .timeout(std::time::Duration::from_secs(20));
                self.dispatch(&retry, &session, Freshness::NoStore).await
            }
            other => other,
        }
    }

    /// 鉴权类端点调用（不缓存且不追加扰动）
    pub async fn call_auth(&self, spec: RequestSpec) -> Result<Value> {
        self.call(spec, Freshness::NoStore).await
    }

    /// 生成设备行为指纹元组 (edt, sid)
    pub fn fingerprint(&self) -> Result<(String, String)> {
        let session = self.session.read().clone();
        let sim = lyra_proto::simulate_now(&lyra_proto::SimInputs {
            mid: &session.device.mid,
            userid: &session.userid().to_string(),
            dfid: session.dfid(),
            webgl: &session.device.webgl,
            ts_millis: now_millis(),
        })?;
        Ok((sim.edt, sim.sid))
    }

    async fn dispatch(&self, spec: &RequestSpec, session: &Session, freshness: Freshness) -> Result<Value> {
        let request = spec.build(session)?;
        let cache_key = format!("{}::{}", spec.path, request.cache_key());

        if let Some(body) = freshness.ttl().and_then(|_| self.cache.get(&cache_key)) {
            return codec::check_response(200, &body);
        }

        let transport = self.transport.clone();
        let key = cache_key.clone();
        let ttl = freshness.ttl();
        let cache = self.cache.clone();

        let value = self
            .flight
            .run(cache_key, || async move {
                let response = transport.execute(request).await?;
                let ssa = response.ssa_code().map(str::to_owned);
                let body = Arc::new(response.body);
                match codec::check_response(response.status, &body) {
                    Ok(value) => {
                        if let Some(ttl) = ttl {
                            cache.insert(key, body, ttl);
                        }
                        Ok(Arc::new(value))
                    }
                    Err(error) => Err(attach_ssa(error, ssa)),
                }
            })
            .await?;
        Ok((*value).clone())
    }
}

/// 根据 ssa-code 响应头附加验证错误
fn attach_ssa(error: Error, ssa: Option<String>) -> Error {
    ssa.map_or(error, Error::NeedsVerify)
}

/// 判断错误是否要求二次行为验证
#[must_use]
pub fn needs_verify(error: &Error) -> bool {
    // 匹配验证错误变体或对应业务错误码
    matches!(error, Error::NeedsVerify(_)) || error.code() == Some(Code::NeedVerify)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::cache::Freshness;
    use crate::transport::MockTransport;

    fn session() -> Session {
        Session::new(lyra_proto::Device {
            guid: "e8456cee7fd2348cfa239342b10a23bd".to_owned(),
            mid: "12345678901234567890".to_owned(),
            mac: "02:00:00:00:00:00".to_owned(),
            dev: "ABCDEFGHIJ".to_owned(),
            webgl: "1".to_owned(),
            dfid: "DfidValue".to_owned(),
        })
    }

    #[tokio::test]
    async fn cached_response_does_not_hit_the_transport_twice() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v1/x", &serde_json::json!({"status": 1, "data": {"n": 1}}));
        let api = Api::new(mock.clone(), session(), 64);

        for _ in 0..3 {
            let value = api.call(RequestSpec::get("/v1/x"), Freshness::PLAYLIST).await.unwrap();
            assert_eq!(value["data"]["n"], 1);
        }
        assert_eq!(mock.count(), 1, "第二次起应命中 L1 缓存");
    }

    #[tokio::test]
    async fn side_effect_endpoints_bypass_cache_and_get_a_perturbation() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v1/add", &serde_json::json!({"status": 1}));
        let api = Api::new(mock.clone(), session(), 64);

        api.call(RequestSpec::post("/v1/add"), Freshness::SideEffect)
            .await
            .unwrap();
        api.call(RequestSpec::post("/v1/add"), Freshness::SideEffect)
            .await
            .unwrap();
        assert_eq!(mock.count(), 2, "有副作用的端点不能被缓存");
        for request in mock.requests() {
            assert!(
                request.query.iter().any(|(k, _)| k == "_lyra_t"),
                "必须追加毫秒扰动参数"
            );
        }
    }

    #[tokio::test]
    async fn business_error_is_surfaced() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v1/x", &serde_json::json!({"status": 0, "msg": "boom"}));
        let api = Api::new(mock.clone(), session(), 64);
        let error = api
            .call(RequestSpec::get("/v1/x"), Freshness::NoStore)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Upstream { .. }));
    }

    #[tokio::test]
    async fn ssa_code_triggers_exactly_one_fingerprint_retry() {
        let mock = Arc::new(MockTransport::new());
        mock.on_ssa(
            "/v1/verify",
            "SSA123",
            &serde_json::json!({"status": 0, "error_code": 20028, "error": "验证"}),
        );
        let api = Api::new(mock.clone(), session(), 64);
        let error = api
            .call(RequestSpec::get("/v1/verify"), Freshness::NoStore)
            .await
            .unwrap_err();
        // 重试失败后返回验证错误。
        assert_eq!(mock.count(), 2, "应当只重试一次");
        assert!(needs_verify(&error), "{error:?}");
        // 校验重试请求包含指纹参数。
        let retry = mock.last_request().unwrap();
        let names: Vec<&str> = retry.query.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"sid"), "{names:?}");
        assert!(names.contains(&"edt"), "{names:?}");
    }

    #[tokio::test]
    async fn ssa_header_alone_triggers_the_retry() {
        // 验证仅凭响应头中的 ssa-code 也能触发重试机制。
        let mock = Arc::new(MockTransport::new());
        mock.on_ssa(
            "/tracker/v5/url",
            "gz_tx_event_deadbeef",
            &serde_json::json!({"errcode": 20028, "status": 0, "error": "本次请求需要验证"}),
        );
        let api = Api::new(mock.clone(), session(), 64);
        let error = api
            .call(RequestSpec::get("/tracker/v5/url"), Freshness::NoStore)
            .await
            .unwrap_err();
        assert_eq!(mock.count(), 2, "带 ssa-code 的失败应答必须重试一次");
        assert!(needs_verify(&error), "{error:?}");
        let retry = mock.last_request().unwrap();
        let names: Vec<&str> = retry.query.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"sid"), "{names:?}");
        assert!(names.contains(&"edt"), "{names:?}");
    }

    #[tokio::test]
    async fn ssa_retry_succeeds_when_the_second_attempt_passes() {
        let mock = Arc::new(MockTransport::new());
        // 注册模拟应答桩。
        mock.on_ssa(
            "/v1/maybe",
            "SSA123",
            &serde_json::json!({"status": 0, "error_code": 20028}),
        );
        let mut ok = crate::transport::Response::json_body(&serde_json::json!({"status": 1, "data": {"ok": true}}));
        ok.headers.push(("ssa-code".to_owned(), "SSA123".to_owned()));
        mock.on("/v1/maybe", ok);
        let api = Api::new(mock.clone(), session(), 64);
        let value = api
            .call(RequestSpec::get("/v1/maybe"), Freshness::NoStore)
            .await
            .unwrap();
        assert_eq!(value["data"]["ok"], true);
    }

    #[tokio::test]
    async fn login_required_endpoint_rejects_anonymous_session() {
        let mock = Arc::new(MockTransport::new());
        let api = Api::new(mock.clone(), session(), 64);
        let error = api
            .call(RequestSpec::post("/v1/x").param("token", "T"), Freshness::NoStore)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::NotLoggedIn));
        assert_eq!(mock.count(), 0, "未登录不应发出请求");
    }

    /// 模拟网络延迟以测试并发合并逻辑。
    struct SlowTransport {
        inner: Arc<MockTransport>,
        delay: std::time::Duration,
    }

    #[async_trait::async_trait]
    impl crate::transport::Transport for SlowTransport {
        async fn execute(&self, request: crate::transport::Request) -> Result<crate::transport::Response> {
            tokio::time::sleep(self.delay).await;
            self.inner.execute(request).await
        }
    }

    #[tokio::test]
    async fn concurrent_posts_with_different_bodies_are_not_coalesced() {
        // 请求体不同的并发 POST 请求不能被合并。
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v4/delete_songs", &serde_json::json!({"status": 1}));
        let slow = Arc::new(SlowTransport {
            inner: mock.clone(),
            delay: std::time::Duration::from_millis(20),
        });
        let api = Api::new(slow, session(), 64);
        let spec = |fileid: i64| {
            RequestSpec::post("/v4/delete_songs").body(serde_json::json!({"listid": "2", "data": [{"fileid": fileid}]}))
        };
        let (first, second) = futures::join!(
            api.call(spec(11), Freshness::NoStore),
            api.call(spec(22), Freshness::NoStore)
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(mock.count(), 2, "删不同的歌必须是两次请求");
        assert_eq!(api.coalesced(), 0, "不同请求体不能被合并");
    }

    #[tokio::test]
    async fn concurrent_identical_calls_are_coalesced() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json("/v1/slow", &serde_json::json!({"status": 1}));
        let slow = Arc::new(SlowTransport {
            inner: mock.clone(),
            delay: std::time::Duration::from_millis(20),
        });
        // 禁用缓存独立验证并发合并。
        let api = Api::new(slow, session(), 64);
        let futures = (0..6).map(|_| api.call(RequestSpec::get("/v1/slow"), Freshness::NoStore));
        let results = futures::future::join_all(futures).await;
        assert!(results.iter().all(std::result::Result::is_ok));
        assert_eq!(mock.count(), 1, "同 key 并发应被合并成一次");
        assert_eq!(api.coalesced(), 5);
    }

    #[test]
    fn fingerprint_is_generated_for_the_device() {
        let mock = Arc::new(MockTransport::new());
        let api = Api::new(mock, session(), 8);
        let (edt, sid) = api.fingerprint().unwrap();
        assert!(!edt.is_empty());
        assert!(!sid.is_empty());
        assert!(lyra_proto::base64_decode(&edt).is_ok());
    }
}
