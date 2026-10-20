//! 歌单详情与曲目列表视图。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Sizable as _, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list};
use gpui_kit::*;
use lyra_api::model::{Page, Playlist, Song};

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{CARD_BORDER, TEXT_MUTED, cover_box, notice_with_icon, row_sizes, title};
use crate::views::player::PlayerModel;
use crate::views::tracks::{TrackRow, build_rows, track_row};

/// 歌单分类标签页
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlaylistTab {
    #[default]
    Created,
    Collected,
}

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
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    /// 切换自建与收藏标签页
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
                self.next_page = page.cursor.as_deref().and_then(|cursor| cursor.parse().ok());
                // 追加时优先零拷贝接管旧列表；队列仍持有引用时才退化为深拷贝
                let mut songs = match Arc::try_unwrap(self.songs.clone()) {
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
                if appending {
                    self.backend.send(Msg::Toast(format!("加载更多失败: {error}")));
                } else {
                    self.next_page = None;
                    self.error = Some(error.into());
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

    /// 刷新当前选中歌单
    pub(crate) fn refresh_current(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.selected {
            self.select(index, cx);
        }
    }

    /// 渲染单首曲目行
    fn row(
        &self,
        index: usize,
        cur_hash: Option<&str>,
        is_playing: bool,
        listid: Option<&SharedString>,
        cx: &App,
    ) -> AnyElement {
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
            listid,
            is_current,
            is_playing,
            cx,
        )
    }

    /// 渲染曲目列表区域
    fn track_area(&self, cx: &Context<Self>) -> AnyElement {
        let mut header = h_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap(px(14.0))
            .items_center()
            .border_b_1()
            .border_color(CARD_BORDER)
            .child(cover_box(self.heading.as_ref(), 48.0, cx))
            .child(
                v_flex().gap(px(2.0)).child(title(self.heading.clone(), cx)).child(
                    div()
                        .text_xs()
                        .text_color(TEXT_MUTED)
                        .child(SharedString::from(format!("共 {} 首曲目", self.songs.len()))),
                ),
            )
            .child(div().flex_1());

        if !self.songs.is_empty() {
            let songs = self.songs.clone();
            let shell = self.shell.clone();
            header = header.child(
                Button::new("play_all")
                    .primary()
                    .small()
                    .icon(IconName::Play)
                    .label("播放全部")
                    .tooltip("从头播放此歌单")
                    .on_click(move |_event, _window, cx| {
                        shell
                            .update(cx, |shell, cx| shell.play_queue(songs.clone(), 0, cx))
                            .ok();
                    }),
            );
        }

        if self.next_page.is_some() {
            header = header.child(
                Button::new("load_more")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label(if self.appending { "加载中…" } else { "加载更多" })
                    .on_click(cx.listener(|view, _event, _window, cx| view.load_more(cx))),
            );
        }

        // 移除按钮需要的歌单 ID：整个列表共用一份，不在每行重新构造
        let listid = self.listid.as_deref().map(SharedString::from);

        let body = if self.loading_tracks {
            notice_with_icon(IconName::RefreshCw, "正在加载曲目…", cx)
        } else if let Some(error) = &self.error {
            notice_with_icon(IconName::CircleAlert, error.clone(), cx)
        } else if self.songs.is_empty() {
            notice_with_icon(IconName::ListMusic, "此歌单暂无曲目", cx)
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
                    // 播放器状态整批读取一次，避免每行都做一次实体读取
                    let (cur_hash, is_playing) = {
                        let model = view.player.read(app);
                        (
                            model.current_song().map(|s| s.hash.clone()),
                            model.snapshot.state == lyra_audio::State::Playing,
                        )
                    };
                    range
                        .map(|index| view.row(index, cur_hash.as_deref(), is_playing, listid.as_ref(), app))
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.scroll_handle)
            .into_any_element()
        };

        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .p_2()
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
