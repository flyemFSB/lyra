//! 歌曲搜索与结果列表视图（桌面端单搜索入口架构）。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Disableable as _, Icon, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::{Page, Song};

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_LIGHT, CARD_BORDER, SURFACE_HOVER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, badge,
    friendly_error_message, hex_rgb, notice_with_icon, row_sizes, title,
};
use crate::views::player::PlayerModel;
use crate::views::tracks::{RowContext, TrackRow, build_rows, current_play_state, song_row, table_header};

/// 提前预取页数：滚动到距末尾不足这么多行时自动拉下一页
const PREFETCH_ROWS: usize = 20;

/// 搜索类型标签页
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SearchTab {
    #[default]
    Songs,
    Albums,
    Artists,
    Playlists,
}

/// 排序筛选条件
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SearchSort {
    #[default]
    Default,
    Latest,
    Hottest,
}

/// 搜索视图
pub(crate) struct SearchView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    pub(crate) query: String,
    pub(crate) tab: SearchTab,
    pub(crate) sort: SearchSort,
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
    /// 下一页页码（None 表示已加载完毕）
    next_page: Option<u32>,
    /// 是否为追加分页数据
    appending: bool,
    _subs: Vec<Subscription>,
}

impl SearchView {
    /// 创建搜索视图并监听播放器事件
    pub(crate) fn new(
        backend: Backend,
        shell: WeakEntity<AppShell>,
        player: Entity<PlayerModel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let player_sub = cx.observe(&player, |_this, _player, cx| {
            cx.notify();
        });

        Self {
            backend,
            shell,
            player,
            query: String::new(),
            tab: SearchTab::Songs,
            sort: SearchSort::Default,
            songs: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            sizes: row_sizes(0),
            scroll_handle: VirtualListScrollHandle::new(),
            loading: false,
            searched: false,
            error: None,
            request_seq: 0,
            next_page: None,
            appending: false,
            _subs: vec![player_sub],
        }
    }

    /// 重置视图状态并废弃在途请求
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.query.clear();
        self.songs = Arc::new(Vec::new());
        self.rows = Arc::new(Vec::new());
        self.sizes = row_sizes(0);
        self.scroll_handle = VirtualListScrollHandle::new();
        self.loading = false;
        self.searched = false;
        self.error = None;
        self.next_page = None;
        self.appending = false;
        self.request_seq += 1;
        cx.notify();
    }

    /// 发起全网搜索请求
    pub(crate) fn execute_search(&mut self, keyword: &str, cx: &mut Context<Self>) {
        if !is_searchable(keyword) {
            return;
        }
        keyword.trim().clone_into(&mut self.query);
        self.loading = true;
        self.error = None;
        self.next_page = None;
        self.appending = false;
        self.scroll_handle = VirtualListScrollHandle::new();
        self.request_seq += 1;
        let seq = self.request_seq;
        cx.notify();

        let app = self.backend.app();
        let kw = self.query.clone();
        self.backend
            .spawn(async move { Msg::Search(seq, app.search(&kw, 1).await.map_err(|error| error.to_string())) });
    }

    /// 加载下一页搜索结果
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.appending {
            return;
        }
        let Some(page) = self.next_page else {
            return;
        };
        self.appending = true;
        self.request_seq += 1;
        let seq = self.request_seq;
        cx.notify();
        let app = self.backend.app();
        let kw = self.query.clone();
        self.backend
            .spawn(async move { Msg::Search(seq, app.search(&kw, page).await.map_err(|error| error.to_string())) });
    }

    /// 虚拟列表滚到接近底部时自动预取下一页（不限量滚动加载）
    fn maybe_load_more(&mut self, visible_end: usize, cx: &mut Context<Self>) {
        if self.next_page.is_some() && visible_end + PREFETCH_ROWS >= self.songs.len() {
            self.load_more(cx);
        }
    }

    /// 处理搜索响应结果（丢弃过期请求）
    pub(crate) fn on_search(&mut self, seq: u64, result: Result<Page<Song>, String>, cx: &mut Context<Self>) {
        if seq != self.request_seq {
            return;
        }
        let appending = std::mem::take(&mut self.appending);
        self.loading = false;
        self.searched = true;
        match result {
            Ok(page) => {
                // 空页不能留下游标，否则滚动预取会无限空转
                self.next_page = if page.items.is_empty() {
                    None
                } else {
                    page.cursor.as_deref().and_then(|cursor| cursor.parse().ok())
                };
                // 追加时优先零拷贝接管旧列表（队列仍持有引用时才退化为深拷贝）
                let mut songs = match Arc::try_unwrap(std::mem::replace(&mut self.songs, Arc::new(Vec::new()))) {
                    Ok(owned) => owned,
                    Err(shared) => (*shared).clone(),
                };
                if !appending {
                    songs.clear();
                }
                songs.extend(page.items);
                self.rows = Arc::new(build_rows(&songs));
                self.songs = Arc::new(songs);
                self.sizes = row_sizes(self.songs.len());
                self.error = None;
            }
            Err(error) => {
                tracing::warn!(%error, "搜索歌曲失败");
                if appending {
                    // 翻页失败不能清空已经拿到的结果
                    self.backend.send(Msg::Toast(friendly_error_message(&error)));
                } else {
                    self.next_page = None;
                    self.songs = Arc::new(Vec::new());
                    self.rows = Arc::new(Vec::new());
                    self.sizes = row_sizes(0);
                    self.error = Some(friendly_error_message(&error).into());
                }
            }
        }
        cx.notify();
    }
}

/// 检查关键词是否有效（非空且非全空白）
pub(crate) fn is_searchable(keyword: &str) -> bool {
    !keyword.trim().is_empty()
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_results = !self.songs.is_empty();
        let songs_for_play = self.songs.clone();
        let shell_for_play = self.shell.clone();

        let header_title = if self.query.is_empty() {
            "搜索音乐".to_owned()
        } else {
            format!("“{}” 的搜索结果", self.query)
        };

        // 桌面端干净统一的信息架构：顶部无冗余输入框，呈现标题、播放全部、Tab 切换与排序筛选
        let header = v_flex()
            .w_full()
            .px_6()
            .pt_5()
            .pb_3()
            .gap(px(14.0))
            .border_b_1()
            .border_color(CARD_BORDER)
            // 第一行：标题 + 结果数 + 播放全部 (主色胶囊)
            .child(
                h_flex()
                    .w_full()
                    .gap(px(12.0))
                    .items_center()
                    .child(title(header_title))
                    .when(self.searched, |this| {
                        this.child(badge(
                            format!("{} 首单曲", self.songs.len()),
                            ACCENT_GREEN_LIGHT,
                            ACCENT_GREEN,
                        ))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("search_play_all")
                            .primary()
                            .icon(IconName::Play)
                            .label("播放全部")
                            .tooltip("按顺序播放全部检索结果")
                            .disabled(!has_results)
                            .on_click(move |_event, _window, cx| {
                                shell_for_play
                                    .update(cx, |shell, cx| shell.play_queue(songs_for_play.clone(), 0, cx))
                                    .ok();
                            }),
                    ),
            )
            // 第二行：Tab 切换（单曲 / 专辑 / 歌手 / 歌单）与 筛选排序
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    // 左侧 Tab 切换
                    .child(
                        h_flex()
                            .gap(px(4.0))
                            .items_center()
                            .child({
                                let active = self.tab == SearchTab::Songs;
                                div()
                                    .id("tab_songs")
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .px_3()
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .text_xs()
                                    .when(active, |s| {
                                        s.bg(hex_rgb(0xf3f4f6))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(ACCENT_GREEN)
                                    })
                                    .when(!active, |s| s.text_color(TEXT_MUTED).hover(|h| h.bg(SURFACE_HOVER)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.tab = SearchTab::Songs;
                                        cx.notify();
                                    }))
                                    .child("单曲")
                            })
                            .child({
                                let active = self.tab == SearchTab::Albums;
                                div()
                                    .id("tab_albums")
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .px_3()
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .text_xs()
                                    .when(active, |s| {
                                        s.bg(hex_rgb(0xf3f4f6))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(ACCENT_GREEN)
                                    })
                                    .when(!active, |s| s.text_color(TEXT_MUTED).hover(|h| h.bg(SURFACE_HOVER)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.tab = SearchTab::Albums;
                                        cx.notify();
                                    }))
                                    .child("专辑")
                            })
                            .child({
                                let active = self.tab == SearchTab::Artists;
                                div()
                                    .id("tab_artists")
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .px_3()
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .text_xs()
                                    .when(active, |s| {
                                        s.bg(hex_rgb(0xf3f4f6))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(ACCENT_GREEN)
                                    })
                                    .when(!active, |s| s.text_color(TEXT_MUTED).hover(|h| h.bg(SURFACE_HOVER)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.tab = SearchTab::Artists;
                                        cx.notify();
                                    }))
                                    .child("歌手")
                            })
                            .child({
                                let active = self.tab == SearchTab::Playlists;
                                div()
                                    .id("tab_playlists")
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .px_3()
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .text_xs()
                                    .when(active, |s| {
                                        s.bg(hex_rgb(0xf3f4f6))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(ACCENT_GREEN)
                                    })
                                    .when(!active, |s| s.text_color(TEXT_MUTED).hover(|h| h.bg(SURFACE_HOVER)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.tab = SearchTab::Playlists;
                                        cx.notify();
                                    }))
                                    .child("歌单")
                            }),
                    )
                    // 右侧排序筛选：[ 综合排序 ]、[ 最新 ]、[ 最热 ]
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .text_xs()
                            .child({
                                let active = self.sort == SearchSort::Default;
                                div()
                                    .id("sort_default")
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .when(active, |s| s.font_weight(FontWeight::BOLD).text_color(TEXT_PRIMARY))
                                    .when(!active, |s| s.text_color(TEXT_FAINT).hover(|h| h.text_color(TEXT_MUTED)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.sort = SearchSort::Default;
                                        cx.notify();
                                    }))
                                    .child("综合排序")
                            })
                            .child(div().text_color(TEXT_FAINT).child("|"))
                            .child({
                                let active = self.sort == SearchSort::Latest;
                                div()
                                    .id("sort_latest")
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .when(active, |s| s.font_weight(FontWeight::BOLD).text_color(TEXT_PRIMARY))
                                    .when(!active, |s| s.text_color(TEXT_FAINT).hover(|h| h.text_color(TEXT_MUTED)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.sort = SearchSort::Latest;
                                        cx.notify();
                                    }))
                                    .child("最新")
                            })
                            .child(div().text_color(TEXT_FAINT).child("|"))
                            .child({
                                let active = self.sort == SearchSort::Hottest;
                                div()
                                    .id("sort_hottest")
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .when(active, |s| s.font_weight(FontWeight::BOLD).text_color(TEXT_PRIMARY))
                                    .when(!active, |s| s.text_color(TEXT_FAINT).hover(|h| h.text_color(TEXT_MUTED)))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.sort = SearchSort::Hottest;
                                        cx.notify();
                                    }))
                                    .child("最热")
                            }),
                    ),
            );

        let body = if self.loading {
            notice_with_icon(IconName::RefreshCw, "正在全网曲库检索中…")
        } else if let Some(error) = &self.error {
            notice_with_icon(IconName::CircleAlert, error.clone())
        } else if !self.searched {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap(px(12.0))
                .child(
                    div()
                        .size(px(64.0))
                        .rounded_full()
                        .bg(hex_rgb(0xf3f4f6))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Search).size(px(28.0)).text_color(TEXT_FAINT)),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(TEXT_MUTED)
                        .child("在顶部搜索框输入歌曲、歌手或专辑，按回车即可全网检索"),
                )
                .into_any_element()
        } else if self.songs.is_empty() {
            notice_with_icon(IconName::Search, "没有找到相关歌曲，换个关键词试试吧")
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("search-results")),
                sizes,
                move |view, range, _window, cx| {
                    let app: &App = cx;
                    let (cur_hash, is_playing) = current_play_state(&view.player, app);
                    let rows = range
                        .clone()
                        .map(|index| {
                            song_row(
                                index,
                                &RowContext {
                                    queue: &view.songs,
                                    rows: &view.rows,
                                    shell: &view.shell,
                                    backend: &view.backend,
                                    selected_index: None,
                                    on_select: None,
                                },
                                cur_hash.as_deref(),
                                is_playing,
                            )
                        })
                        .collect::<Vec<_>>();
                    // 滚到接近末尾就自动续取下一页
                    view.maybe_load_more(range.end, cx);
                    rows
                },
            )
            .track_scroll(&self.scroll_handle)
            .into_any_element()
        };

        v_flex().size_full().child(header).child(table_header()).child(
            div()
                .relative()
                .flex_1()
                .min_h(px(0.0))
                .px_2()
                .py_1()
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
