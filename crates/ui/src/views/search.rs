//! 歌曲搜索与结果列表视图。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, Sizable as _, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::Song;

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CARD_BORDER, TEXT_MUTED, badge, chip, hex_rgb,
    notice_with_icon, row_sizes, title,
};
use crate::views::player::PlayerModel;
use crate::views::tracks::{TrackRow, build_rows, track_row};

/// 快捷热门搜索词
const HOT_TAGS: &[&str] = &[
    "周杰伦",
    "陈奕迅",
    "林俊杰",
    "王菲",
    "轻音乐",
    "国风民乐",
    "粤语经典",
    "欧美流行",
];

/// 搜索视图
pub(crate) struct SearchView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    input: Entity<InputState>,
    songs: Arc<Vec<Song>>,
    /// 预格式化的曲目展示模型
    rows: Arc<Vec<TrackRow>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll_handle: VirtualListScrollHandle,
    loading: bool,
    /// 是否已触发过搜索
    searched: bool,
    error: Option<SharedString>,
    /// 请求序号（用于丢弃过期响应）
    request_seq: u64,
    _subs: Vec<Subscription>,
}

impl SearchView {
    /// 创建搜索视图并监听输入与播放器事件
    pub(crate) fn new(
        backend: Backend,
        shell: WeakEntity<AppShell>,
        player: Entity<PlayerModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索歌曲、歌手或专辑，回车检索"));
        let input_sub = cx.subscribe(&input, |this, _state, event: &InputEvent, cx| {
            match event {
                InputEvent::PressEnter { .. } => this.submit(cx),
                // 输入框只让自己失效，父实体不会自动重绘；
                // 不转发 Change 的话「清空输入」按钮永远不出现。
                InputEvent::Change | InputEvent::Focus | InputEvent::Blur => cx.notify(),
            }
        });
        let player_sub = cx.observe(&player, |_this, _player, cx| {
            cx.notify();
        });

        Self {
            backend,
            shell,
            player,
            input,
            songs: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            sizes: row_sizes(0),
            scroll_handle: VirtualListScrollHandle::new(),
            loading: false,
            searched: false,
            error: None,
            request_seq: 0,
            _subs: vec![input_sub, player_sub],
        }
    }

    /// 重置视图状态并废弃在途请求
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.songs = Arc::new(Vec::new());
        self.rows = Arc::new(Vec::new());
        self.sizes = row_sizes(0);
        self.scroll_handle = VirtualListScrollHandle::new();
        self.loading = false;
        self.searched = false;
        self.error = None;
        self.request_seq += 1;
        cx.notify();
    }

    /// 提交当前输入关键词
    pub(crate) fn submit(&mut self, cx: &mut Context<Self>) {
        let keyword = self.input.read(cx).value().to_string();
        self.execute_search(keyword, cx);
    }

    /// 按指定关键词执行检索
    pub(crate) fn search_keyword(&mut self, keyword: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, input_cx| {
            input.set_value(keyword.to_owned(), window, input_cx);
        });
        self.execute_search(keyword.to_owned(), cx);
    }

    /// 发起搜索请求
    fn execute_search(&mut self, keyword: String, cx: &mut Context<Self>) {
        if !is_searchable(&keyword) {
            return;
        }
        self.loading = true;
        self.error = None;
        self.scroll_handle = VirtualListScrollHandle::new();
        self.request_seq += 1;
        let seq = self.request_seq;
        cx.notify();

        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Search(seq, app.search(&keyword, 1).await.map_err(|error| error.to_string())) });
    }

    /// 处理搜索响应结果（丢弃过期请求）
    pub(crate) fn on_search(&mut self, seq: u64, result: Result<Vec<Song>, String>, cx: &mut Context<Self>) {
        if seq != self.request_seq {
            return;
        }
        self.loading = false;
        self.searched = true;
        match result {
            Ok(songs) => {
                self.rows = Arc::new(build_rows(&songs));
                self.songs = Arc::new(songs);
                self.sizes = row_sizes(self.songs.len());
                self.error = None;
            }
            Err(error) => {
                self.songs = Arc::new(Vec::new());
                self.rows = Arc::new(Vec::new());
                self.sizes = row_sizes(0);
                self.error = Some(error.into());
            }
        }
        cx.notify();
    }

    /// 渲染单首曲目行
    fn row(&self, index: usize, cur_hash: Option<&str>, is_playing: bool, cx: &App) -> AnyElement {
        let this_hash = self.songs.get(index).map(|s| s.hash.as_str());
        let is_current = cur_hash.is_some() && cur_hash == this_hash;
        let Some(row) = self.rows.get(index) else {
            return div().into_any_element();
        };
        track_row(
            index,
            row,
            &self.songs,
            &self.shell,
            &self.backend,
            None,
            is_current,
            is_playing,
            cx,
        )
    }
}

/// 检查关键词是否有效（非空且非全空白）
fn is_searchable(keyword: &str) -> bool {
    !keyword.trim().is_empty()
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_input_text = !self.input.read(cx).value().is_empty();

        let mut header = v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap(px(12.0))
            .border_b_1()
            .border_color(CARD_BORDER)
            .child(
                h_flex()
                    .w_full()
                    .gap(px(10.0))
                    .items_center()
                    .child(title("发现歌曲", cx))
                    .child(badge(
                        format!("{} 条搜索结果", self.songs.len()),
                        ACCENT_GREEN_LIGHT,
                        ACCENT_GREEN_HOVER,
                    ))
                    .child(div().flex_1())
                    .when(!self.songs.is_empty(), |this| {
                        let songs = self.songs.clone();
                        let shell = self.shell.clone();
                        this.child(
                            Button::new("search_play_all")
                                .primary()
                                .small()
                                .icon(IconName::Play)
                                .label("播放全部")
                                .tooltip("按顺序播放全部检索结果")
                                .on_click(move |_event, _window, cx| {
                                    shell.update(cx, |shell, cx| shell.play_queue(songs.clone(), 0, cx)).ok();
                                }),
                        )
                    }),
            )
            // 搜索输入栏
            .child(
                h_flex()
                    .w_full()
                    .gap(px(10.0))
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .px_3()
                            .py(px(4.0))
                            .rounded(px(10.0))
                            .bg(hex_rgb(0xf2f4f7))
                            .border_1()
                            .border_color(CARD_BORDER)
                            .gap(px(8.0))
                            .items_center()
                            .child(Icon::new(IconName::Search).size(px(16.0)).text_color(ACCENT_GREEN))
                            .child(div().flex_1().child(Input::new(&self.input).w_full()))
                            .when(has_input_text, |this| {
                                this.child(
                                    Button::new("clear_input")
                                        .ghost()
                                        .small()
                                        .icon(IconName::X)
                                        .tooltip("清空输入")
                                        .on_click(cx.listener(|this, _event, window, cx| {
                                            this.input.update(cx, |input, input_cx| {
                                                input.set_value(String::new(), window, input_cx);
                                            });
                                        })),
                                )
                            }),
                    )
                    .child(
                        Button::new("search-go")
                            .primary()
                            .small()
                            .icon(IconName::Search)
                            .label("搜索")
                            .on_click(cx.listener(|this, _event, _window, cx| this.submit(cx))),
                    ),
            );

        // 热门搜索词标签栏
        let mut hot_row = h_flex().gap(px(8.0)).items_center().flex_wrap();
        hot_row = hot_row.child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(TEXT_MUTED)
                .child("热门搜索："),
        );
        for (index, tag) in HOT_TAGS.iter().enumerate() {
            // 热门词是 'static 字符串，直接借用，不构造中间 String
            let kw: &'static str = tag;
            hot_row = hot_row.child(
                chip(kw, false, Some(IconName::Sparkles))
                    .id(("hot_tag", index))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.search_keyword(kw, window, cx);
                    })),
            );
        }
        header = header.child(hot_row);

        let body = if self.loading {
            notice_with_icon(IconName::RefreshCw, "正在全网曲库检索中…", cx)
        } else if let Some(error) = &self.error {
            notice_with_icon(IconName::CircleAlert, error.clone(), cx)
        } else if !self.searched {
            notice_with_icon(
                IconName::Search,
                "输入歌曲、歌手或专辑关键字，或点击上方热搜标签探索音乐",
                cx,
            )
        } else if self.songs.is_empty() {
            notice_with_icon(IconName::Search, "没有找到相关歌曲，换个关键词试试吧", cx)
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("search-results")),
                sizes,
                move |view, range, _window, cx| {
                    let app: &App = cx;
                    // 播放器状态整批读取一次，避免每行都做一次实体读取
                    let (cur_hash, is_playing) = {
                        let model = view.player.read(app);
                        (
                            model.current_song().map(|s| s.hash.clone()),
                            model.snapshot.state == lyra_audio::State::Playing,
                        )
                    };
                    range
                        .map(|index| view.row(index, cur_hash.as_deref(), is_playing, app))
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.scroll_handle)
            .into_any_element()
        };

        v_flex().size_full().child(header).child(
            div()
                .relative()
                .flex_1()
                .min_h(px(0.0))
                .p_2()
                .vertical_scrollbar(&self.scroll_handle)
                .child(body),
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

    use super::is_searchable;

    #[test]
    fn blank_keywords_are_not_submitted() {
        assert!(!is_searchable(""));
        assert!(!is_searchable("   "));
        assert!(is_searchable("夜曲"));
        assert!(is_searchable("  周杰伦  "));
    }
}
