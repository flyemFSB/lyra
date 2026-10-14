//! 行为指纹与轨迹模拟（生成 sid 与 edt 参数以应对风控验证）。

use crate::cipher::{fingerprint_edt, md5_hex, random_string_with, rsa_oaep_sha256_base64};
use crate::consts::FP_RSA_PUBLIC;
use crate::error::Result;
use crate::rng::{JsRng, OsRng, js_round, ri};

/// 窗口高宽（模拟移动端 WebView）。
const WIN_W: i64 = 750;
const WIN_H: i64 = 500;

/// 生成结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simulate {
    /// 加密后的行为数据（Base64）。
    pub edt: String,
    /// 加密后的 AES 密钥（Base64）。
    pub sid: String,
}

/// 一次指纹生成所需的输入。
#[derive(Debug, Clone, Copy)]
pub struct SimInputs<'a> {
    /// 设备 MID。
    pub mid: &'a str,
    /// 用户 ID（未登录传空）。
    pub userid: &'a str,
    /// 设备 dfid。
    pub dfid: &'a str,
    /// WebGL 指纹。参考实现里该值缺失会抛错，故必填。
    pub webgl: &'a str,
    /// 毫秒时间戳。
    pub ts_millis: i64,
}

/// 单次指纹生成的随机抽样集合（保证与 JS 参考实现的随机调用序列一致）。
#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    /// 哨兵随机值。
    pub sentinel: u32,
    /// EDT 的 AES-128 密钥（16 位十六进制字符串）。
    pub key: String,
    /// 轨迹采样点数。
    pub points: i64,
    /// 轨迹起点坐标。
    pub start: (f64, f64),
    /// 轨迹终点坐标。
    pub end: (f64, f64),
}

impl Draw {
    /// 按参考实现的调用顺序抽取各随机参数。
    pub fn take(rng: &mut impl JsRng) -> Self {
        // SENTINEL = 0xffffffff - floor(rand * 20)
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let sentinel = 0xffff_ffff_u32 - (rng.next_f64() * 20.0).floor() as u32;
        let key: String = md5_hex(random_string_with(rng, 16).as_bytes())
            .chars()
            .take(16)
            .collect();
        let points = ri(rng, 30, 60);
        #[allow(clippy::cast_precision_loss)]
        let start = (ri(rng, 200, 600) as f64, ri(rng, 200, 500) as f64);
        #[allow(clippy::cast_precision_loss)]
        let end = (ri(rng, 500, 700) as f64, ri(rng, 80, 150) as f64);
        Self {
            sentinel,
            key,
            points,
            start,
            end,
        }
    }
}

/// 生成 `sid` / `edt`，随机源可注入。
pub fn simulate(rng: &mut impl JsRng, inputs: &SimInputs<'_>) -> Result<Simulate> {
    let draw = Draw::take(rng);
    let data = edt_events(rng, draw.sentinel, draw.start, draw.end, draw.points);

    let plaintext = format!(
        "mid={};userid={};dfid={};webgl={};webdriver=0;ts={};data={data}",
        or_zero(inputs.mid),
        or_zero(inputs.userid),
        or_zero(inputs.dfid),
        inputs.webgl,
        inputs.ts_millis,
    );

    Ok(Simulate {
        edt: fingerprint_edt(&plaintext, &draw.key)?,
        sid: rsa_oaep_sha256_base64(draw.key.as_bytes(), FP_RSA_PUBLIC)?,
    })
}

/// 生成 `sid` / `edt`，使用系统熵与当前时间。
pub fn simulate_now(inputs: &SimInputs<'_>) -> Result<Simulate> {
    #[allow(clippy::cast_possible_truncation)]
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let inputs = SimInputs {
        ts_millis: ts,
        ..*inputs
    };
    simulate(&mut OsRng, &inputs)
}

/// JS 的 `x || 0`。
fn or_zero(s: &str) -> &str {
    if s.is_empty() { "0" } else { s }
}

/// 行为事件串：`:` 分隔，每条字段用 `,` 分隔。
#[must_use]
pub fn edt_events(rng: &mut impl JsRng, sentinel: u32, start: (f64, f64), end: (f64, f64), points: i64) -> String {
    let mut entries: Vec<String> = Vec::with_capacity(points as usize * 4 + 32);
    let mut ts: i64 = 0;
    let mut ei: i64 = 0;

    // 前置两条初始滚动事件及其哨兵
    entries.push(f5(0, 0));
    entries.push(fs5(sentinel, 0));
    entries.push(f5(0, 0));
    entries.push(fs5(sentinel, 0));

    // 窗口尺寸事件 (type-6)
    ts += ri(rng, 5, 20);
    entries.push(f6(ts, ei, WIN_W, WIN_H));
    entries.push(fs6(sentinel, ei, WIN_W, WIN_H));
    ei += 1;

    // 随机滚动事件 (type-5)
    for _ in 0..3 {
        ts += ri(rng, 80, 600);
        entries.push(f5(ts, ei));
        entries.push(fs5(sentinel, ei));
        ei += 1;
    }

    // 鼠标滑动轨迹事件 (type-3)
    let path = bezier_path(rng, start.0, start.1, end.0, end.1, points);
    let mut si: i64 = 0;
    for (i, &(x, y)) in path.iter().enumerate() {
        let (xi, yi) = (js_round(x), js_round(y));
        ts += ri(rng, 8, 50);
        entries.push(f3(ts, si, xi, yi));
        entries.push(fs3(sentinel, si, xi, yi));

        // 模拟移动过程中的间歇滚动
        if i > 0 && i % 12 == 0 {
            ts += ri(rng, 20, 60);
            entries.push(f5(ts, ei));
            entries.push(fs5(sentinel, ei));
            ei += 1;
        }
        si = (si + 1) % 2;
    }

    // 终点微抖动与哨兵事件
    ts += ri(rng, 5, 30);
    #[allow(clippy::cast_precision_loss)]
    let (dx, dy) = (ri(rng, -5, 5) as f64, ri(rng, -5, 5) as f64);
    entries.push(f3(ts, 1, js_round(end.0 + dx), js_round(end.1 + dy)));
    entries.push(fs3(sentinel, 1, js_round(end.0), js_round(end.1)));

    entries.join(":")
}

/// 三阶贝塞尔轨迹 + 递减抖动（3px → 0.5px）。
#[must_use]
pub fn bezier_path(rng: &mut impl JsRng, sx: f64, sy: f64, ex: f64, ey: f64, n: i64) -> Vec<(f64, f64)> {
    #[allow(clippy::cast_precision_loss)]
    let c1x = sx + (ex - sx) * 0.3 + ri(rng, -80, 80) as f64;
    #[allow(clippy::cast_precision_loss)]
    let c1y = sy + (ey - sy) * 0.2 + ri(rng, -60, 60) as f64;
    #[allow(clippy::cast_precision_loss)]
    let c2x = sx + (ex - sx) * 0.7 + ri(rng, -60, 60) as f64;
    #[allow(clippy::cast_precision_loss)]
    let c2y = sy + (ey - sy) * 0.8 + ri(rng, -40, 40) as f64;

    let mut pts = Vec::with_capacity(n as usize + 1);
    for i in 0..=n {
        #[allow(clippy::cast_precision_loss)]
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let x = u * u * u * sx + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * ex;
        let y = u * u * u * sy + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * ey;
        // 起步抖动大，移动后逐渐平稳
        let jitter = (3.0 - t * 2.5).max(0.5);
        pts.push((x + (rng.next_f64() - 0.5) * jitter, y + (rng.next_f64() - 0.5) * jitter));
    }
    pts
}

fn f3(t: i64, i: i64, x: i64, y: i64) -> String {
    format!("3,{t},{i},{x},{y}")
}
fn f5(t: i64, i: i64) -> String {
    format!("5,{t},{i}")
}
fn f6(t: i64, i: i64, w: i64, h: i64) -> String {
    format!("6,{t},{i},{w},{h}")
}
fn fs3(sentinel: u32, i: i64, x: i64, y: i64) -> String {
    format!("3,{sentinel},{i},{x},{y}")
}
fn fs5(sentinel: u32, i: i64) -> String {
    format!("5,{sentinel},{i}")
}
fn fs6(sentinel: u32, i: i64, w: i64, h: i64) -> String {
    format!("6,{sentinel},{i},{w},{h}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::rng::Mulberry32;

    #[test]
    fn events_start_with_two_zero_type5_pairs() {
        let mut rng = Mulberry32::new(1);
        let s = edt_events(&mut rng, 0xffff_ffff, (0.0, 0.0), (1.0, 1.0), 1);
        assert!(s.starts_with("5,0,0:5,4294967295,0:5,0,0:5,4294967295,0:6,"), "{s}");
    }

    #[test]
    fn bezier_path_has_n_plus_one_points() {
        let mut rng = Mulberry32::new(2);
        assert_eq!(bezier_path(&mut rng, 0.0, 0.0, 10.0, 10.0, 7).len(), 8);
    }

    #[test]
    fn simulate_produces_decodable_output() {
        let mut rng = Mulberry32::new(3);
        let s = simulate(
            &mut rng,
            &SimInputs {
                mid: "123",
                userid: "456",
                dfid: "789",
                webgl: "111",
                ts_millis: 1,
            },
        )
        .unwrap();
        assert_ne!(s.edt, "");
        assert_ne!(s.sid, "");
    }
}
