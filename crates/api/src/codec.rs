//! 响应宽容解码：字段类型归一化与有界键名检索。

use std::borrow::Cow;
use std::collections::VecDeque;

use serde_json::Value;

use crate::error::{Error, Result};

/// 递归搜索的最大深度限制
const MAX_DEPTH: u8 = 4;

/// 宽容解析为 64 位有符号整数
#[must_use]
pub fn as_i64(value: &Value) -> i64 {
    match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .or_else(|_| s.trim().parse::<f64>().map(|f| f as i64))
            .unwrap_or(0),
        Value::Bool(b) => i64::from(*b),
        _ => 0,
    }
}

/// 宽容解析为 64 位无符号整数
#[must_use]
pub fn as_u64(value: &Value) -> u64 {
    u64::try_from(as_i64(value)).unwrap_or(0)
}

/// 宽容解析为字符串
#[must_use]
pub fn as_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 宽容解析为字符串（借用优先，字符串字段零拷贝）
///
/// 热路径（逐首歌曲解析）用它替代 [`as_str`]，避免为每个字段多分配一次 `String`。
#[must_use]
pub fn as_str_cow(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(s) => Cow::Borrowed(s.as_str()),
        Value::Number(n) => Cow::Owned(n.to_string()),
        Value::Bool(b) => Cow::Owned(b.to_string()),
        Value::Null => Cow::Borrowed(""),
        other => Cow::Owned(other.to_string()),
    }
}

/// 宽容解析为布尔值
#[must_use]
pub fn as_bool(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        Value::String(s) => matches!(s.as_str(), "true" | "1" | "yes"),
        _ => false,
    }
}

/// 按候选键名优先提取字段（大小写无关）
#[must_use]
pub fn pick<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let map = value.as_object()?;
    for key in keys {
        if let Some(found) = map.get(*key) {
            return Some(found);
        }
    }
    for (k, v) in map {
        if keys.iter().any(|want| k.eq_ignore_ascii_case(want)) {
            return Some(v);
        }
    }
    None
}

/// 按候选键名提取字符串
#[must_use]
pub fn pick_str(value: &Value, keys: &[&str]) -> String {
    pick(value, keys).map(as_str).unwrap_or_default()
}

/// 按候选键名提取字符串（借用优先）
#[must_use]
pub fn pick_str_cow<'a>(value: &'a Value, keys: &[&str]) -> Cow<'a, str> {
    pick(value, keys).map_or(Cow::Borrowed(""), as_str_cow)
}

/// 按候选键名提取 64 位有符号整数
#[must_use]
pub fn pick_i64(value: &Value, keys: &[&str]) -> i64 {
    pick(value, keys).map(as_i64).unwrap_or_default()
}

/// 按候选键名提取 64 位无符号整数
#[must_use]
pub fn pick_u64(value: &Value, keys: &[&str]) -> u64 {
    pick(value, keys).map(as_u64).unwrap_or_default()
}

/// 按多级路径提取字段
#[must_use]
pub fn pick_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut node = value;
    for segment in path {
        node = pick(node, &[*segment])?;
    }
    Some(node)
}

/// 按候选多级路径列表提取首个匹配字符串
#[must_use]
pub fn pick_str_paths(value: &Value, paths: &[&[&str]]) -> String {
    paths
        .iter()
        .find_map(|path| pick_path(value, path))
        .map(as_str)
        .unwrap_or_default()
}

/// 按候选多级路径列表提取首个匹配字符串（借用优先）
#[must_use]
pub fn pick_str_paths_cow<'a>(value: &'a Value, paths: &[&[&str]]) -> Cow<'a, str> {
    paths
        .iter()
        .find_map(|path| pick_path(value, path))
        .map_or(Cow::Borrowed(""), as_str_cow)
}

/// 按候选多级路径列表提取首个匹配整数
#[must_use]
pub fn pick_i64_paths(value: &Value, paths: &[&[&str]]) -> i64 {
    paths
        .iter()
        .find_map(|path| pick_path(value, path))
        .map(as_i64)
        .unwrap_or_default()
}

/// 按候选多级路径列表提取首个匹配无符号整数
#[must_use]
pub fn pick_u64_paths(value: &Value, paths: &[&[&str]]) -> u64 {
    paths
        .iter()
        .find_map(|path| pick_path(value, path))
        .map(as_u64)
        .unwrap_or_default()
}

/// 广度优先遍历 JSON 树（受最大深度约束）
fn walk(root: &Value) -> Vec<(&str, &Value)> {
    let mut found = Vec::new();
    let mut queue = VecDeque::from([(root, 0_u8)]);
    while let Some((node, depth)) = queue.pop_front() {
        if depth > MAX_DEPTH {
            continue;
        }
        match node {
            Value::Object(map) => {
                for (key, child) in map {
                    found.push((key.as_str(), child));
                    queue.push_back((child, depth + 1));
                }
            }
            Value::Array(items) => {
                for child in items {
                    queue.push_back((child, depth + 1));
                }
            }
            _ => {}
        }
    }
    found
}

fn matches(key: &str, keys: &[&str]) -> bool {
    keys.iter().any(|want| key.eq_ignore_ascii_case(want))
}

/// 遍历查找首个匹配键名的字段
#[must_use]
pub fn find<'a>(root: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    walk(root)
        .into_iter()
        .find(|(key, _)| matches(key, keys))
        .map(|(_, value)| value)
}

/// 遍历查找首个匹配键名的字符串
#[must_use]
pub fn find_str(root: &Value, keys: &[&str]) -> String {
    find(root, keys).map(as_str).unwrap_or_default()
}

/// 遍历查找首个匹配键名的 64 位整数
#[must_use]
pub fn find_i64(root: &Value, keys: &[&str]) -> i64 {
    find(root, keys).map(as_i64).unwrap_or_default()
}

/// 遍历查找首个匹配键名的 64 位无符号整数
#[must_use]
pub fn find_u64(root: &Value, keys: &[&str]) -> u64 {
    find(root, keys).map(as_u64).unwrap_or_default()
}

/// 遍历查找首个匹配键名的数组
#[must_use]
pub fn find_array<'a>(root: &'a Value, keys: &[&str]) -> Option<&'a Vec<Value>> {
    walk(root).into_iter().find_map(|(key, value)| match value {
        Value::Array(items) if matches(key, keys) => Some(items),
        _ => None,
    })
}

/// 提取上游响应中的错误提示文本
#[must_use]
pub fn error_message(value: &Value) -> String {
    let message = pick_str(
        value,
        &["msg", "error", "errmsg", "message", "error_msg", "errorMessage"],
    );
    // 缺省时截断原始响应体作为错误描述
    if message.is_empty() {
        truncate(&value.to_string(), 200)
    } else {
        message
    }
}

/// 解密会话响应体（解密失败时降级为明文）
#[must_use]
pub fn decode_session_body(cipher: &[u8], secret: &str) -> String {
    lyra_proto::secret_decrypt_bytes(cipher, secret)
        .ok()
        .filter(|bytes| !bytes.is_empty())
        .map_or_else(
            || String::from_utf8_lossy(cipher).into_owned(),
            |bytes| String::from_utf8_lossy(&bytes).into_owned(),
        )
}

/// 检查上游业务状态码
pub fn check_business(value: &Value) -> Result<()> {
    let status_failed = pick(value, &["status"]).is_some_and(|status| as_i64(status) == 0);
    let code_failed = pick(value, &["error_code"]).is_some_and(|code| as_i64(code) != 0);
    if !status_failed && !code_failed {
        return Ok(());
    }
    Err(Error::Upstream {
        code: pick_i64(value, &["error_code"]),
        message: error_message(value),
    })
}

/// 校验 HTTP 状态码并反序列化上游 JSON 响应
pub fn check_response(status: u16, body: &[u8]) -> Result<Value> {
    if !(200..300).contains(&status) {
        return Err(Error::Http {
            status,
            body: truncate(&String::from_utf8_lossy(body), 512),
        });
    }
    let value: Value = serde_json::from_slice(body).map_err(|e| {
        Error::Decode(format!(
            "invalid json body: {e}; head={}",
            truncate(&String::from_utf8_lossy(body), 200)
        ))
    })?;
    check_business(&value)?;
    Ok(value)
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;
    use serde_json::json;

    #[test]
    fn session_body_falls_back_to_plaintext() {
        let secret = "abc123";
        // 成功响应是加密的：正常解密。
        let encrypted = lyra_proto::secret_encrypt_with(r#"{"status":1,"data":{"dfid":"X"}}"#, secret).unwrap();
        let cipher = lyra_proto::base64_decode(&encrypted.base64).unwrap();
        assert!(decode_session_body(&cipher, secret).contains("dfid"));
        // 失败响应是明文 JSON（长度也不对齐），必须原样返回而不是报解密错误。
        let plain = br#"{"data":null,"status":0,"error_code":20006}"#;
        assert_eq!(decode_session_body(plain, secret), String::from_utf8_lossy(plain));
    }

    #[test]
    fn coerces_numbers_and_strings() {
        assert_eq!(as_i64(&json!(12)), 12);
        assert_eq!(as_i64(&json!("12")), 12);
        assert_eq!(as_i64(&json!(12.9)), 12);
        assert_eq!(as_u64(&json!("7")), 7);
        assert_eq!(as_str(&json!(9)), "9");
        assert!(as_bool(&json!("true")));
        assert!(as_bool(&json!(1)));
        assert!(!as_bool(&json!("false")));
    }

    #[test]
    fn picks_by_candidate_keys_case_insensitively() {
        let v = json!({"SongName": "x", "timelen": "3000"});
        assert_eq!(pick_str(&v, &["songname"]), "x");
        assert_eq!(pick_i64(&v, &["duration", "timelen"]), 3000);
    }

    #[test]
    fn picks_by_path() {
        let v = json!({"trans_param": {"union_cover": "http://c/1.jpg"}});
        assert_eq!(pick_str_paths(&v, &[&["trans_param", "union_cover"]]), "http://c/1.jpg");
        assert_eq!(
            pick_str_paths(&v, &[&["img"], &["trans_param", "union_cover"]]),
            "http://c/1.jpg"
        );
    }

    #[test]
    fn finds_arrays_at_any_depth_by_candidate_key() {
        let v = json!({"data": {"song_list": [1, 2, 3]}});
        assert_eq!(find_array(&v, &["song_list", "songs"]).unwrap().len(), 3);
        let nested = json!({"data": {"info": {"songs": [{"hash": "a"}]}}});
        assert_eq!(find_array(&nested, &["songs"]).unwrap().len(), 1);
        assert!(find_array(&json!({"data": {}}), &["songs"]).is_none());
    }

    #[test]
    fn honours_depth_limit() {
        let deep = json!({"a": {"b": {"c": {"d": {"e": {"songs": [1]}}}}}});
        assert!(find_array(&deep, &["songs"]).is_none());
    }

    #[test]
    fn business_failure_on_zero_status() {
        let err = check_business(&json!({"status": 0, "msg": "boom"})).unwrap_err();
        assert!(matches!(err, Error::Upstream { code: 0, .. }));
        assert!(check_business(&json!({"status": "0"})).is_err());
    }

    #[test]
    fn missing_status_is_not_a_failure() {
        // 歌词服务等接口缺省 status 时不应误判为业务失败。
        assert!(check_business(&json!({"candidates": [{"id": 1, "accesskey": "k"}]})).is_ok());
        assert!(check_business(&json!({"error_code": 0, "content": "x"})).is_ok());
        assert!(check_business(&json!({"content": "x", "contenttype": 0})).is_ok());
        assert!(check_business(&json!({})).is_ok());
        // 显式 status=0 仍判定为失败。
        assert!(check_business(&json!({"candidates": [], "status": 0})).is_err());
    }

    #[test]
    fn business_failure_on_non_zero_error_code() {
        let err = check_business(&json!({"status": 1, "error_code": 20028, "error": "验证"})).unwrap_err();
        assert_eq!(err.code(), Some(crate::error::Code::NeedVerify));
        assert!(check_business(&json!({"status": 1, "error_code": 0})).is_ok());
        assert!(check_business(&json!({"status": 1})).is_ok());
    }

    #[test]
    fn http_status_is_rejected() {
        let err = check_response(502, b"nope").unwrap_err();
        assert!(matches!(err, Error::Http { status: 502, .. }));
    }
}
