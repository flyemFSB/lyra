//! 个人歌单与歌曲列表视图组件：分栏 One Island 结构与高响应度联动。

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
use crate::views::tracks::track_row;

/// 歌单分类 Tab：自建歌单（包含我喜欢）与收藏歌单。
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlaylistTab {
    #[default]
    Created,
    Collected,
}

/// 我的歌单视图结构。
pub(crate) struct PlaylistsView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    pub(crate) tab: PlaylistTab,
    pub(crate) playlists: Arc<Vec<Playlist>>,
    pub(crate) selected: Option<usize>,
    pub(crate) scroll_handle: VirtualListScrollHandle,
    songs: Arc<Vec<Song>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    pub(crate) loading_playlists: bool,
    loading_tracks: bool,
    loaded: bool,
    error: Option<SharedString>,
    heading: SharedString,
    listid: Option<String>,
    /// 请求序号：只有最新一次选择歌单的响应才允许落地。
    request_seq: u64,
    /// 下一页页码（为 None 表示已到达最后一页）。
    next_page: Option<u32>,
    /// 本次响应是「追加下一页」还是「替换整个列表」。
    appending: bool,
    _sub: Subscription,
}

impl PlaylistsView {
    /// 创建歌单视图实例并订阅播放状态变化。
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
            _sub: sub,
        }
    }

    /// 清空视图内容（登出/换账号时调用），并让在途响应失效。
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.playlists = Arc::new(Vec::new());
        self.selected = None;
        self.tab = PlaylistTab::Created;
        self.scroll_handle = VirtualListScrollHandle::new();
        self.songs = Arc::new(Vec::new());
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

    /// 视图初次加载时触发歌单列表请求。
    pub(crate) fn ensure(&mut self, cx: &mut Context<Self>) {
        if !self.loaded && !self.loading_playlists {
            self.reload(cx);
        }
    }

    /// 强制重新请求歌单列表。
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading_playlists = true;
        self.error = None;
        cx.notify();
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Playlists(app.playlists().await.map_err(|error| error.to_string())) });
    }

    /// 响应处理歌单列表数据。
    pub(crate) fn on_playlists(&mut self, result: Result<Vec<Playlist>, String>, cx: &mut Context<Self>) {
        self.loading_playlists = false;
        self.loaded = true;
        match result {
            Ok(lists) => {
                self.playlists = Arc::new(lists);
                self.error = None;
                if self.selected.is_none() && !self.playlists.is_empty() {
                    let first = self
                        .playlists
                        .iter()
                        .enumerate()
                        .find(|(_, list)| list.is_liked() || list.kind != 1)
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);
                    self.select(first, cx);
                }
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    /// 切换自建歌单与收藏歌单分类。
    pub(crate) fn switch_tab(&mut self, tab: PlaylistTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        // 查找当前 Tab 下的第一个歌单并选中
        let first_in_tab = self
            .playlists
            .iter()
            .enumerate()
            .find(|(_, list)| match self.tab {
                PlaylistTab::Created => list.is_liked() || list.kind != 1,
                PlaylistTab::Collected => !list.is_liked() && list.kind == 1,
            })
            .map(|(idx, _)| idx);

        if let Some(idx) = first_in_tab {
            self.select(idx, cx);
        }
        cx.notify();
    }

    /// 选中指定歌单并请求曲目列表。
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

    /// 响应处理歌曲列表分页数据。
    pub(crate) fn on_tracks(&mut self, seq: u64, result: Result<Page<Song>, String>, cx: &mut Context<Self>) {
        if seq != self.request_seq {
            return;
        }
        let appending = std::mem::take(&mut self.appending);
        self.loading_tracks = false;
        match result {
            Ok(page) => {
                self.next_page = page.cursor.as_deref().and_then(|cursor| cursor.parse().ok());
                let mut songs = if appending { (*self.songs).clone() } else { Vec::new() };
                songs.extend(page.items);
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

    /// 加载下一页歌曲。
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

    /// 增删歌曲后刷新当前选中的歌单。
    pub(crate) fn refresh_current(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.selected {
            self.select(index, cx);
        }
    }

    /// 渲染单行歌曲组件。
    fn row(&self, index: usize, cx: &App) -> AnyElement {
        let removable = self.listid.as_ref().and_then(|listid| {
            self.songs
                .get(index)
                .map(|song| (SharedString::from(listid.as_str()), song.fileid))
        });

        let (is_current, is_playing) = {
            let model = self.player.read(cx);
            let cur_hash = model.current_song().map(|s| &s.hash);
            let this_hash = self.songs.get(index).map(|s| &s.hash);
            (
                cur_hash.is_some() && cur_hash == this_hash,
                model.snapshot.state == lyra_audio::State::Playing,
            )
        };

        track_row(
            index,
            &self.songs,
            &self.shell,
            &self.backend,
            removable,
            is_current,
            is_playing,
            cx,
        )
    }

    /// 渲染右侧歌曲列表区。
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

        let body = if self.loading_tracks {
            notice_with_icon(IconName::RefreshCw, "正在加载曲目…", cx)
        } else if let Some(error) = &self.error {
            notice_with_icon(IconName::CircleAlert, error.clone(), cx)
        } else if self.songs.is_empty() {
            notice_with_icon(IconName::ListMusic, "此歌单暂无曲目", cx)
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("playlist-tracks")),
                sizes,
                move |view, range, _window, cx| {
                    let app: &App = cx;
                    range.map(|index| view.row(index, app)).collect::<Vec<_>>()
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
        div().size_full().child(self.track_area(cx))
    }
}
