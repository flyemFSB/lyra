//! 模拟测试传输层实现。

use async_trait::async_trait;
use parking_lot::Mutex;

use super::{ChunkSink, Request, Response, Transport};
use crate::error::{Error, Result};

/// 路由打桩规则
#[derive(Debug, Clone)]
struct Route {
    /// 匹配子串（URL 或路径包含即命中）
    needle: String,
    response: Response,
}

/// 内存 Mock 传输实现
#[derive(Debug, Default)]
pub struct MockTransport {
    routes: Mutex<Vec<Route>>,
    seen: Mutex<Vec<Request>>,
}

impl MockTransport {
    /// 创建空 Mock 传输实例
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册桩应答规则
    pub fn on(&self, needle: impl Into<String>, response: Response) -> &Self {
        self.routes.lock().push(Route {
            needle: needle.into(),
            response,
        });
        self
    }

    /// 注册 JSON 桩应答
    pub fn on_json(&self, needle: impl Into<String>, body: &serde_json::Value) -> &Self {
        self.on(needle, Response::json_body(body))
    }

    /// 注册携带 ssa-code 验证头的桩应答
    pub fn on_ssa(&self, needle: impl Into<String>, ssa_code: &str, body: &serde_json::Value) -> &Self {
        let mut response = Response::json_body(body);
        response.headers.push(("ssa-code".to_owned(), ssa_code.to_owned()));
        self.on(needle, response)
    }

    /// 获取已捕获的全部请求列表
    #[must_use]
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().clone()
    }

    /// 获取最近捕获的一次请求
    #[must_use]
    pub fn last_request(&self) -> Option<Request> {
        self.seen.lock().last().cloned()
    }

    /// 获取捕获的请求总数
    #[must_use]
    pub fn count(&self) -> usize {
        self.seen.lock().len()
    }
}

#[async_trait]
impl Transport for MockTransport {
    async fn execute(&self, request: Request) -> Result<Response> {
        let url = request.full_url();
        self.seen.lock().push(request);
        let found = self
            .routes
            .lock()
            .iter()
            .rev()
            .find(|route| url.contains(&route.needle))
            .map(|route| route.response.clone());
        found.ok_or_else(|| Error::Transport(format!("MockTransport 未注册匹配 {url} 的桩应答")))
    }
}

#[async_trait]
impl Transport for std::sync::Arc<MockTransport> {
    async fn execute(&self, request: Request) -> Result<Response> {
        (**self).execute(request).await
    }

    async fn execute_streaming(&self, request: Request, sink: &mut (dyn ChunkSink + Send)) -> Result<()> {
        (**self).execute_streaming(request, sink).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::transport::Method;

    #[tokio::test]
    async fn matches_by_substring_and_records_requests() {
        let mock = MockTransport::new();
        mock.on_json("/v1/class_fm_song", &serde_json::json!({"status": 1}));
        let transport = std::sync::Arc::new(mock);
        let request = Request::new(Method::Post, "https://gateway.kugou.com/v1/class_fm_song").query("mid", "42");
        let response = transport.execute(request).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(transport.count(), 1);
        assert_eq!(transport.last_request().unwrap().get_header("mid"), None);
    }

    #[tokio::test]
    async fn unmatched_route_is_an_error() {
        let transport = std::sync::Arc::new(MockTransport::new());
        let err = transport
            .execute(Request::new(Method::Get, "https://gateway.kugou.com/nope"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Transport(_)));
    }
}
