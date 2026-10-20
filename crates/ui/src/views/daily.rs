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
    ACCENT_GREEN, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CARD_BORDER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, badge,
    hex_rgb, notice_with_icon, row_sizes, title,
};
use crate::views::player::PlayerModel;
use crate::views::tracks::{TrackRow, build_rows, track_row};

/// 获取当前时段问候语（UTC+8）
fn get_greeting() -> &'static str {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let hour = ((secs + 8 * 3600) % 86400) / 3600;
    if (5..12).contains(&hour) {
        "早上好"
    } else if (12..14).contains(&hour) {
        "中午好"
    } else if (14..18).contains(&hour) {
        "下午好"
    } else {
        "晚上好"
    }
}

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
        let sub = cx.observe(&player, |_this, _player, cx| {
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
            }
            Err(error) => self.error = Some(error.into()),
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

impl Render for DailyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let greeting = get_greeting();
        // 展示文本在数据层已经预格式化，渲染期不再克隆整首曲目
        let (first_song_name, first_song_singer) = match self.songs.first() {
            Some(song) => (
                SharedString::from(song.display_title()),
                SharedString::from(song.display_singer()),
            ),
            None => (
                SharedString::new_static("暂无推荐曲目"),
                SharedString::new_static("猜你喜欢"),
            ),
        };

        let songs_for_play = self.songs.clone();
        let shell_play = self.shell.clone();

        // 标题栏
        let recommend_header = h_flex()
            .w_full()
            .px_4()
            .pt_4()
            .pb_2()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::BOLD)
                    .text_color(TEXT_PRIMARY)
                    .child("今日为你推荐"),
            )
            .child(
                Button::new("refresh")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label("换一批")
                    .tooltip("重新生成推荐")
                    .on_click(cx.listener(|view, _event, _window, cx| view.reload(cx))),
            );

        // 问候大卡片
        let greeting_card = v_flex()
            .w(px(220.0))
            .gap(px(6.0))
            .child(
                div()
                    .id("greeting_banner")
                    .w(px(220.0))
                    .h(px(135.0))
                    .rounded(px(14.0))
                    .bg(hex_rgb(0xdce7fd))
                    .border_1()
                    .border_color(hex_rgb(0xc7d8f8))
                    .p_4()
                    .relative()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .child(
                        div()
                            .absolute()
                            .top(px(14.0))
                            .right(px(-18.0))
                            .w(px(84.0))
                            .h(px(84.0))
                            .rounded_full()
                            .bg(hex_rgb(0x1e293b))
                            .shadow_md()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .rounded_full()
                                    .bg(hex_rgb(0x2563eb))
                                    .border_2()
                                    .border_color(hex_rgb(0x60a5fa)),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(hex_rgb(0x0f172a))
                                    .child(greeting),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(hex_rgb(0x475569))
                                    .child("尝试来点儿音乐\n提提神吧~"),
                            ),
                    )
                    .child(
                        div()
                            .id("greeting_play_btn")
                            .role(Role::Button)
                            .cursor_pointer()
                            .w(px(32.0))
                            .h(px(32.0))
                            .rounded_full()
                            .bg(ACCENT_GREEN)
                            .hover(|s| s.bg(ACCENT_GREEN_HOVER))
                            .shadow_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_click(move |_event, _window, cx| {
                                if !songs_for_play.is_empty() {
                                    shell_play
                                        .update(cx, |shell, cx| {
                                            shell.play_queue(songs_for_play.clone(), 0, cx);
                                        })
                                        .ok();
                                }
                            })
                            .child(Icon::new(IconName::Play).size(px(16.0)).text_color(hex_rgb(0xffffff))),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(TEXT_PRIMARY)
                            .truncate()
                            .child(first_song_name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(TEXT_MUTED)
                            .truncate()
                            .child(first_song_singer),
                    ),
            );

        // 主题推荐卡片
        let card_styles = [
            ("精选", hex_rgb(0xfde2e4), hex_rgb(0x3b82f6)),
            ("热歌", hex_rgb(0xd1fae5), ACCENT_GREEN),
            ("新歌", hex_rgb(0xfee2e2), hex_rgb(0xef4444)),
            ("雷达", hex_rgb(0xfef3c7), hex_rgb(0xf59e0b)),
        ];

        let mut cards_row = h_flex().w_full().px_4().gap(px(14.0)).items_start();
        cards_row = cards_row.child(greeting_card);

        for (idx, &(badge_text, bg_col, badge_col)) in card_styles.iter().enumerate() {
            let song_idx = idx + 1;
            let Some(song) = self.songs.get(song_idx) else {
                continue;
            };
            // 直接借用展示文本，不构造中间 String
            let song_title = SharedString::from(song.display_title());
            let song_singer = SharedString::from(song.display_singer());
            let shell = self.shell.clone();
            let songs = self.songs.clone();

            let card = v_flex()
                .id(("themed_card", idx))
                .role(Role::Button)
                .cursor_pointer()
                .w(px(120.0))
                .gap(px(6.0))
                .on_click(move |_event, _window, cx| {
                    if !songs.is_empty() {
                        shell
                            .update(cx, |shell, cx| shell.play_queue(songs.clone(), song_idx, cx))
                            .ok();
                    }
                })
                .child(
                    div()
                        .w(px(120.0))
                        .h(px(120.0))
                        .rounded(px(12.0))
                        .bg(bg_col)
                        .border_1()
                        .border_color(CARD_BORDER)
                        .relative()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Music).size(px(32.0)).text_color(badge_col))
                        .child(div().absolute().bottom(px(6.0)).left(px(6.0)).child(badge(
                            badge_text,
                            badge_col,
                            hex_rgb(0xffffff),
                        ))),
                )
                .child(
                    v_flex()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT_PRIMARY)
                                .truncate()
                                .child(song_title),
                        )
                        .child(div().text_xs().text_color(TEXT_FAINT).truncate().child(song_singer)),
                );
            cards_row = cards_row.child(card);
        }

        // 歌曲列表标题栏
        let mut list_header = h_flex()
            .w_full()
            .px_4()
            .pt_4()
            .pb_2()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(title("推荐歌曲", cx))
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
                    .primary()
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
            notice_with_icon(IconName::RefreshCw, "正在加载每日推荐…", cx)
        } else if let Some(error) = &self.error {
            notice_with_icon(IconName::CircleAlert, error.clone(), cx)
        } else if self.songs.is_empty() {
            notice_with_icon(IconName::Sparkles, "今日推荐已阅毕", cx)
        } else {
            let this = cx.entity();
            let sizes = self.sizes.clone();
            v_virtual_list(
                this,
                ElementId::Name(SharedString::new_static("daily-tracks")),
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

        v_flex()
            .size_full()
            .child(recommend_header)
            .child(cards_row)
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
