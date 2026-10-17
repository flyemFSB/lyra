//! 歌词检索与解析端点。

use lyra_proto::krc_decode_base64;

use crate::client::Api;
use crate::codec;
use crate::error::{Error, Result};
use crate::model::{LyricLine, Song};
use crate::spec::LYRICS_BASE;
use crate::spec::RequestSpec;

/// 歌词候选引用凭据
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LyricRef {
    /// 歌词 ID
    pub id: String,
    /// 访问密钥（accesskey）
    pub accesskey: String,
}

/// 第一阶段：搜索歌曲匹配的歌词候选
pub async fn search_lyric(api: &Api, hash: &str, duration_ms: u64, album_audio_id: &str) -> Result<LyricRef> {
    // 歌词搜索使用独立签名参数集
    let spec = RequestSpec::get("/v1/search")
        .base(LYRICS_BASE)
        .clear_default_params()
        .param("album_audio_id", album_audio_id)
        .param("appid", lyra_proto::consts::APPID)
        .param("clientver", lyra_proto::consts::CLIENTVER)
        .param("duration", duration_ms)
        .param("hash", hash)
        .param("keyword", "")
        .param("lrctxt", 1)
        .param("man", "no");

    let value = api.call(spec, crate::cache::Freshness::CATALOG).await?;
    let candidates = codec::find_array(&value, &["candidates", "candidate", "list"])
        .cloned()
        .unwrap_or_default();
    let first = candidates
        .iter()
        .find(|item| !codec::pick_str(item, &["id"]).is_empty())
        .ok_or_else(|| Error::Decode("歌词搜索没有候选结果".to_owned()))?;
    Ok(LyricRef {
        id: codec::pick_str(first, &["id"]),
        accesskey: codec::pick_str(first, &["accesskey", "access_key"]),
    })
}

/// 第二阶段：下载并解密指定歌词内容
pub async fn download_lyric(api: &Api, reference: &LyricRef, fmt: &str) -> Result<String> {
    let spec = RequestSpec::get("/download")
        .base(LYRICS_BASE)
        .param("ver", 1)
        .param("client", "android")
        .param("id", reference.id.clone())
        .param("accesskey", reference.accesskey.clone())
        .param("fmt", if fmt.is_empty() { "krc" } else { fmt })
        .param("charset", "utf8");

    let value = api.call(spec, crate::cache::Freshness::CATALOG).await?;
    let content = codec::pick_str(&value, &["content"]);
    if content.is_empty() {
        return Err(Error::Decode("歌词响应没有 content".to_owned()));
    }
    let content_type = codec::pick_i64(&value, &["contenttype"]);
    if fmt == "lrc" || content_type != 0 {
        return Ok(String::from_utf8_lossy(&lyra_proto::base64_decode(&content)?).into_owned());
    }
    krc_decode_base64(&content).map_err(Error::from)
}

/// 获取并解析指定歌曲的结构化歌词
pub async fn lyric_for_song(api: &Api, song: &Song) -> Result<Vec<LyricLine>> {
    let reference = search_lyric(api, &song.hash, song.duration_ms, &song.album_audio_id).await?;
    let text = download_lyric(api, &reference, "krc").await?;
    Ok(parse_lyrics(&text))
}

/// 解析 LRC 与 KRC 歌词文本为时间序列列表
#[must_use]
pub fn parse_lyrics(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut body = raw.trim();
        let mut stamps: Vec<u64> = Vec::new();
        // 单行多时间戳展开为独立歌词行
        while let Some(tail) = body.strip_prefix('[') {
            let Some((head, after)) = tail.split_once(']') else {
                break;
            };
            // 跳过元数据标签
            if head.chars().take(2).any(|c| c.is_ascii_alphabetic()) {
                stamps.clear();
                break;
            }
            // 解析 KRC 标签
            let is_krc = head.contains(',');
            let time_ms = if is_krc {
                head.split(',')
                    .next()
                    .map_or(0, |start| start.trim().parse::<u64>().unwrap_or(0))
            } else {
                parse_lrc_time(head)
            };
            stamps.push(time_ms);
            body = after;
            if is_krc {
                break;
            }
        }
        let content = strip_krc_tags(body).trim().to_owned();
        if content.is_empty() {
            continue;
        }
        for time_ms in stamps {
            lines.push(LyricLine {
                time_ms,
                text: content.clone(),
            });
        }
    }
    lines.sort_by_key(|line| line.time_ms);
    lines
}

/// 解析 LRC 时间标签为毫秒时间戳
fn parse_lrc_time(head: &str) -> u64 {
    let mut parts = head.split(':');
    let minutes = parts.next().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
    let seconds_part = parts.next().unwrap_or("");
    let digits: String = seconds_part.chars().take_while(char::is_ascii_digit).collect();
    let seconds = digits.parse::<u64>().unwrap_or(0);
    let fraction: String = seconds_part
        .chars()
        .skip_while(char::is_ascii_digit)
        .skip(1)
        .take(2)
        .collect();
    let millis = match fraction.len() {
        1 => fraction.parse::<u64>().unwrap_or(0) * 100,
        2 => fraction.parse::<u64>().unwrap_or(0) * 10,
        _ => 0,
    };
    minutes * 60_000 + seconds * 1000 + millis
}

/// 剔除 KRC 字级逐字高亮标签
fn strip_krc_tags(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut depth = 0_u32;
    for ch in body.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::transport::MockTransport;

    #[test]
    fn parses_lrc_lines() {
        let text = "[ti:夜曲]\n[ar:周杰伦]\n[00:00.00]本字幕由...\n[00:12.50]一群嗜血的蚂蚁\n[01:02.05]被腐肉所吸引";
        let lines = parse_lyrics(text);
        assert_eq!(lines.len(), 3, "元数据行应被跳过: {lines:?}");
        assert_eq!(lines[0].text, "本字幕由...");
        assert_eq!(lines[1].time_ms, 12_500);
        assert_eq!(lines[2].time_ms, 62_050);
    }

    #[test]
    fn expands_repeated_timestamp_tags() {
        let text = "[00:12.50][01:30.00]副歌\n[00:20.00]主歌";
        let lines = parse_lyrics(text);
        assert_eq!(lines.len(), 3, "同一行的两个时间标签应各出一行: {lines:?}");
        assert_eq!(lines[0].time_ms, 12_500);
        assert_eq!(lines[1].time_ms, 20_000);
        assert_eq!(lines[2].time_ms, 90_000);
        assert!(lines.iter().all(|line| line.text == "副歌" || line.text == "主歌"));
    }

    #[test]
    fn parses_krc_lines_and_strips_word_tags() {
        let text = "[0,0]<0,0,0>\n[1234,567]<0,300,0>一<300,267,0>群";
        let lines = parse_lyrics(text);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].time_ms, 1234);
        assert_eq!(lines[0].text, "一群");
    }

    #[test]
    fn sorts_by_time() {
        let text = "[00:05.00]b\n[00:01.00]a";
        let lines = parse_lyrics(text);
        assert_eq!(lines[0].text, "a");
        assert_eq!(lines[1].text, "b");
    }

    #[test]
    fn ignores_blank_and_malformed_lines() {
        let text = "garbage\n[]\n[00:01.00]   \nnot a lyric";
        assert!(parse_lyrics(text).is_empty());
    }

    #[test]
    fn parses_centisecond_and_decisecond_fractions() {
        assert_eq!(parse_lrc_time("00:01.5"), 1500);
        assert_eq!(parse_lrc_time("00:01.55"), 1550);
        assert_eq!(parse_lrc_time("01:00"), 60_000);
    }

    /// 验证歌词服务在缺省 status 字段时依然能够正常解析。
    #[tokio::test]
    async fn two_step_lyric_flow_accepts_status_less_responses() {
        let mock = Arc::new(MockTransport::new());
        mock.on_json(
            "/v1/search",
            &serde_json::json!({
                "candidates": [{"id": 12345, "accesskey": "AK", "singer": "周杰伦"}]
            }),
        );
        let text = "[00:01.00]一群嗜血的蚂蚁";
        mock.on_json(
            "/download",
            &serde_json::json!({
                // 非零类型表示纯文本 LRC 编码。
                "content": lyra_proto::base64_encode(text.as_bytes()),
                "contenttype": 1
            }),
        );

        let api = crate::testing::anonymous_api(mock.clone());

        // 第一阶段搜索歌词。
        let reference = search_lyric(&api, "abc123", 226_000, "98765").await.unwrap();
        assert_eq!(reference.id, "12345");
        assert_eq!(reference.accesskey, "AK");

        // 第二阶段下载并解析。
        let downloaded = download_lyric(&api, &reference, "lrc").await.unwrap();
        assert_eq!(downloaded, text);
        let lines = parse_lyrics(&downloaded);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "一群嗜血的蚂蚁");

        // 校验歌词搜索带签名且使用独立参数集。
        let search_request = mock.requests().first().cloned().unwrap();
        assert!(
            search_request.query.iter().any(|(k, _)| k == "signature"),
            "歌词搜索必须签名，否则上游回空响应体: {:?}",
            search_request.query
        );
        assert!(search_request.query.iter().any(|(k, _)| k == "appid"));
        assert!(
            !search_request.query.iter().any(|(k, _)| k == "mid"),
            "歌词搜索用独立参数集，不带全局默认参数"
        );
    }
}
