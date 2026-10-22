//! 歌单详情与曲目列表视图。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    Disableable as _, Icon, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::*;
use lyra_api::model::{Page, Playlist, Song};

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_LIGHT, CARD_BORDER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, badge, friendly_error_message,
    hex_rgb, notice_with_icon, row_sizes, song_cover,
};
use crate::views::player::PlayerModel;
use crate::views::tracks::{RowContext, TrackRow, build_rows, current_play_state, table_header, track_row};

/// 歌单分类标签页
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlaylistTab {
    #[default]
    Created,
    Collected,
}

/// 提前预取行数：滚动到距末尾不足这么多行时自动拉下一页
const PREFETCH_ROWS: usize = 20;

/// 侧边栏歌单条目的展示模型（预格式化，渲染期零分配）
pub(crate) struct PlaylistEntry {
    /// 在原始列表中的下标
    pub(crate) index: usize,
    /// 歌单名称
    pub(crate) name: SharedString,
    /// 曲目数量文本
    pub(crate) count: SharedString,
    /// 是否为「我喜欢」
    pub(crate) liked: bool,
}

/// 歌单详情视图
pub(crate) struct PlaylistsView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    pub(crate) tab: PlaylistTab,
    pub(crate) playlists: Arc<Vec<Playlist>>,
    pub(crate) selected: Option<usize>,
    pub(crate) scroll_handle: VirtualListScrollHandle,
    songs: Arc<Vec<Song>>,
    /// 预格式化的曲目展示模型
    rows: Arc<Vec<TrackRow>>,
    /// 侧边栏可见歌单条目（预格式化，随标签页过滤）
    pub(crate) entries: Arc<Vec<PlaylistEntry>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    pub(crate) loading_playlists: bool,
    loading_tracks: bool,
    loaded: bool,
    error: Option<SharedString>,
    heading: SharedString,
    listid: Option<String>,
    /// 请求序号（用于丢弃过期响应）
    request_seq: u64,
    /// 下一页页码（None 表示已加载完毕）
    next_page: Option<u32>,
    /// 是否为追加分页数据
    appending: bool,
    /// 标签页切换序号（驱动过渡动效）
    pub(crate) tab_seq: u64,
    _sub: Subscription,
}

impl PlaylistsView {
    /// 创建歌单视图并订阅播放器状态
    pub(crate) fn new(
        backend: Backend,
        shell: WeakEntity<AppShell>,
        player: Entity<PlayerModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.observe(&player, |_this, _player, cx| {
            cx.notify();
        });
        Self {
            backend,
            shell,
            player,
            tab: PlaylistTab::Created,
            playlists: Arc::new(Vec::new()),
            selected: None,
            scroll_handle: VirtualListScrollHandle::new(),
            songs: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            entries: Arc::new(Vec::new()),
            sizes: row_sizes(0),
            loading_playlists: false,
            loading_tracks: false,
            loaded: false,
            error: None,
            heading: SharedString::new_static("歌单"),
            listid: None,
            request_seq: 0,
            next_page: None,
            appending: false,
            tab_seq: 1,
            _sub: sub,
        }
    }

    /// 重建侧边栏可见歌单条目（预格式化，渲染期零分配）
    fn rebuild_entries(&mut self) {
        let tab = self.tab;
        self.entries = Arc::new(
            self.playlists
                .iter()
                .enumerate()
                .filter(|(_, list)| match tab {
                    PlaylistTab::Created => list.is_liked() || list.kind != 1,
                    PlaylistTab::Collected => !list.is_liked() && list.kind == 1,
                })
                .map(|(index, list)| PlaylistEntry {
                    index,
                    name: SharedString::from(list.name.as_str()),
                    count: SharedString::from(list.count.to_string()),
                    liked: list.is_liked(),
                })
                .collect(),
        );
    }

    /// 重置视图状态并废弃在途请求
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.playlists = Arc::new(Vec::new());
        self.selected = None;
        self.tab = PlaylistTab::Created;
        self.tab_seq = 1;
        self.scroll_handle = VirtualListScrollHandle::new();
        self.songs = Arc::new(Vec::new());
        self.rows = Arc::new(Vec::new());
        self.entries = Arc::new(Vec::new());
        self.sizes = row_sizes(0);
        self.loading_playlists = false;
        self.loading_tracks = false;
        self.loaded = false;
        self.error = None;
        self.heading = SharedString::new_static("歌单");
        self.listid = None;
        self.next_page = None;
        self.appending = false;
        self.request_seq += 1;
        cx.notify();
    }

    /// 确保歌单数据已加载
    pub(crate) fn ensure(&mut self, cx: &mut Context<Self>) {
        if !self.loaded && !self.loading_playlists {
            self.reload(cx);
        }
    }

    /// 重新拉取歌单列表
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading_playlists = true;
        self.error = None;
        cx.notify();
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Playlists(app.playlists().await.map_err(|error| error.to_string())) });
    }

    /// 处理歌单列表响应结果
    pub(crate) fn on_playlists(&mut self, result: Result<Vec<Playlist>, String>, cx: &mut Context<Self>) {
        self.loading_playlists = false;
        self.loaded = true;
        match result {
            Ok(lists) => {
                self.playlists = Arc::new(lists);
                self.error = None;
                self.rebuild_entries();
                if self.selected.is_none() && !self.playlists.is_empty() {
                    let first = self
                        .playlists
                        .iter()
                        .enumerate()
                        .find(|(_, list)| list.is_liked() || list.kind != 1)
                        .map_or(0, |(idx, _)| idx);
                    self.select(first, cx);
                }
            }
            Err(error) => {
                tracing::warn!(%error, "加载歌单失败");
                self.error = Some(friendly_error_message(&error).into());
            }
        }
        cx.notify();
    }

    /// 切换自建与收藏标签页
    #[allow(dead_code)]
    pub(crate) fn switch_tab(&mut self, tab: PlaylistTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.tab_seq += 1;
        self.rebuild_entries();
        let first_in_tab = self.entries.first().map(|entry| entry.index);
        if let Some(idx) = first_in_tab {
            self.select(idx, cx);
        }
        cx.notify();
    }

    /// 选中指定歌单并拉取曲目
    pub(crate) fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(list) = self.playlists.get(index) else {
            return;
        };
        self.selected = Some(index);
        self.heading = SharedString::from(list.name.as_str());
        self.listid = Some(list.listid.clone());
        self.loading_tracks = true;
        self.error = None;
        self.appending = false;
        self.scroll_handle = VirtualListScrollHandle::new();
        self.request_seq += 1;
        let seq = self.request_seq;
        cx.notify();

        let app = self.backend.app();
        let global_id = list.global_collection_id.clone();
        self.backend
            .spawn(async move { Msg::Tracks(seq, app.tracks(&global_id, 1).await.map_err(|error| error.to_string())) });
    }

    /// 处理曲目列表分页响应
    pub(crate) fn on_tracks(&mut self, seq: u64, result: Result<Page<Song>, String>, cx: &mut Context<Self>) {
        if seq != self.request_seq {
            return;
        }
        let appending = std::mem::take(&mut self.appending);
        self.loading_tracks = false;
        match result {
            Ok(page) => {
                // 空页不能留下游标，否则滚动预取会无限空转
                self.next_page = if page.items.is_empty() {
                    None
                } else {
                    page.cursor.as_deref().and_then(|cursor| cursor.parse().ok())
                };
                // 追加时优先零拷贝接管旧列表；队列仍持有引用时才退化为深拷贝
                // 注意：必须先把自己这份 Arc 换走，否则引用计数永远大于 1，
                // try_unwrap 必然失败，退化成每次翻页都深拷贝整张歌单。
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
                tracing::warn!(%error, "加载曲目失败");
                if appending {
                    self.backend.send(Msg::Toast(friendly_error_message(&error)));
                } else {
                    self.next_page = None;
                    self.error = Some(friendly_error_message(&error).into());
                }
            }
        }
        cx.notify();
    }

    /// 加载下一页曲目
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading_tracks || self.appending {
            return;
        }
        let Some(page) = self.next_page else {
            return;
        };
        let Some(index) = self.selected else {
            return;
        };
        let Some(list) = self.playlists.get(index) else {
            return;
        };
        self.appending = true;
        self.request_seq += 1;
        let seq = self.request_seq;
        cx.notify();
        let app = self.backend.app();
        let global_id = list.global_collection_id.clone();
        self.backend.spawn(async move {
            Msg::Tracks(
                seq,
                app.tracks(&global_id, page).await.map_err(|error| error.to_string()),
            )
        });
    }

    /// 虚拟列表滚到接近底部时自动续取下一页（不限量滚动加载）
    fn maybe_load_more(&mut self, visible_end: usize, cx: &mut Context<Self>) {
        if self.next_page.is_some() && visible_end + PREFETCH_ROWS >= self.songs.len() {
            self.load_more(cx);
        }
    }

    /// 刷新当前选中歌单
    pub(crate) fn refresh_current(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.selected {
            self.select(index, cx);
        }
    }

    /// 渲染单首曲目行
    fn row(&self, index: usize, cur_hash: Option<&str>, is_playing: bool, listid: Option<&SharedString>) -> AnyElement {
        let this_hash = self.songs.get(index).map(|s| s.hash.as_str());
        let is_current = cur_hash.is_some() && cur_hash == this_hash;
        let Some(row) = self.rows.get(index) else {
            return div().into_any_element();
        };
        track_row(
            index,
            row,
            &RowContext {
                queue: &self.songs,
                rows: &self.rows,
                shell: &self.shell,
                backend: &self.backend,
                selected_index: None,
                on_select: None,
            },
            listid,
            is_current,
            is_playing,
        )
    }

    /// 渲染曲目列表区域（桌面端仪式感 Header 视觉重塑与表格多列视图）
    fn track_area(&self, cx: &Context<Self>) -> AnyElement {
        let is_liked = self
            .selected
            .and_then(|idx| self.playlists.get(idx))
            .is_some_and(Playlist::is_liked)
            || self.heading == "我喜欢";

        let total_count = self
            .selected
            .and_then(|idx| self.playlists.get(idx))
            .map_or(self.songs.len(), |p| p.count as usize);

        let username = self
            .backend
            .app()
            .store()
            .account()
            .ok()
            .flatten()
            .map(|a| a.nickname)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "rushzb".to_owned());

        // 1. 封面区：尺寸放大至 160×160px（圆角 12px），“我喜欢”使用专门定制的渐变色红心封面与柔光漫反射投影
        let cover_element = if is_liked {
            div()
                .size(px(160.0))
                .flex_shrink_0()
                .rounded(px(12.0))
                .bg(linear_gradient(
                    135.0,
                    linear_color_stop(hex_rgb(0xff4b72), 0.0),
                    linear_color_stop(hex_rgb(0xff758c), 1.0),
                ))
                .shadow_xl()
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Heart).size(px(64.0)).text_color(rgb(0xffffff)))
                .into_any_element()
        } else {
            let first_cover = self.songs.first().map(|s| s.cover_at(400)).unwrap_or_default();
            div()
                .size(px(160.0))
                .flex_shrink_0()
                .rounded(px(12.0))
                .shadow_lg()
                .overflow_hidden()
                .child(song_cover(&first_cover, self.heading.as_ref(), 160.0))
                .into_any_element()
        };

        // 2. 元信息区与操作工具条（垂直排列）
        let loaded_count = self.songs.len();
        let meta_count_text = if loaded_count < total_count && loaded_count > 0 {
            format!("共 {total_count} 首歌曲（已加载 {loaded_count} 首）")
        } else {
            format!("共 {total_count} 首歌曲")
        };

        let songs_for_play = self.songs.clone();
        let shell_for_play = self.shell.clone();
        let has_songs = !self.songs.is_empty();

        let header = h_flex()
            .w_full()
            .p_6()
            .gap(px(24.0))
            .items_end()
            .border_b_1()
            .border_color(CARD_BORDER)
            .child(cover_element)
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(8.0))
                    // 小字标签：歌单
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(badge("歌单", ACCENT_GREEN_LIGHT, ACCENT_GREEN)),
                    )
                    // 超大标题：28px Semi-Bold
                    .child(
                        div()
                            .text_size(px(28.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(TEXT_PRIMARY)
                            .truncate()
                            .child(self.heading.clone()),
                    )
                    // 作者与创建信息：用户头像 + rushzb · 2026年创建 · 共 251 首歌曲
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .text_xs()
                            .text_color(TEXT_MUTED)
                            .child(
                                div()
                                    .size(px(22.0))
                                    .rounded_full()
                                    .bg(ACCENT_GREEN_LIGHT)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::User).size(px(12.0)).text_color(ACCENT_GREEN)),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(TEXT_PRIMARY)
                                    .child(username),
                            )
                            .child(div().text_color(TEXT_FAINT).child("·"))
                            .child(div().child("2026年创建"))
                            .child(div().text_color(TEXT_FAINT).child("·"))
                            .child(div().child(meta_count_text)),
                    )
                    // 操作工具条：[ ▷ 播放全部 (主色胶囊) ]、[ 随机播放 ]、[ 批量操作 ]、[ ··· 更多 ]
                    .child(
                        h_flex()
                            .pt_2()
                            .gap(px(10.0))
                            .items_center()
                            .child(
                                Button::new("playlist_play_all")
                                    .primary()
                                    .icon(IconName::Play)
                                    .label("播放全部")
                                    .tooltip("从第一首开始播放此歌单")
                                    .disabled(!has_songs)
                                    .on_click(move |_event, _window, cx| {
                                        shell_for_play
                                            .update(cx, |shell, cx| shell.play_queue(songs_for_play.clone(), 0, cx))
                                            .ok();
                                    }),
                            )
                            .child(
                                Button::new("playlist_batch")
                                    .ghost()
                                    .icon(IconName::ListMusic)
                                    .label("批量操作")
                                    .tooltip("批量管理歌曲")
                                    .on_click(cx.listener(|_view, _event, _window, cx| {
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("playlist_more")
                                    .ghost()
                                    .icon(IconName::Ellipsis)
                                    .tooltip("更多歌单操作")
                                    .on_click(cx.listener(|_view, _event, _window, cx| {
                                        cx.notify();
                                    })),
                            ),
                    ),
            );

        // 移除按钮需要的歌单 ID：整个列表共用一份，不在每行重新构造
        let listid = self.listid.as_deref().map(SharedString::from);

        let body = if self.loading_tracks && self.songs.is_empty() {
            notice_with_icon(IconName::RefreshCw, "正在加载曲目…")
        } else if let Some(error) = &self.error {
            notice_with_icon(IconName::CircleAlert, error.clone())
        } else if self.songs.is_empty() {
            notice_with_icon(IconName::ListMusic, "此歌单暂无曲目")
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            let listid = listid.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("playlist-tracks")),
                sizes,
                move |view, range, _window, cx| {
                    let app: &App = cx;
                    let (cur_hash, is_playing) = current_play_state(&view.player, app);
                    let rows = range
                        .clone()
                        .map(|index| view.row(index, cur_hash.as_deref(), is_playing, listid.as_ref()))
                        .collect::<Vec<_>>();
                    // 滚到接近末尾就自动续取下一页，直到整张歌单全部可用
                    view.maybe_load_more(range.end, cx);
                    rows
                },
            )
            .track_scroll(&self.scroll_handle)
            .into_any_element()
        };

        v_flex()
            .size_full()
            .child(header)
            .child(table_header())
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .px_2()
                    .py_1()
                    .vertical_scrollbar(&self.scroll_handle)
                    .child(body),
            )
            .into_any_element()
    }
}

impl Render for PlaylistsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.track_area(cx)
    }
}
