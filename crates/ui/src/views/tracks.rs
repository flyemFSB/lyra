//! 通用曲目行渲染组件。

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::Song;

use crate::bridge::{Backend, Msg};
use crate::format::format_duration;
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ROW_HEIGHT, SURFACE_HOVER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY,
    friendly_error_message, hex_rgb, song_cover, sq_badge,
};
use crate::views::player::PlayerModel;

/// 曲目行的展示模型
///
/// GPUI 的 `render` 每帧都会重跑，因此不能在渲染期构造字符串。
/// 所有展示文本在数据到达时一次性预格式化，渲染时只做引用计数克隆。
pub(crate) struct TrackRow {
    /// 主标题（歌名）
    pub(crate) title: SharedString,
    /// 副标题（歌手名）
    pub(crate) singer: SharedString,
    /// 时长文本
    pub(crate) duration: SharedString,
    /// 序号文本（01、02…）
    pub(crate) index: SharedString,
    /// 封面图片 URL
    pub(crate) cover: SharedString,
}

/// 为一组歌曲预构建展示模型
pub(crate) fn build_rows(songs: &[Song]) -> Vec<TrackRow> {
    songs
        .iter()
        .enumerate()
        .map(|(index, song)| TrackRow {
            title: SharedString::from(song.display_title()),
            singer: SharedString::from(song.display_singer()),
            duration: format_duration(song.duration_ms),
            index: SharedString::from(format!("{:02}", index + 1)),
            cover: SharedString::from(song.cover_at(120)),
        })
        .collect()
}

/// 曲目行渲染所需的上下文
///
/// 把「队列 + 外壳 + 后端」三样总是成对出现的东西收在一起，
/// 免得每行都传三个句柄。
pub(crate) struct RowContext<'a> {
    /// 歌曲队列
    pub(crate) queue: &'a Arc<Vec<Song>>,
    /// 队列的预格式化展示模型
    pub(crate) rows: &'a Arc<Vec<TrackRow>>,
    /// 外壳实体（点击行时替换播放队列）
    pub(crate) shell: &'a WeakEntity<AppShell>,
    /// 后端桥接（收藏、移除等动作）
    pub(crate) backend: &'a Backend,
}

/// 渲染普通歌曲行（无移除按钮）
///
/// 四个列表视图的行渲染完全相同，只有「可移除」这一项不同。
pub(crate) fn song_row(index: usize, ctx: &RowContext<'_>, cur_hash: Option<&str>, is_playing: bool) -> AnyElement {
    let this_hash = ctx.queue.get(index).map(|song| song.hash.as_str());
    let is_current = cur_hash.is_some() && cur_hash == this_hash;
    let Some(row) = ctx.rows.get(index) else {
        return div().into_any_element();
    };
    track_row(index, row, ctx, None, is_current, is_playing)
}

/// 读取当前播放状态（整批读取一次，避免每行都做一次实体读取）
pub(crate) fn current_play_state(player: &Entity<PlayerModel>, cx: &App) -> (Option<String>, bool) {
    let model = player.read(cx);
    (
        model.current_song().map(|song| song.hash.clone()),
        model.snapshot.state == lyra_audio::State::Playing,
    )
}

/// 渲染单首曲目行
pub(crate) fn track_row(
    index: usize,
    row: &TrackRow,
    ctx: &RowContext<'_>,
    removable_listid: Option<&SharedString>,
    is_current: bool,
    is_playing: bool,
) -> AnyElement {
    let Some(song) = ctx.queue.get(index) else {
        return div().into_any_element();
    };

    // 队列是 `Arc`，克隆只是引用计数递增；曲目本体等到真正点击「收藏」时再克隆，
    // 避免渲染期把整首曲目（10 个 String 字段）复制一遍。
    let play_queue = ctx.queue.clone();
    let play_shell = ctx.shell.clone();
    let like_queue = ctx.queue.clone();
    let like_backend = ctx.backend.clone();

    let mut track = h_flex()
        .id(("track", index))
        .role(Role::Button)
        .h(px(ROW_HEIGHT))
        .w_full()
        .px_3()
        .gap(px(10.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .items_center()
        .when(is_current, |this| this.bg(hex_rgb(0xf2f4f7)))
        .when(!is_current, |this| this.hover(|style| style.bg(SURFACE_HOVER)))
        .on_click(move |_event, _window, cx| {
            play_shell
                .update(cx, |shell, cx| shell.play_queue(play_queue.clone(), index, cx))
                .ok();
        })
        // [ 序号 / 动态频谱 (24px) ]
        .child(
            div()
                .w(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .when(is_current, |this| {
                    if is_playing {
                        this.child(crate::motion::pulsating_icon(
                            Icon::new(IconName::AudioLines).size(px(15.0)).text_color(ACCENT_GREEN),
                            ("track_wave", index),
                        ))
                    } else {
                        this.child(Icon::new(IconName::Music).size(px(14.0)).text_color(ACCENT_GREEN))
                    }
                })
                .when(!is_current, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(TEXT_FAINT)
                            .child(row.index.clone()),
                    )
                }),
        )
        // [ 封面 (40x40px, rounded 6px，支持真实封面图与优雅矢量降级) ]
        .child(song_cover(row.cover.as_ref(), row.title.as_ref(), 40.0))
        // [ 歌名 + SQ微标签 ]
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if is_current {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(TEXT_PRIMARY)
                                .truncate()
                                .child(row.title.clone()),
                        )
                        .child(sq_badge()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(TEXT_MUTED)
                        .truncate()
                        .child(row.singer.clone()),
                ),
        )
        // [ 时长 (靠右, 等宽对齐) ]
        .child(
            div()
                .w(px(52.0))
                .text_right()
                .text_xs()
                .text_color(TEXT_FAINT)
                .child(row.duration.clone()),
        )
        // [ 悬浮/行内操作图标 ]
        .child(
            Button::new(("like", index))
                .ghost()
                .small()
                .icon(IconName::Heart)
                .tooltip("收藏到我喜欢")
                .block_mouse_except_scroll()
                .on_click(move |_event, _window, _cx| {
                    let Some(song) = like_queue.get(index) else {
                        return;
                    };
                    let song = song.clone();
                    let app = like_backend.app();
                    like_backend.spawn(async move {
                        match app.like(&song).await {
                            Ok(()) => Msg::Noop,
                            Err(error) => {
                                tracing::warn!(%error, "收藏歌曲失败");
                                Msg::Toast(friendly_error_message(&error.to_string()))
                            }
                        }
                    });
                }),
        );

    // 移除按钮（仅自建歌单可用）
    if let Some(listid) = removable_listid {
        let remove_backend = ctx.backend.clone();
        let listid = listid.clone();
        let fileid = song.fileid;
        track = track.child(
            Button::new(("remove", index))
                .ghost()
                .small()
                .icon(IconName::Trash)
                .tooltip("从歌单移除")
                .block_mouse_except_scroll()
                .on_click(move |_event, _window, _cx| {
                    remove_backend.send(Msg::RemoveTrack {
                        listid: listid.to_string(),
                        fileid,
                    });
                }),
        );
    }

    track.into_any_element()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

    use super::{TrackRow, build_rows};
    use lyra_api::model::Song;

    fn song(hash: &str, name: &str, singer: &str, duration_ms: u64) -> Song {
        Song {
            hash: hash.to_owned(),
            name: name.to_owned(),
            singer: singer.to_owned(),
            duration_ms,
            ..Song::default()
        }
    }

    #[test]
    fn rows_are_preformatted_so_render_does_not_allocate() {
        // 展示模型必须在数据层就把文本准备好：
        // 渲染期只做引用计数克隆，不再构造字符串。
        let songs = vec![
            song("a", "夜曲", "周杰伦", 226_000),
            song("b", "晴天", "周杰伦", 269_000),
        ];
        let rows: Vec<TrackRow> = build_rows(&songs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title.as_ref(), "夜曲");
        assert_eq!(rows[0].singer.as_ref(), "周杰伦");
        assert_eq!(rows[0].duration.as_ref(), "3:46");
        assert_eq!(rows[0].index.as_ref(), "01");
        assert_eq!(rows[1].index.as_ref(), "02");
        assert_eq!(rows[1].duration.as_ref(), "4:29");
    }

    #[test]
    fn the_row_model_does_not_clone_the_song() {
        // 展示模型只持有预格式化的文本与索引，不复制曲目本体：
        // 曲目克隆留给「收藏」按钮点击时按需进行。
        let songs = vec![song("a", "夜曲", "周杰伦", 226_000)];
        let rows = build_rows(&songs);
        assert_eq!(rows[0].title.as_ref(), "夜曲");
        // 原始曲目仍在，模型构建没有把它搬走
        assert_eq!(songs[0].hash, "a");
    }
}
