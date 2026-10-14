//! 命令行接口实现。

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::service::Kugou;

/// 解析默认数据目录：环境变量优先，未设置则使用本地应用数据路径。
#[must_use]
pub fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LYRA_DATA_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local).join("lyra");
    }
    PathBuf::from(".lyra-data")
}

/// 命令行使用帮助文本。
#[must_use]
pub const fn help() -> &'static str {
    "lyra — 酷狗概念版桌面客户端（CLI 模式）

用法: lyra <命令> [参数]

  login <手机号> <验证码>   短信登录并落盘（先不带验证码调用会下发验证码）
  code <手机号>             只下发验证码，打印剩余可发次数
  whoami                    查看登录态与设备信息
  playlists                 列出全部歌单
  tracks <global_id> [页号] 列出歌单歌曲（默认第 1 页）
  rights <hash> [专辑id]    打印音质权益矩阵
  url <hash> [专辑id] [音质] 取播放地址
  download <hash> [专辑id] <输出文件> 下载音频并校验文件头（M1 验收）
  daily                     每日推荐
  fm                        个人电台一页
  stations                  推荐电台频道
  classes                   电台分类
  station <fmid> [offset]   某个频道的歌
  vip                       会员状态
  claim                     领取每日畅听 VIP
  help                      显示本帮助

位于 hash 之后的参数全部按顺序对应，所以不需要专辑 id 时必须写空串：
  lyra url <hash> \"\" 320        取 320k 播放地址（直接写 lyra url <hash> 320 会把 320 当成专辑 id）
  lyra download <hash> \"\" out.flac   下载到 out.flac
"
}

/// 解析并分发执行命令行子命令。
pub async fn run(args: &[String]) -> Result<()> {
    let Some(command) = args.first().map(String::as_str) else {
        println!("{}", help());
        return Ok(());
    };
    let rest = &args[1..];
    let app = Kugou::open(default_data_dir())?;

    match command {
        "help" | "--help" | "-h" => println!("{}", help()),
        "code" => {
            let mobile = required(rest, 0, "手机号")?;
            let count = app.send_code(mobile).await?;
            println!("验证码已下发，剩余可发次数 {count}");
        }
        "login" => {
            let mobile = required(rest, 0, "手机号")?;
            if let Some(code) = rest.get(1) {
                let account = app.login(mobile, code).await?;
                println!("登录成功 userid={} token={}", account.userid, mask(&account.token));
            } else {
                let count = app.send_code(mobile).await?;
                println!("已下发验证码（剩余 {count} 次）。带验证码重跑: lyra login <手机号> <验证码>");
            }
        }
        "whoami" => {
            let session = app.session();
            println!("登录: {}", if app.is_logged_in() { "是" } else { "否" });
            println!("userid: {}", session.userid());
            println!("mid: {}", session.device.mid);
            println!("guid: {}", session.device.guid);
            println!("dfid: {}", session.dfid());
            println!("webgl: {}", session.device.webgl);
            println!("数据目录: {}", app.data_dir().display());
        }
        "playlists" => {
            for list in app.playlists().await? {
                println!(
                    "{:<12} {:<24} {:>5} 首  type={}  {}",
                    list.listid,
                    clip(&list.name, 24),
                    list.count,
                    list.kind,
                    list.global_collection_id
                );
            }
        }
        "tracks" => {
            let global_id = required(rest, 0, "global_collection_id")?;
            let page = rest.get(1).and_then(|p| p.parse().ok()).unwrap_or(1);
            let page_data = app.tracks(global_id, page).await?;
            for (index, song) in page_data.items.iter().enumerate() {
                println!(
                    "{:>3}. {:<28} {:<18} {}ms  {}",
                    index + 1,
                    clip(&song.name, 28),
                    clip(&song.singer, 18),
                    song.duration_ms,
                    song.hash
                );
            }
            println!("共 {} 首，下一页游标 {:?}", page_data.items.len(), page_data.cursor);
        }
        "rights" => {
            let item = song_stub(rest)?;
            let rights = app.rights(&item).await?;
            println!(
                "{} — {} / {}   整首免费={}",
                rights.name, rights.singer, rights.albumname, rights.free_song
            );
            for quality in &rights.qualities {
                println!(
                    "  {:<14} exists={:<5} level={:<3} 可获取={:<5} 需会员={:<5} 会员类型={:<8} {}",
                    quality.quality,
                    quality.exists,
                    quality.level,
                    quality.obtainable,
                    quality.need_vip,
                    if quality.require_vip_type.is_empty() {
                        "-"
                    } else {
                        &quality.require_vip_type
                    },
                    quality.hash
                );
            }
            println!("最佳可获取音质: {}", rights.best_free_quality);
            println!("需要超级 VIP 的音质: {:?}", rights.super_vip_qualities);
        }
        "url" => {
            let item = song_stub(rest)?;
            let quality = rest.get(2).map_or("", String::as_str);
            let url = app.song_url(&item, quality).await?;
            println!("url: {}", url.url);
            println!(
                "ext: {}  quality: {}  size: {} B",
                url.ext_or_guess(),
                url.quality,
                url.size
            );
            println!("备用节点: {}", url.backup_urls.len());
        }
        "download" => {
            let item = song_stub(rest)?;
            let out = rest
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| Error::Invalid("缺少输出文件路径".to_owned()))?;
            let url = app.song_url(&item, "").await?;
            let ext = url.ext_or_guess();
            let path = app
                .blobs()
                .ensure(&url.url, &item.hash, &ext)
                .await
                .map_err(|error| Error::Api(error.to_string()))?;
            // 读取音频文件头魔数校验容器格式。
            let mut head = [0_u8; 4];
            let read = std::io::Read::read(&mut std::fs::File::open(&path)?, &mut head)?;
            let head = &head[..read];
            let magic = String::from_utf8_lossy(head);
            println!("落盘: {}", path.display());
            println!("字节数: {}", std::fs::metadata(&path)?.len());
            println!("文件头: {head:02X?} ({magic})");
            if magic == "fLaC" {
                println!("✅ 确为真无损 FLAC");
            } else if magic == "OggS" {
                println!("✅ Ogg 容器（概念版 128/320 的标准形态）");
            } else {
                println!("⚠️ 未识别的容器头");
            }
            if out != path {
                std::fs::copy(&path, &out)?;
                println!("已复制到: {}", out.display());
            }
        }
        "daily" => print_songs(&app.daily().await?),
        "fm" => {
            let page = app.personal_fm(None, "normal").await?;
            print_songs(&page.items);
            println!("游标: {:?}", page.cursor);
        }
        "stations" => {
            for station in app.stations().await? {
                println!(
                    "{:<10} {:<24} 热度 {}  {}",
                    station.fmid,
                    clip(&station.name, 24),
                    station.heat,
                    station.classid
                );
            }
        }
        "classes" => {
            for class in app.station_classes().await? {
                println!(
                    "{:<8} {:<20} {:>4} 个频道",
                    class.classid,
                    clip(&class.name, 20),
                    class.count
                );
            }
        }
        "station" => {
            let fmid = required(rest, 0, "fmid")?;
            let offset = rest.get(1).and_then(|v| v.parse().ok()).unwrap_or(-1);
            print_songs(&app.station_songs(fmid, offset).await?);
        }
        "vip" => {
            let status = app.vip().await?;
            println!("概念版畅听 VIP 有效: {}", status.is_concept_active());
            println!("到期时间(ms): {:?}", status.concept_expires_at());
            for entry in &status.entries {
                println!(
                    "  {:<10} {:<8} {} → {}",
                    entry.busi_type, entry.product_type, entry.begin_ms, entry.end_ms
                );
            }
        }
        "claim" => {
            let claim = app.claim_daily_vip().await?;
            if claim.already_claimed {
                println!("今天已经领过了（297002 视为幂等成功）");
            } else {
                println!("领取成功，到账 {} 小时", claim.recharge_hours);
            }
        }
        other => return Err(Error::Invalid(format!("未知命令 {other}（试试 lyra help）"))),
    }
    Ok(())
}

/// 获取指定位置的必填命令行参数。
fn required<'a>(args: &'a [String], index: usize, name: &str) -> Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| Error::Invalid(format!("缺少参数 {name}")))
}

/// 解析简要歌曲参数。
fn song_stub(args: &[String]) -> Result<lyra_api::model::Song> {
    let hash = required(args, 0, "hash")?.to_lowercase();
    if hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid(format!("hash 应为 32 位 hex，得到 {hash}")));
    }
    // 专辑 ID 位在音质之前，两者均为可选参数。
    let album_id = args.get(1).cloned().unwrap_or_default();
    if !album_id.is_empty() && !album_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Invalid(format!(
            "专辑 id 应为数字，得到 {album_id}（音质是下一个参数）"
        )));
    }
    Ok(lyra_api::model::Song {
        hash,
        album_id,
        ..lyra_api::model::Song::default()
    })
}

/// 格式化打印歌曲列表。
fn print_songs(songs: &[lyra_api::model::Song]) {
    for (index, song) in songs.iter().enumerate() {
        println!(
            "{:>3}. {:<28} {:<18} {}ms  {}",
            index + 1,
            clip(&song.name, 28),
            clip(&song.singer, 18),
            song.duration_ms,
            song.hash
        );
    }
    println!("共 {} 首", songs.len());
}

/// 裁剪字符串至指定字符宽度。
fn clip(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_owned();
    }
    let head: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{head}…")
}

/// 脱敏敏感令牌字符串（保留前后各 3 个字符）。
fn mask(token: &str) -> String {
    let count = token.chars().count();
    if count <= 6 {
        return "*".repeat(count);
    }
    let head: String = token.chars().take(3).collect();
    let tail: String = token.chars().skip(count - 3).collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn default_data_dir_is_never_empty() {
        // 验证默认数据目录路径非空。
        assert!(!default_data_dir().as_os_str().is_empty());
    }

    #[test]
    fn help_lists_the_milestone_one_path() {
        let text = help();
        for command in ["login", "playlists", "tracks", "url", "download"] {
            assert!(text.contains(command), "帮助里应列出 {command}");
        }
    }

    #[test]
    fn song_stub_validates_the_hash() {
        assert!(song_stub(&["short".to_owned()]).is_err());
        assert!(song_stub(&["z".repeat(32)]).is_err());
        let song = song_stub(&["A".repeat(32), "1234".to_owned()]).unwrap();
        assert_eq!(song.hash, "a".repeat(32), "hash 必须小写");
        assert_eq!(song.album_id, "1234");
    }

    #[test]
    fn masks_tokens() {
        assert_eq!(mask("abcdef123456"), "abc…456");
        assert_eq!(mask("abc"), "***");
    }

    #[test]
    fn clips_wide_text_by_chars_not_bytes() {
        assert_eq!(clip("周杰伦的歌", 10), "周杰伦的歌");
        assert_eq!(clip("周杰伦周杰伦", 4), "周杰伦…");
    }
}
