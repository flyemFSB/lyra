//! 歌曲列表项共用渲染逻辑。

use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::*;

use crate::bridge::{Backend, Msg};
use crate::format::format_duration;
use crate::shell::AppShell;
use crate::theme::{ROW_HEIGHT, cover_box};

/// 渲染单行歌曲列表项组件。
pub(crate) fn track_row(
    index: usize,
    queue: &Arc<Vec<lyra_api::model::Song>>,
    shell: &WeakEntity<AppShell>,
    backend: &Backend,
    removable: Option<(SharedString, i64)>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let Some(song) = queue.get(index) else {
        return div().into_any_element();
    };

    let play_queue = queue.clone();
    let play_shell = shell.clone();
    let like_backend = backend.clone();
    let like_song = song.clone();

    let mut row = h_flex()
        .id(("track", index))
        .h(px(ROW_HEIGHT))
        .w_full()
        .px_3()
        .gap(px(12.0))
        .cursor_pointer()
        .hover(|style| style.bg(theme.list_hover))
        .on_click(move |_event, _window, cx| {
            play_shell
                .update(cx, |shell, cx| shell.play_queue(play_queue.clone(), index, cx))
                .ok();
        })
        .child(
            div()
                .w(px(28.0))
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!("{}", index + 1))),
        )
        .child(cover_box(&song.name, 36.0, cx))
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.0))
                .child(div().text_sm().truncate().child(SharedString::from(song.name.as_str())))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(SharedString::from(song.singer.as_str())),
                ),
        )
        .child(
            div()
                .w(px(56.0))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format_duration(song.duration_ms))),
        )
        .child(
            Button::new(("like", index))
                .ghost()
                .small()
                .label("♥")
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

    if let Some((listid, fileid)) = removable {
        let remove_backend = backend.clone();
        row = row.child(Button::new(("remove", index)).ghost().small().label("移除").on_click(
            move |_event, _window, _cx| {
                remove_backend.send(Msg::RemoveTrack {
                    listid: listid.to_string(),
                    fileid,
                });
            },
        ));
    }

    row.into_any_element()
}
