//! 每日推荐视图，包含问候卡片、分类专辑与推荐曲目列表。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, Sizable as _, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list};
use gpui_kit::*;
use lyra_api::model::Song;

use crate::bridge::{Backend, Msg};
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CARD_BG, TEXT_MUTED, TEXT_PRIMARY, badge,
    friendly_error_message, hex_rgb, notice_with_icon, row_sizes, title,
};
use crate::views::player::PlayerModel;
use crate::views::tracks::{RowContext, TrackRow, build_rows, current_play_state, song_row};

/// 每日推荐视图
pub(crate) struct DailyView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    songs: Arc<Vec<Song>>,
    /// 预格式化的曲目展示模型
    rows: Arc<Vec<TrackRow>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll_handle: VirtualListScrollHandle,
    focus_handle: FocusHandle,
    /// 选中的曲目索引（键盘导航/单击选中）
    pub(crate) selected_index: Option<usize>,
    /// 上一次观察到的播放曲目哈希（用于清除过期选中态）
    last_current: Option<String>,
    loading: bool,
    loaded: bool,
    error: Option<SharedString>,
    _sub: Subscription,
}

impl DailyView {
    /// 创建每日推荐视图并订阅播放器状态
    pub(crate) fn new(
        backend: Backend,
        shell: WeakEntity<AppShell>,
        player: Entity<PlayerModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.observe(&player, |this, player, cx| {
            // 切歌后旧的灰色选中态必须清掉，否则上一首会同时留着灰底，
            // 让人误以为两首同时在播。
            let current = player.read(cx).current_hash().map(str::to_owned);
            if current != this.last_current {
                this.last_current = current;
                this.selected_index = None;
            }
            cx.notify();
        });
        Self {
            backend,
            shell,
            player,
            songs: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            sizes: row_sizes(0),
            scroll_handle: VirtualListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            selected_index: None,
            last_current: None,
            loading: false,
            loaded: false,
            error: None,
            _sub: sub,
        }
    }

    /// 重置视图状态
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.songs = Arc::new(Vec::new());
        self.rows = Arc::new(Vec::new());
        self.sizes = row_sizes(0);
        self.scroll_handle = VirtualListScrollHandle::new();
        self.selected_index = None;
        self.last_current = None;
        self.loading = false;
        self.loaded = false;
        self.error = None;
        cx.notify();
    }

    /// 确保推荐数据已加载
    pub(crate) fn ensure(&mut self, cx: &mut Context<Self>) {
        if !self.loaded && !self.loading {
            self.reload(cx);
        }
    }

    /// 重新拉取每日推荐歌曲
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        // 换一批后列表内容完全不同：滚动位置必须归零，
        // 否则会停在新一批歌曲的中间。
        self.scroll_handle = VirtualListScrollHandle::new();
        cx.notify();
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Daily(app.daily().await.map_err(|error| error.to_string())) });
    }

    /// 处理每日推荐响应结果
    pub(crate) fn on_daily(&mut self, result: Result<Vec<Song>, String>, cx: &mut Context<Self>) {
        self.loading = false;
        match result {
            Ok(songs) => {
                self.rows = Arc::new(build_rows(&songs));
                self.songs = Arc::new(songs);
                self.sizes = row_sizes(self.songs.len());
                self.loaded = true;
                self.error = None;
                let songs_for_quality = (*self.songs).clone();
                crate::views::tracks::request_qualities(&self.player, &self.backend, &songs_for_quality, cx);
            }
            Err(error) => {
                tracing::warn!(%error, "获取每日推荐失败");
                self.error = Some(friendly_error_message(&error).into());
            }
        }
        cx.notify();
    }
}

impl Render for DailyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell_fm = self.shell.clone();
        let shell_daily = self.shell.clone();
        let songs_daily = self.songs.clone();

        // 卡片 1：红心漫游（私人 FM 随心畅听）
        let card_radio = v_flex()
            .id("personal_fm_card")
            .role(Role::Button)
            .cursor_pointer()
            .flex_1()
            .min_w(px(130.0))
            .p_3()
            .rounded(px(14.0))
            .bg(hex_rgb(0xfff1f2))
            .border_1()
            .border_color(hex_rgb(0xfecdd3))
            .shadow_xs()
            .hover(|s| s.bg(hex_rgb(0xfee2e2)).shadow_sm())
            .on_click(move |_event, _window, cx| {
                shell_fm.update(cx, AppShell::start_personal_fm).ok();
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_start()
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .rounded_full()
                            .bg(hex_rgb(0xffe4e6))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Heart).size(px(22.0)).text_color(hex_rgb(0xef4444))),
                    )
                    .child(
                        div()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded_full()
                            .bg(ACCENT_GREEN)
                            .hover(|s| s.bg(ACCENT_GREEN_HOVER))
                            .shadow_xs()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Play).size(px(13.0)).text_color(CARD_BG)),
                    ),
            )
            .child(
                v_flex()
                    .mt_3()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(TEXT_PRIMARY)
                            .child("红心漫游"),
                    )
                    .child(div().text_xs().text_color(TEXT_MUTED).child("私人电台 · 随心畅听")),
            );

        // 卡片 2：每日 30 首（今日专属定制）
        let card_daily = v_flex()
            .id("daily_30_card")
            .role(Role::Button)
            .cursor_pointer()
            .flex_1()
            .min_w(px(130.0))
            .p_3()
            .rounded(px(14.0))
            .bg(hex_rgb(0xecfdf5))
            .border_1()
            .border_color(hex_rgb(0xa7f3d0))
            .shadow_xs()
            .hover(|s| s.bg(hex_rgb(0xd1fae5)).shadow_sm())
            .on_click(move |_event, _window, cx| {
                if !songs_daily.is_empty() {
                    shell_daily
                        .update(cx, |shell, cx| shell.play_queue(songs_daily.clone(), 0, cx))
                        .ok();
                }
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_start()
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .rounded_full()
                            .bg(hex_rgb(0xd1fae5))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Sparkles).size(px(22.0)).text_color(ACCENT_GREEN)),
                    )
                    .child(
                        div()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded_full()
                            .bg(ACCENT_GREEN)
                            .hover(|s| s.bg(ACCENT_GREEN_HOVER))
                            .shadow_xs()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Play).size(px(13.0)).text_color(CARD_BG)),
                    ),
            )
            .child(
                v_flex()
                    .mt_3()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(TEXT_PRIMARY)
                            .child("每日 30 首"),
                    )
                    .child(div().text_xs().text_color(TEXT_MUTED).child("专属精选 · 每日更新")),
            );

        // 顶层只保留两个入口：红心漫游（私人电台）与今日推荐（每日 30 首）
        let top_section = v_flex()
            .w_full()
            .px_4()
            .pt_4()
            .pb_1()
            .gap(px(10.0))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(TEXT_MUTED)
                    .child("私人漫游与今日推荐"),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(px(12.0))
                    .items_center()
                    .child(card_radio)
                    .child(card_daily),
            );

        // 歌曲列表标题栏
        let mut list_header = h_flex()
            .w_full()
            .px_4()
            .pt_3()
            .pb_2()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(title("今日推荐"))
                    .child(badge(
                        format!("{} 首", self.songs.len()),
                        ACCENT_GREEN_LIGHT,
                        ACCENT_GREEN_HOVER,
                    )),
            );

        if !self.songs.is_empty() {
            let songs = self.songs.clone();
            let shell = self.shell.clone();
            list_header = list_header.child(
                Button::new("play_all")
                    .secondary()
                    .small()
                    .icon(IconName::Play)
                    .label("全部播放")
                    .tooltip("播放今日全部推荐")
                    .on_click(move |_event, _window, cx| {
                        shell
                            .update(cx, |shell, cx| shell.play_queue(songs.clone(), 0, cx))
                            .ok();
                    }),
            );
        }

        let body = if self.loading {
            notice_with_icon(IconName::RefreshCw, "正在加载每日推荐…")
        } else if let Some(error) = &self.error {
            notice_with_icon(IconName::CircleAlert, error.clone())
        } else if self.songs.is_empty() {
            notice_with_icon(IconName::Sparkles, "今日推荐已阅毕")
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            let this_for_select = this.clone();
            let qualities = self.player.read(cx).qualities.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("daily-tracks")),
                sizes,
                move |view, range, _window, cx| {
                    let app: &App = cx;
                    // 播放器状态整批读取一次，避免每行都做一次实体读取
                    let (cur_hash, is_playing) = current_play_state(&view.player, app);
                    let sel_entity = this_for_select.clone();
                    let on_select = std::rc::Rc::new(move |selected_idx: usize, _window: &mut Window, cx: &mut App| {
                        sel_entity.update(cx, |daily, cx| {
                            daily.selected_index = Some(selected_idx);
                            cx.notify();
                        });
                    });
                    range
                        .map(|index| {
                            song_row(
                                index,
                                &RowContext {
                                    queue: &view.songs,
                                    rows: &view.rows,
                                    shell: &view.shell,
                                    backend: &view.backend,
                                    selected_index: view.selected_index,
                                    on_select: Some(on_select.clone()),
                                    qualities: &qualities,
                                    checked: None,
                                },
                                cur_hash.as_deref(),
                                is_playing,
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
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(
                |this, event: &KeyDownEvent, _window, cx| match event.keystroke.key.as_str() {
                    "down" => {
                        let total = this.songs.len();
                        if total > 0 {
                            this.selected_index = match this.selected_index {
                                Some(i) if i + 1 < total => Some(i + 1),
                                None => Some(0),
                                other => other,
                            };
                            cx.notify();
                        }
                    }
                    "up" => {
                        this.selected_index = match this.selected_index {
                            Some(i) if i > 0 => Some(i - 1),
                            _ => Some(0),
                        };
                        cx.notify();
                    }
                    "space" => {
                        if let Some(engine) = this.backend.engine() {
                            engine.toggle();
                        }
                    }
                    "enter" => {
                        if let Some(idx) = this.selected_index
                            && idx < this.songs.len()
                        {
                            let songs = this.songs.clone();
                            this.shell.update(cx, |shell, cx| shell.play_queue(songs, idx, cx)).ok();
                        }
                    }
                    _ => {}
                },
            ))
            .child(top_section)
            .child(list_header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .px_2()
                    .vertical_scrollbar(&self.scroll_handle)
                    .child(body),
            )
    }
}
