//! 基于 reqwest 的 HTTP 传输层实现。

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::transport::{ChunkSink, Method, Request, Response, Transport};

/// 读超时（秒）：流式传输停滞检测的兜底
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
        match Self::try_new() {
            Ok(transport) => transport,
            Err(error) => {
                // 构建失败时退回到默认客户端：宁可失去 HTTP/2 也不能让传输层不可用。
                // 读超时是停滞检测的兜底，必须保住，因此退回时只保留读超时配置。
                tracing::warn!(%error, "构建 reqwest 客户端失败，退回最小配置");
                let client = reqwest::Client::builder()
                    .read_timeout(READ_TIMEOUT)
                    .build()
                    .unwrap_or_default();
                Self { client }
            }
        }
    }

    /// 创建预设超时的 HTTP 传输实例（构建失败时返回错误）
    pub fn try_new() -> Result<Self> {
        let client = reqwest::Client::builder()
            // 启用 HTTP/2 自适应流控窗口
            .http2_adaptive_window(true)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|error| Error::Transport(format!("构建 HTTP 客户端失败: {error}")))?;
        Ok(Self { client })
    }

    /// 转换网络错误并脱敏 URL
    fn transport_error(error: reqwest::Error) -> Error {
        Error::Transport(error.without_url().to_string())
    }

    /// 装配 reqwest 请求（请求体直接移交，不做整份拷贝）
    fn request(&self, request: Request) -> reqwest::RequestBuilder {
        let Request {
            method,
            url,
            query,
            headers,
            body,
            timeout,
        } = request;
        let method = match method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut builder = self.client.request(method, &url);
        if !query.is_empty() {
            builder = builder.query(&query);
        }
        for (key, value) in &headers {
            builder = builder.header(key, value);
        }
        if let Some(body) = body {
            builder = builder.body(body);
        }
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        builder
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn execute(&self, request: Request) -> Result<Response> {
        let response = self
            .request(request)
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
            .request(request)
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
