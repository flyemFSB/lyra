//! 滚动歌词视图组件。

use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::LyricLine;

use crate::format::lyric_index;
use crate::theme::notice;
use crate::views::player::PlayerModel;

/// 歌词展示视图结构。
pub(crate) struct LyricsView {
    player: Entity<PlayerModel>,
    lines: Arc<Vec<LyricLine>>,
    /// 预构建的歌词行文本集合。
    texts: Arc<Vec<SharedString>>,
    title: SharedString,
    singer: SharedString,
    current_hash: Option<String>,
    status: Option<SharedString>,
    _sub: Subscription,
}

impl LyricsView {
    /// 创建歌词视图并监听播放进度。
    pub(crate) fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&player, |this, _player, cx| {
            this.refresh_title(cx);
            cx.notify();
        });
        Self {
            player,
            lines: Arc::new(Vec::new()),
            texts: Arc::new(Vec::new()),
            title: SharedString::new_static("未在播放"),
            singer: SharedString::new_static(""),
            current_hash: None,
            status: None,
            _sub: sub,
        }
    }

    /// 响应处理歌曲歌词数据。
    pub(crate) fn set_lines(&mut self, result: Result<Vec<LyricLine>, String>, cx: &mut Context<Self>) {
        match result {
            Ok(lines) => {
                self.texts = Arc::new(
                    lines
                        .iter()
                        .map(|line| SharedString::from(line.text.as_str()))
                        .collect(),
                );
                self.lines = Arc::new(lines);
                self.status = None;
            }
            Err(error) => {
                self.lines = Arc::new(Vec::new());
                self.texts = Arc::new(Vec::new());
                self.status = Some(error.into());
            }
        }
        cx.notify();
    }

    /// 歌曲切换时刷新标题与歌手缓存。
    fn refresh_title(&mut self, cx: &Context<Self>) {
        let (hash, title, singer) = self.player.read(cx).current_labels();
        if hash == self.current_hash {
            return;
        }
        self.current_hash = hash;
        self.title = title;
        self.singer = singer;
    }
}

impl Render for LyricsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let position = self.player.read(cx).snapshot.position_ms;
        let current = lyric_index(&self.lines, position);

        let header = v_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap(px(2.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.singer.clone()),
            );

        let body = if let Some(status) = &self.status {
            notice(status.clone(), cx)
        } else if self.lines.is_empty() {
            notice("暂无歌词（播放一首歌后自动加载）", cx)
        } else {
            let mut list = v_flex().w_full().py_8().gap(px(14.0));
            // 复用预拼装文本引用避免逐行分配。
            for (index, text) in self.texts.iter().enumerate() {
                let active = current == Some(index);
                list = list.child(
                    div()
                        .w_full()
                        .px_6()
                        .text_center()
                        .when(active, |this| {
                            this.text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.primary)
                        })
                        .when(!active, |this| this.text_sm().text_color(theme.muted_foreground))
                        .child(text.clone()),
                );
            }
            div()
                .id("lyrics-body")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .child(list)
                .into_any_element()
        };

        v_flex().size_full().child(header).child(body)
    }
}
