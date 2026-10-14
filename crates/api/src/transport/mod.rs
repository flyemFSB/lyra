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

    /// 不参与缓存键计算的易变查询参数列表。
    const VOLATILE_QUERY: &'static [&'static str] = &[
        "clienttime",
        "clienttime_ms",
        "visit_time",
        "last_time",
        "key",
        "p",
        "signature",
        "_lyra_t",
    ];

    /// 不参与缓存键计算的易变请求体字段列表。
    const VOLATILE_BODY: &'static [&'static str] = &[
        "clienttime",
        "clienttime_ms",
        "visit_time",
        "last_time",
        "key",
        "p",
        "_lyra_t",
    ];

    /// 生成包含主机、路径、稳定查询参数与请求体摘要的唯一缓存键。
    #[must_use]
    pub fn cache_key(&self) -> String {
        let mut key = match url::Url::parse(&self.url) {
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
        };
        if let Some(body) = &self.body {
            key.push('#');
            key.push_str(&Self::body_digest(body));
        }
        key
    }

    /// 剔除易变字段后的请求体摘要（非 JSON 体直接哈希原始字节）。
    fn body_digest(body: &[u8]) -> String {
        let normalized = match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(serde_json::Value::Object(mut map)) => {
                map.retain(|key, _| !Self::VOLATILE_BODY.contains(&key.as_str()));
                serde_json::Value::Object(map).to_string().into_bytes()
            }
            _ => body.to_vec(),
        };
        lyra_proto::sha256_hex(&normalized)
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

    /// `ssa-code` 响应头：出现即表示需要行为验证。
    #[must_use]
    pub fn ssa_code(&self) -> Option<&str> {
        self.header("ssa-code").filter(|s| !s.is_empty())
    }
}

/// 流式抓取的分块接收端。
pub trait ChunkSink: Send {
    /// 收到响应体长度声明（`Content-Length`），供接收端校验下载是否完整。
    fn expected_len(&mut self, _len: Option<u64>) {}

    /// 收到一块数据。
    fn chunk(&mut self, bytes: &[u8]);
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
                // 与 http 实现保持同一条界：上游错误体可能是一整页 HTML。
                body: crate::codec::truncate(&String::from_utf8_lossy(&response.body), 512),
            });
        }
        sink.expected_len(Some(response.body.len() as u64));
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
    fn cache_key_includes_the_request_body() {
        // 同一端点、同一 query，只有请求体不同（点第二首歌 / 删第二首）必须得到不同键。
        let a = Request::new(Method::Post, "https://g.kugou.com/v2/get_res_privilege/lite")
            .query("userid", "1")
            .body(
                serde_json::json!({"resource": [{"hash": "a"}]})
                    .to_string()
                    .into_bytes(),
            );
        let b = Request::new(Method::Post, "https://g.kugou.com/v2/get_res_privilege/lite")
            .query("userid", "1")
            .body(
                serde_json::json!({"resource": [{"hash": "b"}]})
                    .to_string()
                    .into_bytes(),
            );
        assert_ne!(a.cache_key(), b.cache_key(), "换歌曲不能共用缓存");
        assert_eq!(a.cache_key(), a.clone().cache_key(), "同请求必须稳定");
    }

    #[test]
    fn cache_key_ignores_volatile_body_fields() {
        let build = |clienttime: i64| {
            Request::new(Method::Post, "https://g.kugou.com/v2/personal_recommend").body(
                serde_json::json!({"clienttime": clienttime, "key": format!("k{clienttime}"), "mode": "normal"})
                    .to_string()
                    .into_bytes(),
            )
        };
        assert_eq!(build(1).cache_key(), build(2).cache_key());
    }

    #[test]
    fn cache_key_keeps_identity_params() {
        let a = Request::new(Method::Get, "https://g.kugou.com/v1/x").query("userid", "1");
        let b = Request::new(Method::Get, "https://g.kugou.com/v1/x").query("userid", "2");
        assert_ne!(a.cache_key(), b.cache_key(), "换账号不能共用缓存");
        // 删歌的身份在 body 的 fileid 上。
        let c = Request::new(Method::Post, "https://g.kugou.com/v4/delete_songs")
            .body(serde_json::json!({"data": [{"fileid": 1}]}).to_string().into_bytes());
        let d = Request::new(Method::Post, "https://g.kugou.com/v4/delete_songs")
            .body(serde_json::json!({"data": [{"fileid": 2}]}).to_string().into_bytes());
        assert_ne!(c.cache_key(), d.cache_key(), "删不同歌曲不能共用单飞键");
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
