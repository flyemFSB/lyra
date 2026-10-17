//! 基于 reqwest 的 HTTP 传输层实现。

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::transport::{ChunkSink, Method, Request, Response, Transport};

/// Reqwest HTTP 传输层
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
    /// 创建预设超时的 HTTP 传输实例
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                // 启用 HTTP/2 自适应流控窗口
                .http2_adaptive_window(true)
                .read_timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// 转换网络错误并脱敏 URL
    fn transport_error(error: reqwest::Error) -> Error {
        Error::Transport(error.without_url().to_string())
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
            .map_err(ReqwestTransport::transport_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(key, value)| (key.as_str().to_owned(), value.to_str().unwrap_or_default().to_owned()))
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(ReqwestTransport::transport_error)?
            .to_vec();
        Ok(Response { status, headers, body })
    }

    async fn execute_streaming(&self, request: Request, sink: &mut (dyn ChunkSink + Send)) -> Result<()> {
        let mut response = self
            .request(&request)
            .send()
            .await
            .map_err(ReqwestTransport::transport_error)?;
        if !response.status().is_success() {
            return Err(Error::Http {
                status: response.status().as_u16(),
                // 截断异常响应体
                body: crate::codec::truncate(&response.text().await.unwrap_or_default(), 512),
            });
        }
        // 上报声明的内容总长度
        sink.expected_len(response.content_length());
        // 流式分块读取并写入接收器
        while let Some(chunk) = response.chunk().await.map_err(ReqwestTransport::transport_error)? {
            sink.chunk(&chunk);
        }
        Ok(())
    }
}
