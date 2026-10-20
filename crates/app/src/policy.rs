//! 播放策略：音质回落与换源匹配（纯函数，不触碰 IO）

use lyra_api::model::{Song, SongRights};

use crate::error::{Error, Result};

/// 默认音质回落优先级（从高到低）
const QUALITY_FALLBACK: &[&str] = &["flac", "320", "128"];

/// 换源时容许的时长偏差（毫秒，兼容转码或重传误差）
const REPLACEMENT_DRIFT_MS: u64 = 8_000;

/// 匹配用户偏好且可用的最优音质
pub(crate) fn pick_quality(rights: &SongRights, requested: &str) -> Result<String> {
    let preferred = if requested.is_empty() { "flac" } else { requested };
    if rights.quality(preferred).is_some_and(|q| q.obtainable) {
        return Ok(preferred.to_owned());
    }
    for candidate in QUALITY_FALLBACK {
        if rights.quality(candidate).is_some_and(|q| q.obtainable) {
            return Ok((*candidate).to_owned());
        }
    }
    if !rights.best_free_quality.is_empty() {
        return Ok(rights.best_free_quality.clone());
    }
    Err(Error::NoPlayableQuality {
        requested: preferred.to_owned(),
        available: rights.free_qualities.clone(),
    })
}

/// 判定候选歌曲是否为可用替换源（标题一致且时长相近）
pub(crate) fn is_replacement(original: &Song, candidate: &Song) -> bool {
    if candidate.hash.is_empty() || candidate.hash.eq_ignore_ascii_case(&original.hash) {
        return false;
    }
    let want = squash(&original.name);
    let got = squash(&candidate.name);
    // 避免单字等过短标题误匹配
    if want.chars().count() < 2 || got.is_empty() {
        return false;
    }
    let core_matches = core_title(&got) == core_title(&want);
    if !(got.contains(&want) || want.contains(&got) || core_matches) {
        return false;
    }
    // 存在有效时长时校验偏差，否则要求核心标题完全一致
    match (original.duration_ms, candidate.duration_ms) {
        (a, b) if a > 1_000 && b > 1_000 => a.abs_diff(b) <= REPLACEMENT_DRIFT_MS,
        _ => core_matches,
    }
}

/// 去除空白并转为小写
pub(crate) fn squash(name: &str) -> String {
    name.chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// 剔除括号说明与版本后缀并归一化
pub(crate) fn core_title(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0_u32;
    for ch in name.chars() {
        match ch {
            '(' | '（' | '《' | '[' | '【' => depth += 1,
            ')' | '）' | '》' | ']' | '】' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    squash(&out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn song(name: &str, singer: &str, hash: &str, duration_ms: u64, privilege: i64) -> Song {
        Song {
            name: name.to_owned(),
            singer: singer.to_owned(),
            hash: hash.to_owned(),
            duration_ms,
            privilege,
            ..Song::default()
        }
    }

    #[test]
    fn replacement_accepts_a_reupload_of_the_same_recording() {
        // 验证官方下架但 UGC 重传版可播的情形（允许 5 秒内偏差）
        let original = song(
            "阴天 (2026明日世界巡回演唱会北京站)",
            "汪苏泷、莫文蔚",
            "f78de2e4",
            225_000,
            5,
        );
        let reupload = song(
            "汪苏泷、莫文蔚《阴天 (2026明日世界巡回演唱会北京站)》",
            "汪苏泷、莫文蔚",
            "313c11d5",
            230_000,
            0,
        );
        assert!(is_replacement(&original, &reupload));
    }

    #[test]
    fn replacement_rejects_other_versions_other_songs_and_itself() {
        let original = song("阴天 (2026巡演北京站)", "汪苏泷", "f78de", 225_000, 5);
        // 时长偏差过大视为不同版本
        assert!(!is_replacement(
            &original,
            &song("阴天 (Live)", "莫文蔚", "a", 247_000, 10)
        ));
        // 歌曲完全不匹配
        assert!(!is_replacement(&original, &song("嘉宾", "皮大大", "b", 225_000, 0)));
        // 原歌曲不作为替换目标
        assert!(!is_replacement(&original, &song("阴天", "汪苏泷", "f78de", 225_000, 5)));
        // 单字标题不参与换源
        assert!(!is_replacement(
            &song("雨", "x", "c", 0, 5),
            &song("雨", "y", "d", 0, 0)
        ));
    }

    #[test]
    fn core_title_drops_version_suffixes() {
        assert_eq!(core_title("阴天（Live）"), "阴天");
        assert_eq!(core_title("汪苏泷、莫文蔚《阴天 (2026演唱会)》"), "汪苏泷、莫文蔚");
        assert_eq!(squash("MiXeD  Case"), "mixedcase");
    }
}
