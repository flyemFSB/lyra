//! 文本与数值格式化工具函数

use lyra_api::model::LyricLine;

/// 设置页支持的音质选项列表
pub(crate) const QUALITY_CHOICES: &[(&str, &str)] = &[("flac", "无损"), ("320", "高品 320K"), ("128", "标准 128K")];

/// 获取音质编码对应的展示文本
pub(crate) fn quality_label(quality: &str) -> &'static str {
    match quality {
        "flac" => "无损",
        "320" => "高品 320K",
        "128" => "标准 128K",
        "viper_tape" => "蝰蛇母带",
        "viper_clear" => "蝰蛇全景",
        "viper_atmos" => "蝰蛇环绕",
        _ => "标准",
    }
}

/// 毫秒时间格式化为时分秒字符串
pub(crate) fn format_duration(ms: u64) -> String {
    let total = ms / 1000;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// 计算播放进度百分比
#[allow(clippy::cast_precision_loss)]
pub(crate) fn progress_percent(position_ms: u64, duration_ms: u64) -> f32 {
    if duration_ms == 0 {
        return 0.0;
    }
    let ratio = position_ms.min(duration_ms) as f64 / duration_ms as f64;
    (ratio * 100.0) as f32
}

/// 字节数格式化为易读存储单位
#[allow(clippy::cast_precision_loss)]
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// 二分查找当前播放时间对应的歌词行索引
pub(crate) fn lyric_index(lines: &[LyricLine], position_ms: u64) -> Option<usize> {
    lines.partition_point(|line| line.time_ms <= position_ms).checked_sub(1)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

    use super::*;

    fn line(time_ms: u64, text: &str) -> LyricLine {
        LyricLine {
            time_ms,
            text: text.to_owned(),
        }
    }

    #[test]
    fn duration_formats_minutes_and_hours() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(59_000), "0:59");
        assert_eq!(format_duration(226_000), "3:46");
        assert_eq!(format_duration(3_600_000), "1:00:00");
    }

    #[test]
    fn lyric_index_finds_current_line() {
        let lines = vec![line(0, "a"), line(1000, "b"), line(2500, "c")];
        assert_eq!(lyric_index(&lines, 0), Some(0));
        assert_eq!(lyric_index(&lines, 999), Some(0));
        assert_eq!(lyric_index(&lines, 1000), Some(1));
        assert_eq!(lyric_index(&lines, 2600), Some(2));
        assert_eq!(lyric_index(&lines, 10_000), Some(2));
        assert_eq!(lyric_index(&[], 100), None);
        assert_eq!(lyric_index(&[line(1000, "x")], 0), None);
    }

    #[test]
    fn bytes_format_scales() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn progress_percent_clamps_to_range() {
        assert_eq!(progress_percent(0, 0), 0.0);
        assert_eq!(progress_percent(50, 100), 50.0);
        assert_eq!(progress_percent(200, 100), 100.0);
    }

    #[test]
    fn quality_labels_cover_known_and_unknown() {
        assert_eq!(quality_label("flac"), "无损");
        assert_eq!(quality_label("320"), "高品 320K");
        assert_eq!(quality_label("weird"), "标准");
    }
}
