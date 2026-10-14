//! 请求参数有序表与签名格式化（对齐 JS 的 UTF-16 排序与渲染规则）。

use std::fmt::Write as _;

/// 单个参数值。
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    /// 标量：按原样参与拼接。
    Raw(String),
    /// 对象 / 数组：参与拼接前先序列化为紧凑 JSON。
    Json(serde_json::Value),
}

impl Val {
    /// 渲染为参与签名拼接的字符串。
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Raw(s) => s.clone(),
            Self::Json(v) => v.to_string(),
        }
    }

    /// 取标量视图；`Json` 返回 `None`。
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Raw(s) => Some(s),
            Self::Json(_) => None,
        }
    }
}

impl From<&str> for Val {
    fn from(v: &str) -> Self {
        Self::Raw(v.to_owned())
    }
}
impl From<&String> for Val {
    fn from(v: &String) -> Self {
        Self::Raw(v.clone())
    }
}
impl From<String> for Val {
    fn from(v: String) -> Self {
        Self::Raw(v)
    }
}
impl From<bool> for Val {
    fn from(v: bool) -> Self {
        Self::Raw(v.to_string())
    }
}
impl From<serde_json::Value> for Val {
    fn from(v: serde_json::Value) -> Self {
        Self::Json(v)
    }
}

macro_rules! val_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Val {
            fn from(v: $t) -> Self { Self::Raw(v.to_string()) }
        }
    )*};
}
val_from_int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// 保序参数表（重复 set 同一 key 会覆盖，与 JS 对象语义一致）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params {
    entries: Vec<(String, Val)>,
}

impl Params {
    /// 空表。
    #[must_use]
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// 写入 / 覆盖一个参数。
    pub fn set(&mut self, key: impl Into<String>, val: impl Into<Val>) -> &mut Self {
        let key = key.into();
        let val = val.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = val,
            None => self.entries.push((key, val)),
        }
        self
    }

    /// 读一个参数。
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// 读一个标量参数。
    #[must_use]
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Val::as_str)
    }

    /// 删除一个参数。
    pub fn remove(&mut self, key: &str) -> Option<Val> {
        let idx = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(idx).1)
    }

    /// 是否包含某参数。
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// 参数个数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 遍历 `(key, val)`。
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Val)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// 按 "k=v" 格式整串排序后拼接（供 Android 与 Web 签名使用）。
    #[must_use]
    pub fn sorted_kv(&self) -> String {
        let mut chunks: Vec<String> = self
            .entries
            .iter()
            .map(|(k, v)| {
                let mut s = String::with_capacity(k.len() + 1 + 8);
                let _ = write!(s, "{k}={}", v.render());
                s
            })
            .collect();
        sort_utf16(&mut chunks);
        chunks.concat()
    }

    /// 仅对参数值排序后拼接（供设备注册签名使用）。
    #[must_use]
    pub fn sorted_values(&self) -> String {
        let mut chunks: Vec<String> = self.entries.iter().map(|(_, v)| v.render()).collect();
        sort_utf16(&mut chunks);
        chunks.concat()
    }

    /// 展开为查询参数对（传输层负责百分号编码）。
    #[must_use]
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        self.entries.iter().map(|(k, v)| (k.clone(), v.render())).collect()
    }
}

impl FromIterator<(String, Val)> for Params {
    fn from_iter<T: IntoIterator<Item = (String, Val)>>(iter: T) -> Self {
        Self {
            entries: iter.into_iter().collect(),
        }
    }
}

/// 按 UTF-16 代码单元排序（对齐 JS Array#sort 默认字典序）。
fn sort_utf16(items: &mut [String]) {
    items.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use super::*;

    #[test]
    fn renders_scalars_like_js_template() {
        let mut p = Params::new();
        p.set("a", "x").set("n", 12_i64).set("f", false);
        assert_eq!(p.get("a").unwrap().render(), "x");
        assert_eq!(p.get("n").unwrap().render(), "12");
        assert_eq!(p.get("f").unwrap().render(), "false");
    }

    #[test]
    fn renders_objects_as_compact_json() {
        let mut p = Params::new();
        p.set("o", serde_json::json!({"k": 1, "s": "v"}));
        assert_eq!(p.get("o").unwrap().render(), r#"{"k":1,"s":"v"}"#);
    }

    #[test]
    fn set_overwrites_in_place() {
        let mut p = Params::new();
        p.set("a", "1").set("b", "2").set("a", "3");
        assert_eq!(p.len(), 2);
        assert_eq!(p.sorted_kv(), "a=3b=2");
    }

    #[test]
    fn sorts_whole_kv_string_not_key() {
        let mut p = Params::new();
        p.set("bb", "1").set("b", "2");
        // 字符串 "b=2" 排在 "bb=1" 之前（等号字符码 0x3D 小于字母 b 码值）
        assert_eq!(p.sorted_kv(), "b=2bb=1");
    }

    #[test]
    fn sorts_values_for_register_signature() {
        let mut p = Params::new();
        p.set("z", "bbb").set("a", "aaa").set("m", "ccc");
        assert_eq!(p.sorted_values(), "aaabbbccc");
    }
}
