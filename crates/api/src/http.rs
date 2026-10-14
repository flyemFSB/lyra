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
    /// 创建配置了 HTTP/2 自适应窗口与读超时的客户端实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                // 启用 HTTP/2 自适应窗口提升吞吐。
                .http2_adaptive_window(true)
                .read_timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// 将网络错误转换为传输错误并剔除 URL，防止令牌等敏感信息泄露。
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
                // 截断错误响应文本，避免大型 HTML 页面污染错误文案。
                body: crate::codec::truncate(&response.text().await.unwrap_or_default(), 512),
            });
        }
        // 上报期望内容长度以校验下载完整性。
        sink.expected_len(response.content_length());
        // 流式分块传输避免大文件占用额外内存。
        while let Some(chunk) = response.chunk().await.map_err(ReqwestTransport::transport_error)? {
            sink.chunk(&chunk);
        }
        Ok(())
    }
}
