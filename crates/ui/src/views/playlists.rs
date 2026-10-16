//! 个人歌单与歌曲列表视图组件。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::{Sizable as _, h_flex, v_flex, v_virtual_list};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::{Page, Playlist, Song};

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{notice, row_sizes, title};
use crate::views::tracks::track_row;

/// 我的歌单视图结构。
pub(crate) struct PlaylistsView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    playlists: Arc<Vec<Playlist>>,
    selected: Option<usize>,
    songs: Arc<Vec<Song>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    loading_playlists: bool,
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
}

impl PlaylistsView {
    /// 创建歌单视图实例。
    pub(crate) fn new(backend: Backend, shell: WeakEntity<AppShell>, _cx: &mut Context<Self>) -> Self {
        Self {
            backend,
            shell,
            playlists: Arc::new(Vec::new()),
            selected: None,
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
        }
    }

    /// 清空视图内容（登出/换账号时调用），并让在途响应失效。
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.playlists = Arc::new(Vec::new());
        self.selected = None;
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
        match result {
            Ok(playlists) => {
                self.playlists = Arc::new(playlists);
                self.loaded = true;
                self.error = None;
                if self.selected.is_some_and(|index| index >= self.playlists.len()) {
                    self.selected = None;
                }
                if self.selected.is_none() && !self.playlists.is_empty() {
                    self.select(0, cx);
                }
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    /// 选中指定索引歌单并加载其曲目。
    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(list) = self.playlists.get(index).cloned() else {
            return;
        };
        self.selected = Some(index);
        self.heading = SharedString::from(list.name.as_str());
        self.listid = Some(list.listid);
        self.loading_tracks = true;
        self.songs = Arc::new(Vec::new());
        self.sizes = row_sizes(0);
        self.next_page = None;
        self.appending = false;
        self.request_seq += 1;
        let seq = self.request_seq;
        cx.notify();
        let app = self.backend.app();
        let global_id = list.global_collection_id;
        self.backend
            .spawn(async move { Msg::Tracks(seq, app.tracks(&global_id, 1).await.map_err(|error| error.to_string())) });
    }

    /// 响应处理歌曲分页列表数据（丢弃过期请求序号的异步应答）。
    pub(crate) fn on_tracks(&mut self, seq: u64, result: Result<Page<Song>, String>, cx: &mut Context<Self>) {
        if seq != self.request_seq {
            return;
        }
        // 读取并复位追加标记，区分追加分页与重置列表。
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
                    // 追加失败不该把已经加载出来的列表换成错误页。
                    self.backend.send(Msg::Toast(format!("加载更多失败: {error}")));
                } else {
                    self.next_page = None;
                    self.error = Some(error.into());
                }
            }
        }
        cx.notify();
    }

    /// 追加加载下一页歌曲。
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let (Some(index), Some(page)) = (self.selected, self.next_page) else {
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
        track_row(index, &self.songs, &self.shell, &self.backend, removable, cx)
    }

    /// 渲染左侧歌单目录列表。
    fn playlist_column(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut column = v_flex()
            .id("playlist-column")
            .w(px(220.0))
            .h_full()
            .p_2()
            .gap(px(2.0))
            .overflow_y_scroll();
        if self.loading_playlists && self.playlists.is_empty() {
            column = column.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("加载中…"),
            );
        }
        for (index, list) in self.playlists.iter().enumerate() {
            let active = self.selected == Some(index);
            let label = SharedString::from(format!("{}（{}）", list.name, list.count));
            column = column.child(
                div()
                    .id(("playlist", index))
                    .role(Role::Button)
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .when(active, |this| this.bg(theme.list_active).text_color(theme.foreground))
                    .when(!active, |this| {
                        this.text_color(theme.muted_foreground)
                            .hover(|style| style.bg(theme.list_hover))
                    })
                    .on_click(cx.listener(move |view, _event, _window, cx| view.select(index, cx)))
                    .child(div().text_sm().truncate().child(label)),
            );
        }
        column
    }

    /// 渲染右侧歌曲列表区。
    fn track_area(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let header = h_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap(px(12.0))
            .border_b_1()
            .border_color(theme.border)
            .child(title(self.heading.clone(), cx))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!("{} 首", self.songs.len()))),
            );
        let header = if self.next_page.is_some() {
            header.child(div().flex_1()).child(
                Button::new("load_more")
                    .small()
                    .label(if self.appending { "加载中…" } else { "加载更多" })
                    .on_click(cx.listener(|view, _event, _window, cx| view.load_more(cx))),
            )
        } else {
            header
        };

        let body = if self.loading_tracks {
            notice("加载中…", cx)
        } else if let Some(error) = &self.error {
            notice(error.clone(), cx)
        } else if self.songs.is_empty() {
            notice("这里还没有曲目", cx)
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
            .into_any_element()
        };

        v_flex()
            .size_full()
            .child(header)
            .child(div().flex_1().min_h(px(0.0)).child(body))
            .into_any_element()
    }
}

impl Render for PlaylistsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let column = self.playlist_column(cx);
        let track_area = self.track_area(cx);
        let border = cx.theme().border;
        div()
            .flex()
            .flex_row()
            .size_full()
            .child(column)
            .child(div().w(px(1.0)).h_full().bg(border))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .child(track_area),
            )
    }
}
