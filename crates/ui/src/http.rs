//! GPUI 网络客户端桥接：基于 reqwest 与 Tokio 运行时实现 GPUI 的 HttpClient 接口。

use futures::future::BoxFuture;
use gpui_kit::http_client::http::HeaderValue;
use gpui_kit::http_client::{AsyncBody, HttpClient, Request, Response, Url};

/// 基于共享 Tokio 运行时与 reqwest 的 HTTP 客户端
pub(crate) struct LyraHttpClient {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
}

impl LyraHttpClient {
    /// 构造新的客户端实例
    pub(crate) fn new(runtime: tokio::runtime::Handle) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client, runtime }
    }
}

impl HttpClient for LyraHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let runtime = self.runtime.clone();

        Box::pin(async move {
            let (parts, _body) = req.into_parts();
            let uri = parts.uri.to_string();
            let method = parts.method;

            // 在 Tokio 工作线程池中安全执行异步 HTTP 请求，避免 GPUI 执行器缺少 Tokio Reactor 上下文
            let response_task = runtime.spawn(async move {
                let reqwest_method = match method.as_str() {
                    "GET" => reqwest::Method::GET,
                    "POST" => reqwest::Method::POST,
                    "HEAD" => reqwest::Method::HEAD,
                    other => reqwest::Method::from_bytes(other.as_bytes())
                        .map_err(|e| anyhow::anyhow!("无效 HTTP 方法: {e}"))?,
                };

                let mut req_builder = client.request(reqwest_method, &uri);
                for (name, value) in &parts.headers {
                    if let (Ok(h_name), Ok(h_val)) = (
                        reqwest::header::HeaderName::from_bytes(name.as_ref()),
                        reqwest::header::HeaderValue::from_bytes(value.as_ref()),
                    ) {
                        req_builder = req_builder.header(h_name, h_val);
                    }
                }

                let resp = req_builder
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("网络请求失败 ({uri}): {e}"))?;

                let status_u16 = resp.status().as_u16();
                let status = gpui_kit::http_client::StatusCode::from_u16(status_u16)
                    .map_err(|e| anyhow::anyhow!("无效状态码: {e}"))?;

                let mut response_builder = gpui_kit::http_client::Response::builder().status(status);
                for (name, value) in resp.headers() {
                    if let (Ok(h_name), Ok(h_val)) = (
                        gpui_kit::http_client::http::HeaderName::from_bytes(name.as_ref()),
                        gpui_kit::http_client::http::HeaderValue::from_bytes(value.as_ref()),
                    ) {
                        response_builder = response_builder.header(h_name, h_val);
                    }
                }

                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| anyhow::anyhow!("读取图片流失败 ({uri}): {e}"))?;
                let body = AsyncBody::from_bytes(bytes);
                let response = response_builder
                    .body(body)
                    .map_err(|e| anyhow::anyhow!("构造响应失败: {e}"))?;
                Ok::<_, anyhow::Error>(response)
            });

            match response_task.await {
                Ok(result) => result,
                Err(join_err) => Err(anyhow::anyhow!("Tokio 任务调度异常: {join_err}")),
            }
        })
    }
}
