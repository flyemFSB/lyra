//! 歌单详情与曲目列表视图。

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    Disableable as _, Icon, Sizable as _, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::{Playlist, Song};

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_LIGHT, CARD_BORDER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, badge, friendly_error_message,
    hex_rgb, notice_with_icon, row_sizes, song_cover,
};
use crate::views::player::PlayerModel;
use crate::views::tracks::{
    RowContext, SelectCallback, TrackRow, build_rows, current_play_state, table_header, track_row,
};

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
    /// 是否处于批量选择模式
    batch: bool,
    /// 批量选择的行下标（Arc 让每帧读取零分配）
    checked: Arc<HashSet<usize>>,
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
            batch: false,
            checked: Arc::new(HashSet::new()),
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
        self.batch = false;
        self.checked = Arc::new(HashSet::new());
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
        self.batch = false;
        self.checked = Arc::new(HashSet::new());
        self.scroll_handle = VirtualListScrollHandle::new();
        self.request_seq += 1;
        let seq = self.request_seq;
        cx.notify();

        let app = self.backend.app();
        let global_id = list.global_collection_id.clone();
        self.backend.spawn(async move {
            Msg::Tracks(seq, app.tracks_all(&global_id).await.map_err(|error| error.to_string()))
        });
    }

    /// 处理曲目列表响应（整张歌单一次取回）
    pub(crate) fn on_tracks(&mut self, seq: u64, result: Result<Vec<Song>, String>, cx: &mut Context<Self>) {
        if seq != self.request_seq {
            return;
        }
        self.loading_tracks = false;
        match result {
            Ok(songs) => {
                self.rows = Arc::new(build_rows(&songs));
                self.sizes = row_sizes(songs.len());
                // 只为尚未查询过的歌曲拉取真实最高音质（仅 Hi-Res / SQ 展示徽标）
                crate::views::tracks::request_qualities(&self.player, &self.backend, &songs, cx);
                self.songs = Arc::new(songs);
                self.error = None;
            }
            Err(error) => {
                tracing::warn!(%error, "加载曲目失败");
                self.songs = Arc::new(Vec::new());
                self.rows = Arc::new(Vec::new());
                self.sizes = row_sizes(0);
                self.error = Some(friendly_error_message(&error).into());
            }
        }
        cx.notify();
    }

    /// 刷新当前选中歌单
    pub(crate) fn refresh_current(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.selected {
            self.select(index, cx);
        }
    }

    /// 进入/退出批量选择模式
    fn toggle_batch(&mut self, cx: &mut Context<Self>) {
        self.batch = !self.batch;
        self.checked = Arc::new(HashSet::new());
        cx.notify();
    }

    /// 切换单行勾选状态（批量模式下的单击行为）
    fn toggle_checked(&mut self, index: usize, cx: &mut Context<Self>) {
        let checked = Arc::make_mut(&mut self.checked);
        if !checked.remove(&index) {
            checked.insert(index);
        }
        cx.notify();
    }

    /// 全选当前已加载的全部曲目
    fn check_all(&mut self, cx: &mut Context<Self>) {
        self.checked = Arc::new((0..self.songs.len()).collect());
        cx.notify();
    }

    /// 当前勾选的歌曲（按列表顺序）
    fn checked_songs(&self) -> Vec<Song> {
        let mut indices: Vec<usize> = self.checked.iter().copied().collect();
        indices.sort_unstable();
        indices
            .into_iter()
            .filter_map(|index| self.songs.get(index).cloned())
            .collect()
    }

    /// 从当前歌单批量移除勾选的歌曲
    fn remove_checked(&mut self, _cx: &mut Context<Self>) {
        let Some(listid) = self.listid.clone() else {
            return;
        };
        let mut indices: Vec<usize> = self.checked.iter().copied().collect();
        indices.sort_unstable();
        let fileids: Vec<i64> = indices
            .into_iter()
            .filter_map(|index| self.songs.get(index).map(|song| song.fileid))
            .filter(|fileid| *fileid != 0)
            .collect();
        if fileids.is_empty() {
            self.backend.send(Msg::Toast("所选歌曲缺少可移除的内部 ID".to_owned()));
            return;
        }
        self.backend.send(Msg::RemoveTracks { listid, fileids });
    }

    /// 渲染单首曲目行
    fn row(
        &self,
        index: usize,
        play: (Option<&str>, bool),
        listid: Option<&SharedString>,
        qualities: &std::collections::HashMap<String, &'static str>,
        checked: Option<&HashSet<usize>>,
        on_select: Option<SelectCallback>,
    ) -> AnyElement {
        let (cur_hash, is_playing) = play;
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
                on_select,
                qualities,
                checked,
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
        let meta_count_text = format!("共 {total_count} 首歌曲");

        let songs_for_play = self.songs.clone();
        let shell_for_play = self.shell.clone();
        let has_songs = !self.songs.is_empty();
        let batch = self.batch;
        let checked_count = self.checked.len();
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
                    // 作者与曲目总数：用户头像 + rushzb · 共 241 首歌曲
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
                            .child(div().child(meta_count_text)),
                    )
                    // 操作工具条：播放/批量管理与更多操作
                    .child(
                        div()
                            .relative()
                            .pt_2()
                            .child(
                                h_flex()
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
                                                    .update(cx, |shell, cx| {
                                                        shell.play_queue(songs_for_play.clone(), 0, cx);
                                                    })
                                                    .ok();
                                            }),
                                    )
                                    .when(!batch, |this| {
                                        this.child(
                                            Button::new("playlist_batch")
                                                .ghost()
                                                .icon(IconName::ListChecks)
                                                .label("批量操作")
                                                .tooltip("多选歌曲后批量添加或移除")
                                                .disabled(!has_songs)
                                                .on_click(cx.listener(|view, _event, _window, cx| {
                                                    view.toggle_batch(cx);
                                                })),
                                        )
                                    })
                                    .when(batch, |this| {
                                        this.child(badge(
                                            format!("已选 {checked_count} 首"),
                                            ACCENT_GREEN_LIGHT,
                                            ACCENT_GREEN,
                                        ))
                                        .child(
                                            Button::new("playlist_check_all")
                                                .ghost()
                                                .small()
                                                .icon(IconName::ListChecks)
                                                .label("全选")
                                                .tooltip("勾选当前已加载的全部歌曲")
                                                .on_click(cx.listener(|view, _event, _window, cx| {
                                                    view.check_all(cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("playlist_batch_add")
                                                .secondary()
                                                .small()
                                                .icon(IconName::ListPlus)
                                                .label("添加到歌单")
                                                .disabled(checked_count == 0)
                                                .on_click(cx.listener(|view, _event, _window, cx| {
                                                    let songs = view.checked_songs();
                                                    if songs.is_empty() {
                                                        return;
                                                    }
                                                    view.shell
                                                        .update(cx, |shell, cx| shell.open_add_to(songs, cx))
                                                        .ok();
                                                })),
                                        )
                                        .child(
                                            Button::new("playlist_batch_remove")
                                                .danger()
                                                .small()
                                                .icon(IconName::Trash)
                                                .label("移除所选")
                                                .disabled(checked_count == 0)
                                                .on_click(cx.listener(|view, _event, _window, cx| {
                                                    view.remove_checked(cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("playlist_batch_exit")
                                                .ghost()
                                                .small()
                                                .icon(IconName::X)
                                                .label("退出批量")
                                                .on_click(cx.listener(|view, _event, _window, cx| {
                                                    view.toggle_batch(cx);
                                                })),
                                        )
                                    }),
                            ),
                    )
                    // 批量模式下提示单击勾选
                    .when(batch, |this| {
                        this.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(TEXT_FAINT)
                                .child("单击任意歌曲勾选/取消，双击仍可直接播放"),
                        )
                    }),
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
            // 音质缓存是 Arc：克隆仅递增引用计数，渲染期零分配
            let qualities = self.player.read(cx).qualities.clone();
            let batch = self.batch;
            let checked = self.checked.clone();
            let this_for_select = this.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("playlist-tracks")),
                sizes,
                move |view, range, _window, cx| {
                    let app: &App = cx;
                    let (cur_hash, is_playing) = current_play_state(&view.player, app);
                    // 批量模式下单击只勾选，不切歌
                    let on_select: Option<SelectCallback> = batch.then(|| {
                        let daily = this_for_select.clone();
                        Rc::new(move |index: usize, _window: &mut Window, cx: &mut App| {
                            daily.update(cx, |view, cx| view.toggle_checked(index, cx));
                        }) as SelectCallback
                    });
                    range
                        .clone()
                        .map(|index| {
                            view.row(
                                index,
                                (cur_hash.as_deref(), is_playing),
                                listid.as_ref(),
                                &qualities,
                                batch.then_some(checked.as_ref()),
                                on_select.clone(),
                            )
                        })
                        .collect::<Vec<_>>()
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
