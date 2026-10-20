//! 请求参数映射表与签名序列化（对齐 JS UTF-16 排序）。

use std::borrow::Cow;

use smallvec::SmallVec;

/// 签名热路径的参数容量（方案 §5.1：小参数表直接内联在栈上）
type ParamPairs<'a> = SmallVec<[(&'a str, Cow<'a, str>); 16]>;

/// 参数值类型
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    /// 字符串标量
    Raw(String),
    /// JSON 对象或数组
    Json(serde_json::Value),
}

impl Val {
    /// 序列化为签名用字符串
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Raw(s) => s.clone(),
            Self::Json(v) => v.to_string(),
        }
    }

    /// 获取标量字符串切片
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Raw(s) => Some(s),
            Self::Json(_) => None,
        }
    }

    /// 借用形式的渲染结果：标量零拷贝，JSON 才需要序列化。
    fn render_cow(&self) -> Cow<'_, str> {
        match self {
            Self::Raw(s) => Cow::Borrowed(s.as_str()),
            Self::Json(v) => Cow::Owned(v.to_string()),
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

/// 保序参数表（重复 key 自动覆盖）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params {
    entries: Vec<(String, Val)>,
}

impl Params {
    /// 创建空参数表
    #[must_use]
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// 设置参数（存在则覆盖）
    pub fn set(&mut self, key: impl Into<String>, val: impl Into<Val>) -> &mut Self {
        let key = key.into();
        let val = val.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = val,
            None => self.entries.push((key, val)),
        }
        self
    }

    /// 获取参数值
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// 获取标量参数值
    #[must_use]
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Val::as_str)
    }

    /// 是否包含指定参数
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// 参数数量
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 迭代所有参数键值对
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Val)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// 拼接 "k=v" 后**整串**排序（Web 签名规则）。
    ///
    /// 与 [`Params::sorted_kv_by_key`] 只在「某个 key 是另一个 key 的严格前缀，
    /// 且后一位是数字」时不同（`hash` 与 `hash2` 即是此例：整串排序时
    /// `'2'(0x32) < '='(0x3D)`，于是 `hash2=…` 排在 `hash=…` 之前）。
    ///
    /// ⚠️ 两种规则的差异**无法**由既有黄金向量区分（所有向量参数集里都没有
    /// 前缀 + 数字的键对），方案 §5.1 的文字「参数按 key 字典序」更接近
    /// `sorted_kv_by_key`。在拿到参考实现的确凿证据之前，这里保持既有行为不变，
    /// 并由 `crates/proto/tests/vectors.rs` 的 `signature_*_prefix_keys_*` 向量
    /// 把当前行为钉死，避免两种实现被无意互换。
    #[must_use]
    pub fn sorted_kv(&self) -> String {
        let mut chunks: SmallVec<[String; 16]> = self
            .entries
            .iter()
            .map(|(k, v)| {
                let value = v.render_cow();
                let mut s = String::with_capacity(k.len() + 1 + value.len());
                s.push_str(k);
                s.push('=');
                s.push_str(&value);
                s
            })
            .collect();
        sort_utf16(&mut chunks);
        let mut out = String::with_capacity(chunks.iter().map(String::len).sum());
        for chunk in &chunks {
            out.push_str(chunk);
        }
        out
    }

    /// 按 key 排序后拼接键值对（Android 签名规则，方案 §5.1 的默认规则）。
    ///
    /// 与 [`Params::sorted_kv`] 的差异见该函数的说明。
    #[must_use]
    pub fn sorted_kv_by_key(&self) -> String {
        let mut pairs: ParamPairs<'_> = self.entries.iter().map(|(k, v)| (k.as_str(), v.render_cow())).collect();
        pairs.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
        let mut out = String::with_capacity(pairs.iter().map(|(k, v)| k.len() + 1 + v.len()).sum());
        for (key, value) in &pairs {
            out.push_str(key);
            out.push('=');
            out.push_str(value);
        }
        out
    }

    /// 仅对参数值排序后拼接（设备注册签名规则）
    #[must_use]
    pub fn sorted_values(&self) -> String {
        let mut values: SmallVec<[Cow<'_, str>; 16]> = self.entries.iter().map(|(_, v)| v.render_cow()).collect();
        values.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        let mut out = String::with_capacity(values.iter().map(|v| v.len()).sum());
        for value in &values {
            out.push_str(value);
        }
        out
    }

    /// 转换为查询参数对
    #[must_use]
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        self.entries.iter().map(|(k, v)| (k.clone(), v.render())).collect()
    }
}

/// 按 UTF-16 代码单元排序（对齐 JS 字典序）
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
        // "b=2" 排在 "bb=1" 之前（'=' 码值小于 'b'）
        assert_eq!(p.sorted_kv(), "b=2bb=1");
    }

    #[test]
    fn android_sorts_keys_before_joining_while_web_does_not() {
        let mut p = Params::new();
        p.set("hash", "1").set("hash2", "2");
        // Web 规则（先拼接后排序）："hash2=2" 优先于 "hash=1"
        assert_eq!(p.sorted_kv(), "hash2=2hash=1");
        // Android 规则（按键排序）："hash" 优先于 "hash2"
        assert_eq!(p.sorted_kv_by_key(), "hash=1hash2=2");
        // 无公共前缀时两种规则排序结果一致
        let mut q = Params::new();
        q.set("mid", "m").set("appid", "a").set("plat", "p");
        assert_eq!(q.sorted_kv(), q.sorted_kv_by_key());
    }

    #[test]
    fn the_two_sort_rules_really_differ_for_prefix_keys() {
        // 明确钉住两种排序规则的差异：前缀 + 数字的键对会让整串排序与按键排序分道扬镳。
        let mut p = Params::new();
        p.set("hash", "1").set("hash2", "2");
        assert_ne!(p.sorted_kv(), p.sorted_kv_by_key());
        // 前缀后接非数字字符时两者一致（这是绝大多数真实参数集的情况）。
        let mut q = Params::new();
        q.set("hash", "1").set("hashx", "2");
        assert_eq!(q.sorted_kv(), q.sorted_kv_by_key());
    }

    #[test]
    fn sorts_values_for_register_signature() {
        let mut p = Params::new();
        p.set("z", "bbb").set("a", "aaa").set("m", "ccc");
        assert_eq!(p.sorted_values(), "aaabbbccc");
    }
}
