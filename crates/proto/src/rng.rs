//! 随机数生成器抽象（支持 Mulberry32 与系统随机源）。

use rand::Rng;

/// JS 随机数生成器抽象（返回 [0, 1) 浮点数）
pub trait JsRng {
    /// 生成 [0, 1) 浮点数
    fn next_f64(&mut self) -> f64;
}

/// Mulberry32 伪随机数生成器（对齐 JS 参考实现）
#[derive(Debug, Clone)]
pub struct Mulberry32 {
    state: u32,
}

impl Mulberry32 {
    /// 使用种子初始化
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
        // 掩码 61 (0b0011_1101) 对齐 JS 算法
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 0b0011_1101));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

/// 系统随机源
#[derive(Debug, Clone, Default)]
pub struct OsRng;

impl JsRng for OsRng {
    fn next_f64(&mut self) -> f64 {
        // r#gen 避免与 Rust 2024 关键字冲突
        rand::thread_rng().r#gen::<f64>()
    }
}

/// 生成 [min, max] 闭区间随机整数（对齐 JS Math.floor）
pub(crate) fn ri(rng: &mut impl JsRng, min: i64, max: i64) -> i64 {
    (rng.next_f64() * ((max - min + 1) as f64)).floor() as i64 + min
}

/// 对齐 JS Math.round 四舍五入
pub(crate) fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// 对齐 JS Math.ceil 向上取整
pub(crate) fn js_ceil(x: f64) -> f64 {
    x.ceil()
}
