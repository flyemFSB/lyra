//! 封面主导色提取与沉浸式动态氛围调色盘。
//!
//! 基于色彩量化与色相过滤，从专辑封面中提取两套明亮主导色与微光底色，
//! 严格滤除脏灰、暗浊与杂乱色相，确保背景光晕通透、柔和、高级。

use gpui_kit::{Hsla, hsla};

/// 沉浸页专辑光晕调色盘
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AlbumPalette {
    /// 第一主导色（亮色调，如浅粉色 #F5A8B8 或翡翠绿）
    pub(crate) primary: Hsla,
    /// 第二主导色（暖色调，如暖肉粉色 #E88B9E）
    pub(crate) secondary: Hsla,
    /// 封面漫反射环绕投影色
    pub(crate) glow: Hsla,
    /// 底层微调浅色渐变底（柔和透亮，与纯白自然过渡）
    pub(crate) surface_tint: Hsla,
}

impl Default for AlbumPalette {
    fn default() -> Self {
        // 默认温润浅粉微暖调色（如《LOVE》同款优雅色阶）
        Self {
            primary: hsla(348.0 / 360.0, 0.75, 0.82, 0.22),
            secondary: hsla(16.0 / 360.0, 0.70, 0.78, 0.18),
            glow: hsla(350.0 / 360.0, 0.65, 0.72, 0.28),
            surface_tint: hsla(345.0 / 360.0, 0.35, 0.98, 1.0),
        }
    }
}

/// RGB 转 HSL (h: 0..360, s: 0..1, l: 0..1)
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);
    if (max - min).abs() < 1e-4 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let mut h = if (max - r).abs() < 1e-4 {
        (g - b) / d + (if g < b { 6.0 } else { 0.0 })
    } else if (max - g).abs() < 1e-4 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h *= 60.0;
    (h, s, l)
}

/// 色相直方图桶
#[derive(Default, Clone, Copy)]
struct Bucket {
    count: usize,
    sum_h: f32,
    sum_s: f32,
    sum_l: f32,
}

/// 从图片原始字节中提取 2 个主导色并构建调色盘
pub(crate) fn extract_palette_from_bytes(bytes: &[u8]) -> Option<AlbumPalette> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return None;
    }

    // 采用跨行跨列步长采样（至多采样 ~2500 像素，毫秒级零延迟完成）
    let step_x = (width / 50).max(1);
    let step_y = (height / 50).max(1);

    let mut buckets = [Bucket::default(); 12];
    let mut total_valid = 0;

    for y in (0..height).step_by(step_y as usize) {
        for x in (0..width).step_by(step_x as usize) {
            let pixel = img.get_pixel(x, y);
            // 忽略透明像素
            if pixel[3] < 128 {
                continue;
            }
            let r = f32::from(pixel[0]) / 255.0;
            let g = f32::from(pixel[1]) / 255.0;
            let b = f32::from(pixel[2]) / 255.0;
            let (h, s, l) = rgb_to_hsl(r, g, b);

            // 严格过滤：滤除饱和度极低的灰色、死黑与纯白，确保提取出来的都是纯正饱和的音乐色彩
            if s < 0.18 || l < 0.18 || l > 0.93 {
                continue;
            }

            let bucket_idx = ((h / 30.0) as usize).min(11);
            let b = &mut buckets[bucket_idx];
            b.count += 1;
            b.sum_h += h;
            b.sum_s += s;
            b.sum_l += l;
            total_valid += 1;
        }
    }

    if total_valid < 10 {
        return None;
    }

    // 按像素计数排序
    let mut indices: Vec<usize> = (0..12).collect();
    indices.sort_by_key(|&idx| std::cmp::Reverse(buckets[idx].count));

    let b1 = buckets[indices[0]];
    if b1.count == 0 {
        return None;
    }

    let c1_count = b1.count as f32;
    let h1 = b1.sum_h / c1_count;
    let s1 = (b1.sum_s / c1_count).clamp(0.40, 0.85);
    // 亮度适度上提，防止暗沉
    let l1 = (b1.sum_l / c1_count).clamp(0.60, 0.84);

    // 寻找第二个具有对比度或差异的主导桶
    let mut second_idx = None;
    for &idx in &indices[1..] {
        let b = buckets[idx];
        if b.count > 0 {
            let h = b.sum_h / (b.count as f32);
            let diff = (h - h1).abs();
            let angle_diff = diff.min(360.0 - diff);
            if angle_diff >= 25.0 {
                second_idx = Some(idx);
                break;
            }
        }
    }

    let (h2, s2, l2) = if let Some(idx) = second_idx {
        let b2 = buckets[idx];
        let c2_count = b2.count as f32;
        (
            b2.sum_h / c2_count,
            (b2.sum_s / c2_count).clamp(0.40, 0.85),
            (b2.sum_l / c2_count).clamp(0.60, 0.84),
        )
    } else {
        // 色相对调微旋 35° 派生柔和互补色
        ((h1 + 35.0) % 360.0, s1 * 0.9, l1.clamp(0.65, 0.85))
    };

    Some(AlbumPalette {
        primary: hsla(h1 / 360.0, s1, l1, 0.22),
        secondary: hsla(h2 / 360.0, s2, l2, 0.18),
        glow: hsla(h1 / 360.0, s1, l1, 0.28),
        surface_tint: hsla(h1 / 360.0, (s1 * 0.40).clamp(0.15, 0.35), 0.98, 1.0),
    })
}

/// 基于曲目标题与歌手的确定性哈希派生温润高品质调色盘（无暗浊脏色）
pub(crate) fn fallback_palette(title: &str, singer: &str) -> AlbumPalette {
    let mut hash: u32 = 2166136261;
    for b in title.bytes().chain(singer.bytes()) {
        hash = (hash ^ (b as u32)).wrapping_mul(16777619);
    }

    // 映射到优雅的音乐主色调区间（粉红、暖橙、温润琥珀、薄荷绿、暖紫罗兰）
    let hues = [
        348.0, // 柔粉
        355.0, // 玫瑰粉
        18.0,  // 暖杏珊瑚
        36.0,  // 暖金琥珀
        155.0, // 翡翠薄荷
        170.0, // 浅青碧
        275.0, // 浅紫罗兰
        320.0, // 暖洋红
    ];
    let h1 = hues[(hash as usize) % hues.len()];
    let h2 = (h1 + 40.0) % 360.0;

    AlbumPalette {
        primary: hsla(h1 / 360.0, 0.70, 0.80, 0.20),
        secondary: hsla(h2 / 360.0, 0.65, 0.76, 0.16),
        glow: hsla(h1 / 360.0, 0.65, 0.72, 0.26),
        surface_tint: hsla(h1 / 360.0, 0.25, 0.98, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_palette_produces_luminous_colors() {
        let p = fallback_palette("LOVE", "周杰伦");
        assert!(p.primary.l >= 0.70);
        assert!(p.secondary.l >= 0.70);
        assert!(p.surface_tint.l >= 0.95);
    }

    #[test]
    fn rgb_to_hsl_calculates_correct_hue() {
        // 纯红
        let (h, s, l) = rgb_to_hsl(1.0, 0.0, 0.0);
        assert!((h - 0.0).abs() < 1e-3);
        assert!((s - 1.0).abs() < 1e-3);
        assert!((l - 0.5).abs() < 1e-3);

        // 浅粉色 (245, 168, 184) -> #F5A8B8
        let (h, s, l) = rgb_to_hsl(245.0 / 255.0, 168.0 / 255.0, 184.0 / 255.0);
        assert!((340.0..=355.0).contains(&h), "Hue should be pinkish, got {h}");
        assert!(s >= 0.70, "Saturation should be high, got {s}");
        assert!(l >= 0.75, "Lightness should be high, got {l}");
    }
}
