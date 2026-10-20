//! 传输抽象层：解耦端点逻辑与底层 HTTP 实现。

pub mod mock;

pub use mock::MockTransport;

use std::time::Duration;

use async_trait::async_trait;

use crate::error::{Error, Result};

/// HTTP 请求方法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET 方法
    Get,
    /// POST 方法
    Post,
}

impl Method {
    /// 获取方法名的大写字符串
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// 抽象 HTTP 请求
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 请求方法
    pub method: Method,
    /// 目标 URL（不含查询参数）
    pub url: String,
    /// 查询参数键值对
    pub query: Vec<(String, String)>,
    /// 请求头列表
    pub headers: Vec<(String, String)>,
    /// 请求体原始字节
    pub body: Option<Vec<u8>>,
    /// 超时时长
    pub timeout: Option<Duration>,
}

impl Request {
    /// 创建新请求实例
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

    /// 添加查询参数
    #[must_use]
    pub fn query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((key.into(), value.into()));
        self
    }

    /// 添加请求头
    #[must_use]
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// 设置请求体
    #[must_use]
    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    /// 获取指定请求头（大小写无关）
    #[must_use]
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 拼接完整 URL 并对查询参数编码
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
            // 解析失败保留原始地址
            Err(_) => self.url.clone(),
        }
    }

    /// 易变查询参数白名单（不参与缓存键计算）
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

    /// 易变请求体字段白名单（不参与缓存键计算）
    const VOLATILE_BODY: &'static [&'static str] = &[
        "clienttime",
        "clienttime_ms",
        "visit_time",
        "last_time",
        "key",
        "p",
        "_lyra_t",
    ];

    /// 缓存键分段分隔符（ASCII 单元分隔符）
    ///
    /// URL 解析后的路径与百分号编码后的查询值都不会出现该字符，
    /// 因此分段之间不存在歧义。
    const KEY_SEP: char = '\u{1f}';

    /// 计算规范化请求缓存键
    ///
    /// 键由四段组成：HTTP 方法、主机与路径、规范化查询串、请求体摘要。
    /// 方法必须参与计算：同一路径的 GET 与 POST 不是同一个资源。
    #[must_use]
    pub fn cache_key(&self) -> String {
        let (host_path, query) = match url::Url::parse(&self.url) {
            Ok(parsed) => {
                let mut host_path = String::from(parsed.host_str().unwrap_or_default());
                host_path.push_str(parsed.path());
                (host_path, Self::encoded_query(&self.query))
            }
            Err(_) => (self.url.clone(), Self::encoded_query(&self.query)),
        };
        let digest = self.body.as_deref().map_or_else(String::new, Self::body_digest);
        [
            self.method.as_str(),
            host_path.as_str(),
            query.as_str(),
            digest.as_str(),
        ]
        .join(&Self::KEY_SEP.to_string())
    }

    /// 规范化查询串：剔除易变字段、排序、百分号编码
    ///
    /// 百分号编码保证参数值里的 `&` / `=` 不会与参数分隔符混淆。
    fn encoded_query(query: &[(String, String)]) -> String {
        let mut pairs: Vec<(&str, &str)> = query
            .iter()
            .filter(|(k, _)| !Self::VOLATILE_QUERY.contains(&k.as_str()))
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        pairs.sort_unstable();
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        serializer.finish()
    }

    /// 计算请求体 SHA-256 摘要（剔除易变字段）
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

/// 抽象 HTTP 响应
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP 状态码
    pub status: u16,
    /// 响应头列表
    pub headers: Vec<(String, String)>,
    /// 响应体原始字节
    pub body: Vec<u8>,
}

impl Response {
    /// 创建响应实例
    #[must_use]
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body,
        }
    }

    /// 创建 200 状态码的 JSON 响应
    #[must_use]
    pub fn json_body(value: &serde_json::Value) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: value.to_string().into_bytes(),
        }
    }

    /// 获取指定响应头（大小写无关）
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 响应体转换为 UTF-8 字符串
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// 获取二次行为验证头 ssa-code
    #[must_use]
    pub fn ssa_code(&self) -> Option<&str> {
        self.header("ssa-code").filter(|s| !s.is_empty())
    }
}

/// 流式数据分块接收器
pub trait ChunkSink: Send {
    /// 接收上报的内容总长度（Content-Length）
    fn expected_len(&mut self, _len: Option<u64>) {}

    /// 写入数据分块
    fn chunk(&mut self, bytes: &[u8]);
}

/// HTTP 传输层抽象特征
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// 执行单次 HTTP 请求
    async fn execute(&self, request: Request) -> Result<Response>;

    /// 执行流式传输并写入接收器
    async fn execute_streaming(&self, request: Request, sink: &mut (dyn ChunkSink + Send)) -> Result<()> {
        let response = self.execute(request).await?;
        if !(200..300).contains(&response.status) {
            return Err(Error::Http {
                status: response.status,
                // 截断上游异常响应体
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
        assert_eq!(a.cache_key(), b.cache_key(), "时间流逝后键必须稳定");
        // 键的构成：方法 \x1f 主机与路径 \x1f 查询串 \x1f 请求体摘要
        let expected = ["GET", "gateway.kugou.com/v7/get_all_list", "userid=1", ""].join("\u{1f}");
        assert_eq!(a.cache_key(), expected);
    }

    #[test]
    fn cache_key_distinguishes_ambiguous_query_parameters() {
        // 旧实现直接拼 `k=v`，参数值里的 `&` / `=` 会与分隔符混淆。
        let a = Request::new(Method::Get, "https://g.kugou.com/v1/x")
            .query("a", "1")
            .query("b", "2");
        let b = Request::new(Method::Get, "https://g.kugou.com/v1/x").query("a", "1&b=2");
        assert_ne!(a.cache_key(), b.cache_key(), "参数值不得与参数分隔符混淆");
    }

    #[test]
    fn cache_key_includes_the_http_method() {
        // 同一路径的 GET 与 POST 不是同一个资源，不能共用缓存条目。
        let get = Request::new(Method::Get, "https://g.kugou.com/v1/x").query("userid", "1");
        let post = Request::new(Method::Post, "https://g.kugou.com/v1/x").query("userid", "1");
        assert_ne!(get.cache_key(), post.cache_key(), "方法必须参与缓存键");
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
