//! 可注入随机数生成器（用于行为指纹生成与测试向量复现）。

use rand::Rng;

/// JS 随机数生成器抽象接口（返回 [0, 1) 浮点数）。
pub trait JsRng {
    /// 生成 [0, 1) 区间的 64 位浮点数。
    fn next_f64(&mut self) -> f64;
}

/// 32 位伪随机数生成器 Mulberry32（确保跨语言字节级对齐）。
#[derive(Debug, Clone)]
pub struct Mulberry32 {
    state: u32,
}

impl Mulberry32 {
    /// 使用指定种子初始化随机状态。
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self { state: seed }
    }
}

impl JsRng for Mulberry32 {
    fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        // 使用掩码 61 (0b0011_1101) 保证与 JS 算法严格一致
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 0b0011_1101));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

/// 基于操作系统熵源的随机生成器。
#[derive(Debug, Clone, Default)]
pub struct OsRng;

impl JsRng for OsRng {
    fn next_f64(&mut self) -> f64 {
        // Rust 2024 将 gen 列为关键字，此处使用原生标识符调用
        rand::thread_rng().r#gen::<f64>()
    }
}

/// 生成 [min, max] 闭区间随机整数（对应 JS Math.floor 逻辑）。
pub(crate) fn ri(rng: &mut impl JsRng, min: i64, max: i64) -> i64 {
    (rng.next_f64() * ((max - min + 1) as f64)).floor() as i64 + min
}

/// 对齐 JS 四舍五入行为（平局向正无穷取整）。
pub(crate) fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// JS `Math.ceil`。
pub(crate) fn js_ceil(x: f64) -> f64 {
    x.ceil()
}
