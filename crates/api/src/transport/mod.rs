//! 传输抽象：解耦端点逻辑与底层 HTTP 实现，提供离线打桩能力。

pub mod mock;

pub use mock::MockTransport;

use std::time::Duration;

use async_trait::async_trait;

use crate::error::{Error, Result};

/// HTTP 方法（本协议只用到这两种）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET
    Get,
    /// POST
    Post,
}

impl Method {
    /// 大写形式，便于断言。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// 请求参数与元数据定义，查询参数暂不进行预编码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// HTTP 方法。
    pub method: Method,
    /// 不含 query 的完整 URL。
    pub url: String,
    /// 查询参数，保持插入顺序。
    pub query: Vec<(String, String)>,
    /// 请求头。
    pub headers: Vec<(String, String)>,
    /// 请求体原始字节（`None` 表示无体）。
    pub body: Option<Vec<u8>>,
    /// 超时。
    pub timeout: Option<Duration>,
}

impl Request {
    /// 新建一个请求。
    #[must_use]
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            timeout: None,
        }
    }

    /// 追加查询参数。
    #[must_use]
    pub fn query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((key.into(), value.into()));
        self
    }

    /// 追加请求头。
    #[must_use]
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// 设置请求体。
    #[must_use]
    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    /// 取某个请求头（大小写不敏感）。
    #[must_use]
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 拼接完整目标 URL 并对查询参数进行百分号编码。
    #[must_use]
    pub fn full_url(&self) -> String {
        if self.query.is_empty() {
            return self.url.clone();
        }
        match url::Url::parse(&self.url) {
            Ok(mut parsed) => {
                parsed
                    .query_pairs_mut()
                    .extend_pairs(self.query.iter().map(|(k, v)| (k, v)));
                parsed.to_string()
            }
            // 解析失败保留原始地址。
            Err(_) => self.url.clone(),
        }
    }

    /// 不参与缓存键计算的易变参数列表。
    const VOLATILE_QUERY: &'static [&'static str] = &["clienttime", "signature", "_lyra_t"];

    /// `host + path + 稳定 query`，用作缓存 key。
    #[must_use]
    pub fn cache_key(&self) -> String {
        match url::Url::parse(&self.url) {
            Ok(parsed) => {
                let mut key = String::from(parsed.host_str().unwrap_or_default());
                key.push_str(parsed.path());
                let mut pairs: Vec<String> = self
                    .query
                    .iter()
                    .filter(|(k, _)| !Self::VOLATILE_QUERY.contains(&k.as_str()))
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                if !pairs.is_empty() {
                    pairs.sort();
                    key.push('?');
                    key.push_str(&pairs.join("&"));
                }
                key
            }
            Err(_) => self.url.clone(),
        }
    }
}

/// 一次响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应头。
    pub headers: Vec<(String, String)>,
    /// 响应体原始字节。
    pub body: Vec<u8>,
}

impl Response {
    /// 构造响应。
    #[must_use]
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body,
        }
    }

    /// 构造 200 + JSON 响应。
    #[must_use]
    pub fn json_body(value: &serde_json::Value) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: value.to_string().into_bytes(),
        }
    }

    /// 取某个响应头（大小写不敏感）。
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 响应体按 UTF-8 解读（有损）。
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// 响应体解析为 JSON。
    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.body).map_err(|e| Error::Decode(format!("invalid json body: {e}")))
    }

    /// `ssa-code` 响应头：出现即表示需要行为验证。
    #[must_use]
    pub fn ssa_code(&self) -> Option<&str> {
        self.header("ssa-code").filter(|s| !s.is_empty())
    }
}

/// 流式抓取的分块接收端。
pub trait ChunkSink: Send {
    /// 收到一块数据。
    fn chunk(&mut self, bytes: &[u8]);
}

/// 把分块写进 `Vec`（测试 / 小响应用）。
#[derive(Debug, Default)]
pub struct VecSink(pub Vec<u8>);

impl ChunkSink for VecSink {
    fn chunk(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

/// HTTP 传输实现。
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// 执行一次请求。
    async fn execute(&self, request: Request) -> Result<Response>;

    /// 流式接收数据块至接收端对象。
    async fn execute_streaming(&self, request: Request, sink: &mut (dyn ChunkSink + Send)) -> Result<()> {
        let response = self.execute(request).await?;
        if !(200..300).contains(&response.status) {
            return Err(Error::Http {
                status: response.status,
                body: String::from_utf8_lossy(&response.body).into_owned(),
            });
        }
        sink.chunk(&response.body);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn cache_key_ignores_volatile_query_params() {
        // 易变参数不影响缓存条目判定。
        let a = Request::new(Method::Get, "https://gateway.kugou.com/v7/get_all_list")
            .query("userid", "1")
            .query("clienttime", "1750000000")
            .query("signature", "aaa");
        let b = Request::new(Method::Get, "https://gateway.kugou.com/v7/get_all_list")
            .query("userid", "1")
            .query("clienttime", "1750000999")
            .query("signature", "bbb");
        assert_eq!(a.cache_key(), b.cache_key());
        assert_eq!(a.cache_key(), "gateway.kugou.com/v7/get_all_list?userid=1");
    }

    #[test]
    fn cache_key_keeps_identity_params() {
        let a = Request::new(Method::Get, "https://g.kugou.com/v1/x").query("userid", "1");
        let b = Request::new(Method::Get, "https://g.kugou.com/v1/x").query("userid", "2");
        assert_ne!(a.cache_key(), b.cache_key(), "换账号不能共用缓存");
    }

    #[test]
    fn cache_key_is_order_insensitive() {
        let a = Request::new(Method::Get, "https://g.kugou.com/v1/x")
            .query("b", "2")
            .query("a", "1");
        let b = Request::new(Method::Get, "https://g.kugou.com/v1/x")
            .query("a", "1")
            .query("b", "2");
        assert_eq!(a.cache_key(), b.cache_key());
    }

    #[test]
    fn full_url_percent_encodes_the_query() {
        let request = Request::new(Method::Get, "https://g.kugou.com/v1/search")
            .query("keyword", "周杰伦 🎵")
            .query("page", "1");
        let url = request.full_url();
        assert!(url.starts_with("https://g.kugou.com/v1/search?"), "{url}");
        assert!(!url.contains(' '), "空格必须被编码: {url}");
        assert!(!url.contains("周"), "非 ASCII 必须被编码: {url}");
    }
}
