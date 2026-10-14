//! 基于 reqwest 的 HTTP 传输实现，供 CLI 与自动化测试使用。

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::transport::{ChunkSink, Method, Request, Response, Transport};

/// 基于 `reqwest` 的传输实现。
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestTransport {
    /// 创建客户端实例并配置自适应连接池。
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                // 启用 HTTP/2 自适应窗口提升吞吐。
                .http2_adaptive_window(true)
                .build()
                .unwrap_or_default(),
        }
    }

    fn request(&self, request: &Request) -> reqwest::RequestBuilder {
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut builder = self.client.request(method, &request.url);
        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }
        for (key, value) in &request.headers {
            builder = builder.header(key, value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        if let Some(timeout) = request.timeout {
            builder = builder.timeout(timeout);
        }
        builder
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn execute(&self, request: Request) -> Result<Response> {
        let response = self
            .request(&request)
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(key, value)| (key.as_str().to_owned(), value.to_str().unwrap_or_default().to_owned()))
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?
            .to_vec();
        Ok(Response { status, headers, body })
    }

    async fn execute_streaming(&self, request: Request, sink: &mut (dyn ChunkSink + Send)) -> Result<()> {
        let mut response = self
            .request(&request)
            .send()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;
        if !response.status().is_success() {
            return Err(Error::Http {
                status: response.status().as_u16(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        // 流式分块传输避免大文件占用额外内存。
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| Error::Transport(error.to_string()))?
        {
            sink.chunk(&chunk);
        }
        Ok(())
    }
}
