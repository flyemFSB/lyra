//! 歌曲列表项共用渲染逻辑：简约淡雅排版、动态翡翠绿声波指示与纯图标交互。

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::bridge::{Backend, Msg};
use crate::format::format_duration;
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_BORDER, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, ROW_HEIGHT, SURFACE_HOVER, TEXT_FAINT,
    TEXT_MUTED, TEXT_PRIMARY, badge, cover_box,
};

/// 渲染单行歌曲列表项组件（支持当前播放高亮与声波指示）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn track_row(
    index: usize,
    queue: &Arc<Vec<lyra_api::model::Song>>,
    shell: &WeakEntity<AppShell>,
    backend: &Backend,
    removable: Option<(SharedString, i64)>,
    is_current: bool,
    is_playing: bool,
    cx: &App,
) -> AnyElement {
    let Some(song) = queue.get(index) else {
        return div().into_any_element();
    };

    let play_queue = queue.clone();
    let play_shell = shell.clone();
    let like_backend = backend.clone();
    let like_song = song.clone();

    let mut row = h_flex()
        .id(("track", index))
        .role(Role::Button)
        .h(px(ROW_HEIGHT))
        .w_full()
        .px_3()
        .gap(px(12.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .when(is_current, |this| {
            this.bg(ACCENT_GREEN_LIGHT)
                .border_1()
                .border_color(ACCENT_GREEN_BORDER)
        })
        .when(!is_current, |this| {
            this.hover(|style| style.bg(SURFACE_HOVER))
        })
        .on_click(move |_event, _window, cx| {
            play_shell
                .update(cx, |shell, cx| shell.play_queue(play_queue.clone(), index, cx))
                .ok();
        })
        // 序号列或正在播放声波指示器
        .child(
            div()
                .w(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .when(is_current, |this| {
                    let icon = if is_playing {
                        IconName::AudioLines
                    } else {
                        IconName::Music
                    };
                    this.child(Icon::new(icon).size(px(16.0)).text_color(ACCENT_GREEN))
                })
                .when(!is_current, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(TEXT_FAINT)
                            .child(SharedString::from(format!("{:02}", index + 1))),
                    )
                }),
        )
        // 封面图
        .child(
            div()
                .when(is_current, |this| {
                    this.rounded(px(8.0))
                        .border_1()
                        .border_color(ACCENT_GREEN_BORDER)
                })
                .child(cover_box(&song.name, 36.0, cx)),
        )
        // 标题与歌手信息
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
                                    FontWeight::SEMIBOLD
                                })
                                .text_color(if is_current {
                                    ACCENT_GREEN_HOVER
                                } else {
                                    TEXT_PRIMARY
                                })
                                .truncate()
                                .child(SharedString::from(song.name.as_str())),
                        )
                        .child(badge("SQ", ACCENT_GREEN_LIGHT, ACCENT_GREEN)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(TEXT_MUTED)
                        .truncate()
                        .child(SharedString::from(song.singer.as_str())),
                ),
        )
        // 时长
        .child(
            div()
                .w(px(56.0))
                .text_xs()
                .text_color(TEXT_FAINT)
                .child(SharedString::from(format_duration(song.duration_ms))),
        )
        // 收藏按钮
        .child(
            Button::new(("like", index))
                .ghost()
                .small()
                .icon(IconName::Heart)
                .tooltip("收藏到我喜欢")
                .block_mouse_except_scroll()
                .on_click(move |_event, _window, _cx| {
                    let app = like_backend.app();
                    let song = like_song.clone();
                    like_backend.spawn(async move {
                        Msg::Toast(match app.like(&song).await {
                            Ok(()) => "已加入「我喜欢」".to_owned(),
                            Err(error) => format!("收藏失败: {error}"),
                        })
                    });
                }),
        );

    // 移除按钮（当且仅当属于可编辑歌单时展示）
    if let Some((listid, fileid)) = removable {
        let remove_backend = backend.clone();
        row = row.child(
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

    row.into_any_element()
}
