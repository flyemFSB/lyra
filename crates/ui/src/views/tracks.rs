//! 通用曲目行与桌面端表格渲染组件。

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::Song;

use crate::bridge::{Backend, Msg};
use crate::format::format_duration;
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, CARD_BORDER, ROW_HEIGHT, SURFACE_HOVER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, audio_equalizer,
    friendly_error_message, hex_rgb, skeleton_shimmer, song_cover, sq_badge,
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
    /// 所属专辑
    pub(crate) album: SharedString,
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
            album: SharedString::from(if song.album_name.trim().is_empty() {
                "—"
            } else {
                song.album_name.trim()
            }),
            duration: format_duration(song.duration_ms),
            index: SharedString::from(format!("{:02}", index + 1)),
            cover: SharedString::from(song.cover_at(120)),
        })
        .collect()
}

/// 曲目行渲染所需的上下文
pub(crate) struct RowContext<'a> {
    /// 歌曲队列
    pub(crate) queue: &'a Arc<Vec<Song>>,
    /// 队列的预格式化展示模型
    pub(crate) rows: &'a Arc<Vec<TrackRow>>,
    /// 外壳实体（点击行时替换播放队列）
    pub(crate) shell: &'a WeakEntity<AppShell>,
    /// 后端桥接（收藏、移除等动作）
    pub(crate) backend: &'a Backend,
    /// 当前选中的曲目索引（键盘导航或单击选中态）
    pub(crate) selected_index: Option<usize>,
    /// 单击行时的回调（同时写入选中态，供键盘导航复用）
    pub(crate) on_select: Option<SelectCallback>,
}

/// 单击行回调函数类型
pub(crate) type SelectCallback = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// 渲染普通歌曲行（无移除按钮）
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

/// 桌面端表格表头（轻量 12px 浅灰表头，单行多列对齐）
pub(crate) fn table_header() -> AnyElement {
    h_flex()
        .w_full()
        .h(px(34.0))
        .px_3()
        .gap(px(10.0))
        .items_center()
        .border_b_1()
        .border_color(CARD_BORDER)
        .text_size(px(12.0))
        .text_color(TEXT_FAINT)
        // # 序号 (40px)
        .child(
            div()
                .w(px(40.0))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .child("#"),
        )
        // 标题 (40% 弹性拉伸)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex_grow(8.0)
                .pl_1()
                .child("标题"),
        )
        // 歌手 (25% 弹性拉伸)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex_grow(5.0)
                .child("歌手"),
        )
        // 专辑 (25% 弹性拉伸)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex_grow(5.0)
                .child("专辑"),
        )
        // 时长 & 操作 (100px 固定靠右)
        .child(
            div()
                .w(px(100.0))
                .flex_shrink_0()
                .text_right()
                .pr_3()
                .child("时长"),
        )
        .into_any_element()
}

/// 渲染单首桌面端多列表格曲目行
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

    let play_queue = ctx.queue.clone();
    let play_shell = ctx.shell.clone();
    let like_queue = ctx.queue.clone();
    let like_backend = ctx.backend.clone();
    let is_selected = ctx.selected_index == Some(index);
    let on_select = ctx.on_select.clone();

    let track = h_flex()
        .id(("track", index))
        .group("track_row")
        .role(Role::Button)
        .h(px(ROW_HEIGHT))
        .w_full()
        .px_3()
        .gap(px(10.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .items_center()
        .when(is_current, |this| this.bg(hex_rgb(0xf0fdf4)))
        .when(!is_current && is_selected, |this| {
            this.bg(hex_rgb(0xf3f4f6)).border_1().border_color(hex_rgb(0xd1d5db))
        })
        .when(!is_current && !is_selected, |this| {
            this.hover(|style| style.bg(SURFACE_HOVER))
        })
        .on_click(move |event, window, cx| {
            // 双击的第二下不再重复触发：第一下已经插队播放了
            if event.click_count() >= 2 {
                return;
            }
            if let Some(select_fn) = &on_select {
                // 选中态只用于键盘导航高亮，单击仍然直接播放
                select_fn(index, window, cx);
            }
            play_shell
                .update(cx, |shell, cx| shell.play_queue(play_queue.clone(), index, cx))
                .ok();
        })
        // 1. [ # 序号 / 3柱声波均衡器 (40px 固定居中) ]
        .child(
            div()
                .w(px(40.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .when(is_current, |this| {
                    this.child(audio_equalizer(is_playing, ("track_eq", index)))
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
        // 2. [ 标题列 (40% 弹性拉伸)：封面 (36x36px) + 歌名 + SQ微标签 ]
        .child(
            h_flex()
                .flex_1()
                .min_w(px(0.0))
                .flex_grow(8.0)
                .gap(px(10.0))
                .items_center()
                .child(song_cover(row.cover.as_ref(), row.title.as_ref(), 36.0))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w(px(0.0))
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
                                .text_color(if is_current {
                                    ACCENT_GREEN
                                } else {
                                    TEXT_PRIMARY
                                })
                                .truncate()
                                .child(row.title.clone()),
                        )
                        .child(sq_badge()),
                ),
        )
        // 3. [ 歌手列 (25% 弹性拉伸)：单独一列，悬浮高亮 ]
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex_grow(5.0)
                .text_xs()
                .text_color(TEXT_MUTED)
                .hover(|s| s.text_color(ACCENT_GREEN))
                .truncate()
                .child(row.singer.clone()),
        )
        // 4. [ 专辑列 (25% 弹性拉伸)：展示所属专辑 ]
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex_grow(5.0)
                .text_xs()
                .text_color(TEXT_FAINT)
                .truncate()
                .child(row.album.clone()),
        )
        // 5. [ 时长 & Hover-to-Reveal 交互区 (100px 固定靠右) ]
        .child({
            let remove_backend = ctx.backend.clone();
            let removable = removable_listid.cloned();
            let fileid = song.fileid;
            let title_for_more = row.title.clone();
            let singer_for_more = row.singer.clone();

            div()
                .w(px(100.0))
                .flex_shrink_0()
                .h_full()
                .relative()
                .flex()
                .items_center()
                .justify_end()
                .pr_2()
                // 常态显示时长（若为 0 显示骨架微光；鼠标悬浮时平滑淡出）
                .child(
                    div()
                        .group_hover("track_row", |s| s.opacity(0.0))
                        .text_right()
                        .text_xs()
                        .text_color(TEXT_FAINT)
                        .child(if song.duration_ms == 0 {
                            skeleton_shimmer(34.0, 6.5, ("duration_skel", index))
                        } else {
                            div().child(row.duration.clone()).into_any_element()
                        }),
                )
                // 鼠标 Hover 到该行时时长淡出，浮现 [ 爱心 ] 与 [ ··· 更多 ]
                .child(
                    h_flex()
                        .absolute()
                        .right(px(4.0))
                        .items_center()
                        .gap(px(2.0))
                        .opacity(0.0)
                        .group_hover("track_row", |s| s.opacity(1.0))
                        // [ 爱心 ]
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
                                            Ok(()) => Msg::Toast("已添加到我喜欢".to_owned()),
                                            Err(error) => {
                                                tracing::warn!(%error, "收藏歌曲失败");
                                                Msg::Toast(friendly_error_message(&error.to_string()))
                                            }
                                        }
                                    });
                                }),
                        )
                        // [ ··· 更多操作 ]（从歌单移除 / 复制歌名，拒绝常驻垃圾桶图标）
                        .child(
                            Button::new(("more", index))
                                .ghost()
                                .small()
                                .icon(IconName::Ellipsis)
                                .tooltip(if removable.is_some() {
                                    "从歌单移除"
                                } else {
                                    "复制歌名"
                                })
                                .block_mouse_except_scroll()
                                .on_click(move |_event, _window, cx| {
                                    if let Some(listid) = &removable {
                                        remove_backend.send(Msg::RemoveTrack {
                                            listid: listid.to_string(),
                                            fileid,
                                        });
                                    } else {
                                        cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                            "{singer_for_more} - {title_for_more}"
                                        )));
                                        remove_backend.send(Msg::Toast(format!(
                                            "已复制: {singer_for_more} - {title_for_more}"
                                        )));
                                    }
                                }),
                        ),
                )
        });

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
        let songs = vec![
            song("a", "夜曲", "周杰伦", 226_000),
            song("b", "晴天", "周杰伦", 269_000),
        ];
        let rows: Vec<TrackRow> = build_rows(&songs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title.as_ref(), "夜曲");
        assert_eq!(rows[0].singer.as_ref(), "周杰伦");
        assert_eq!(rows[0].album.as_ref(), "—");
        assert_eq!(rows[0].duration.as_ref(), "3:46");
        assert_eq!(rows[0].index.as_ref(), "01");
        assert_eq!(rows[1].index.as_ref(), "02");
        assert_eq!(rows[1].duration.as_ref(), "4:29");
    }

    #[test]
    fn the_row_model_does_not_clone_the_song() {
        let songs = vec![song("a", "夜曲", "周杰伦", 226_000)];
        let rows = build_rows(&songs);
        assert_eq!(rows[0].title.as_ref(), "夜曲");
        assert_eq!(songs[0].hash, "a");
    }
}
