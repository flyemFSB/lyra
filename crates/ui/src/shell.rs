//! 应用主外壳：导航、内容承载、播放模式调度与播放队列抽屉

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::{Playlist, Song};

use crate::bridge::{Backend, Msg, Nav};
use crate::format::format_duration;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_BORDER, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CANVAS_BG, CARD_BG, CARD_BORDER,
    SURFACE_HOVER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, badge, cover_box, hex_rgb, main_card, notice,
    notice_with_icon,
};
use crate::views::daily::DailyView;
use crate::views::login::LoginView;
use crate::views::lyrics::LyricsView;
use crate::views::player::{PlayModeUi, PlayerBar, PlayerModel};
use crate::views::playlists::{PlaylistTab, PlaylistsView};
use crate::views::radio::RadioView;
use crate::views::search::SearchView;
use crate::views::settings::SettingsView;

/// 检查连续自动跳过次数是否未超限
fn should_skip_unplayable(skips: usize) -> bool {
    skips < MAX_AUTO_SKIPS
}

/// 轻提示默认展示时长
const TOAST_TTL: Duration = Duration::from_secs(4);

/// 自动续播允许连续跳过的曲目上限（避免死循环）
const MAX_AUTO_SKIPS: usize = 5;

/// 缓存的子视图实体集合
struct Views {
    player_bar: Entity<PlayerBar>,
    playlists: Entity<PlaylistsView>,
    daily: Entity<DailyView>,
    radio: Entity<RadioView>,
    search: Entity<SearchView>,
    settings: Entity<SettingsView>,
    login: Entity<LoginView>,
    lyrics: Entity<LyricsView>,
}

/// 应用主外壳状态
pub(crate) struct AppShell {
    backend: Backend,
    pub(crate) nav: Nav,
    history: Vec<Nav>,
    history_idx: usize,
    logged_in: bool,
    player: Entity<PlayerModel>,
    views: Option<Views>,
    toast: Option<SharedString>,
    toast_deadline: Option<Instant>,
    /// 是否展开右侧播放队列抽屉
    show_queue: bool,
    /// 歌词大屏正在收起退出过渡
    closing_lyrics: bool,
    /// 歌词收起动效序号
    closing_lyrics_seq: u64,
    _lyrics_close_task: Option<Task<()>>,
    /// 导航切换序号
    pub(crate) nav_seq: u64,
    /// 轻提示变更序号（驱动浮动提示下落动效）
    toast_seq: u64,
    /// 播放队列抽屉切换序号（驱动抽屉滑入动效）
    queue_seq: u64,
    /// 歌词请求序列号（用于丢弃过期响应）
    lyrics_seq: u64,
    /// 连续自动跳过的曲目计数
    auto_skips: usize,
    _drain: Task<()>,
}

impl AppShell {
    /// 创建主外壳状态
    pub(crate) fn new(backend: Backend, receiver: Receiver<Msg>, cx: &mut Context<Self>) -> Self {
        let logged_in = backend.app().is_logged_in();
        let has_engine = backend.has_engine();
        // 恢复持久化的播放模式设置
        let mode = backend.app().play_mode();
        let player = cx.new(|_| PlayerModel::new(has_engine, mode));
        let drain = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                for msg in receiver.try_iter() {
                    if this.update(cx, |shell, cx| shell.apply(msg, cx)).is_err() {
                        return;
                    }
                }
                let _ = this.update(cx, AppShell::expire_toast);
            }
        });
        let initial_nav = if logged_in { Nav::Daily } else { Nav::Login };
        Self {
            backend,
            nav: initial_nav,
            history: vec![initial_nav],
            history_idx: 0,
            logged_in,
            player,
            views: None,
            toast: None,
            toast_deadline: None,
            show_queue: false,
            closing_lyrics: false,
            closing_lyrics_seq: 1,
            _lyrics_close_task: None,
            nav_seq: 1,
            toast_seq: 1,
            queue_seq: 1,
            lyrics_seq: 0,
            auto_skips: 0,
            _drain: drain,
        }
    }

    /// 初始化并挂载各子视图
    pub(crate) fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.views.is_some() {
            return;
        }
        let shell = cx.entity().downgrade();
        let backend = self.backend.clone();
        let player = self.player.clone();

        let player_bar = cx.new(|cx| PlayerBar::new(backend.clone(), shell.clone(), player.clone(), window, cx));
        let playlists = cx.new(|cx| PlaylistsView::new(backend.clone(), shell.clone(), player.clone(), cx));
        let daily = cx.new(|cx| DailyView::new(backend.clone(), shell.clone(), player.clone(), cx));
        let radio = cx.new(|cx| RadioView::new(backend.clone(), shell.clone(), player.clone(), cx));
        let search = cx.new(|cx| SearchView::new(backend.clone(), shell.clone(), player.clone(), window, cx));
        let settings = cx.new(|cx| SettingsView::new(backend.clone(), window, cx));
        let login = cx.new(|cx| LoginView::new(backend.clone(), window, cx));
        let lyrics = cx.new(|cx| LyricsView::new(backend, shell.clone(), player, cx));

        self.views = Some(Views {
            player_bar,
            playlists: playlists.clone(),
            daily,
            radio,
            search,
            settings,
            login,
            lyrics,
        });
        self.ensure_loaded(cx);
        playlists.update(cx, PlaylistsView::ensure);
    }

    /// 分发处理后台异步事件
    fn apply(&mut self, msg: Msg, cx: &mut Context<Self>) {
        let (playlists, daily, radio, search, settings, login, lyrics) = {
            let Some(views) = self.views.as_ref() else {
                return;
            };
            (
                views.playlists.clone(),
                views.daily.clone(),
                views.radio.clone(),
                views.search.clone(),
                views.settings.clone(),
                views.login.clone(),
                views.lyrics.clone(),
            )
        };

        match msg {
            Msg::AuthCode(result) => login.update(cx, |view, cx| view.on_auth_code(result, cx)),
            Msg::Login(result) => {
                let ok = result.is_ok();
                login.update(cx, |view, cx| view.on_login(result, cx));
                if ok {
                    self.on_logged_in(cx);
                }
            }
            Msg::Playlists(result) => playlists.update(cx, |view, cx| view.on_playlists(result, cx)),
            Msg::Tracks(seq, result) => playlists.update(cx, |view, cx| view.on_tracks(seq, result, cx)),
            Msg::Daily(result) => daily.update(cx, |view, cx| view.on_daily(result, cx)),
            Msg::Search(seq, result) => search.update(cx, |view, cx| view.on_search(seq, result, cx)),
            Msg::Stations(result) => radio.update(cx, |view, cx| view.on_stations(result, cx)),
            Msg::StationClasses(result) => radio.update(cx, |view, cx| view.on_classes(result, cx)),
            Msg::StationSongs(seq, result) => radio.update(cx, |view, cx| view.on_songs(seq, result, cx)),
            Msg::Lyrics(seq, result) => {
                // 忽略过期请求的歌词响应
                if seq == self.lyrics_seq {
                    lyrics.update(cx, |view, cx| view.set_lines(result, cx));
                }
            }
            Msg::Vip(result) => settings.update(cx, |view, cx| view.on_vip(result, cx)),
            Msg::VipClaim(result) => settings.update(cx, |view, cx| view.on_claim(result, cx)),
            Msg::SettingsSaved(result) => settings.update(cx, |view, cx| view.on_saved(result, cx)),
            Msg::TrackRemoved(result) => match result {
                Ok(()) => {
                    playlists.update(cx, PlaylistsView::refresh_current);
                    self.set_toast("已移除".to_owned(), cx);
                }
                Err(error) => self.set_toast(format!("移除失败: {error}"), cx),
            },
            Msg::AudioEnded => {
                // 正常播放结束，重置连续跳过计数
                self.auto_skips = 0;
                self.play_next(cx);
            }
            // 播放失败处理：提示并尝试跳过
            Msg::AudioFailed { hash, message } => self.on_play_failed(&hash, &message, cx),
            Msg::EngineFailed(message) => self.on_engine_failed(&message, cx),
            Msg::Noop => {}
            Msg::Toast(text) => self.set_toast(text, cx),
            Msg::LoggedOut => self.on_logged_out(cx),
            Msg::RemoveTrack { listid, fileid } => self.remove_track(&listid, fileid),
        }
    }

    /// 切换导航页面并记录历史栈
    pub(crate) fn set_nav(&mut self, nav: Nav, cx: &mut Context<Self>) {
        self.closing_lyrics = false;
        self._lyrics_close_task = None;
        if self.nav != nav {
            self.history.truncate(self.history_idx + 1);
            self.history.push(nav);
            self.history_idx = self.history.len() - 1;
            self.nav = nav;
            self.nav_seq += 1;
            self.ensure_loaded(cx);
            cx.notify();
        }
    }

    /// 后退至历史上一页
    pub(crate) fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.nav == Nav::Lyrics {
            if !self.closing_lyrics {
                self.close_lyrics(cx);
            }
            return;
        }
        if self.history_idx > 0 {
            self.history_idx -= 1;
            self.nav = self.history[self.history_idx];
            self.nav_seq += 1;
            self.ensure_loaded(cx);
            cx.notify();
        }
    }

    /// 关闭歌词大屏（伴随平滑向下滑出退出动效）
    pub(crate) fn close_lyrics(&mut self, cx: &mut Context<Self>) {
        if self.nav != Nav::Lyrics || self.closing_lyrics {
            return;
        }
        self.closing_lyrics = true;
        self.closing_lyrics_seq += 1;
        cx.notify();

        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::motion::LYRICS_TRANSITION_DURATION)
                .await;
            this.update(cx, |shell, cx| {
                shell.closing_lyrics = false;
                shell._lyrics_close_task = None;
                while shell.history_idx > 0 && shell.history[shell.history_idx] == Nav::Lyrics {
                    shell.history_idx -= 1;
                }
                if shell.history[shell.history_idx] == Nav::Lyrics {
                    shell.nav = Nav::Playlists;
                } else {
                    shell.nav = shell.history[shell.history_idx];
                }
                shell.nav_seq += 1;
                shell.ensure_loaded(cx);
                cx.notify();
            })
            .ok();
        });
        self._lyrics_close_task = Some(task);
    }

    /// 前进至历史下一页
    pub(crate) fn go_forward(&mut self, cx: &mut Context<Self>) {
        if self.history_idx + 1 < self.history.len() {
            self.history_idx += 1;
            self.nav = self.history[self.history_idx];
            self.nav_seq += 1;
            self.ensure_loaded(cx);
            cx.notify();
        }
    }

    /// 切换播放队列抽屉展开状态
    pub(crate) fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        self.show_queue = !self.show_queue;
        if self.show_queue {
            self.queue_seq += 1;
        }
        cx.notify();
    }

    /// 激活视图时触发数据加载
    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let Some(views) = self.views.as_ref() else {
            return;
        };
        match self.nav {
            Nav::Playlists => views.playlists.update(cx, PlaylistsView::ensure),
            Nav::Daily => views.daily.update(cx, DailyView::ensure),
            Nav::Radio => views.radio.update(cx, RadioView::ensure),
            Nav::Settings => views.settings.update(cx, |view, _cx| view.ensure()),
            Nav::Login | Nav::Search | Nav::Lyrics => {}
        }
    }

    /// 设置播放队列并播放指定索引歌曲
    pub(crate) fn play_queue(&mut self, songs: Arc<Vec<Song>>, index: usize, cx: &mut Context<Self>) {
        if index >= songs.len() {
            return;
        }
        self.auto_skips = 0;
        self.player.update(cx, |model, model_cx| {
            model.set_queue(songs, index);
            model_cx.notify();
        });
        self.play_index(cx);
        self.load_lyrics(cx);
    }

    /// 播放当前选中歌曲
    fn play_index(&mut self, cx: &mut Context<Self>) {
        let Some(song) = self.player.read(cx).current_song().cloned() else {
            return;
        };
        let app = self.backend.app();
        let quality = app
            .settings()
            .map_or_else(|_| "flac".to_owned(), |settings| settings.quality);
        // 携带歌曲哈希以在异步回调中核对曲目一致性
        let hash = song.hash.clone();
        self.backend.spawn(async move {
            match app.play(&song, &quality).await {
                // 上游仅授权试听片段
                Ok(playback) if playback.preview_ms > 0 => Msg::Toast(format!(
                    "仅试听 {} 秒（完整版需会员或购买）",
                    playback.preview_ms / 1000
                )),
                Ok(_) => Msg::Noop,
                Err(error) => Msg::AudioFailed {
                    hash,
                    message: error.to_string(),
                },
            }
        });
    }

    /// 处理取址失败（忽略过期响应，提示并尝试跳过）
    fn on_play_failed(&mut self, hash: &str, message: &str, cx: &mut Context<Self>) {
        // 忽略非当前曲目的失败响应
        let is_current = self
            .player
            .read(cx)
            .current_song()
            .is_some_and(|song| song.hash == hash);
        if !is_current {
            return;
        }
        self.set_toast(format!("播放失败: {message}"), cx);
        self.skip_unplayable(cx);
    }

    /// 处理音频引擎拉流或解码失败事件
    fn on_engine_failed(&mut self, message: &str, cx: &mut Context<Self>) {
        self.set_toast(format!("播放失败: {message}"), cx);
        self.skip_unplayable(cx);
    }

    /// 当前曲目不可播时尝试跳过至下一首
    fn skip_unplayable(&mut self, cx: &mut Context<Self>) {
        if !should_skip_unplayable(self.auto_skips) {
            tracing::warn!(skips = self.auto_skips, "连续跳过过多，停止自动跳过");
            return;
        }
        self.auto_skips += 1;
        self.play_next(cx);
    }

    /// 播放下一首歌曲（遵循当前播放模式）
    pub(crate) fn play_next(&mut self, cx: &mut Context<Self>) {
        let next_idx = self.player.read(cx).next_index();
        if let Some(idx) = next_idx {
            self.player.update(cx, |model, model_cx| {
                model.index = idx;
                model_cx.notify();
            });
            self.play_index(cx);
            self.load_lyrics(cx);
        }
    }

    /// 播放上一首歌曲（遵循当前播放模式）
    pub(crate) fn play_prev(&mut self, cx: &mut Context<Self>) {
        self.auto_skips = 0;
        let prev_idx = self.player.read(cx).prev_index();
        if let Some(idx) = prev_idx {
            self.player.update(cx, |model, model_cx| {
                model.index = idx;
                model_cx.notify();
            });
            self.play_index(cx);
            self.load_lyrics(cx);
        }
    }

    /// 异步加载当前歌曲歌词
    fn load_lyrics(&mut self, cx: &mut Context<Self>) {
        let Some(song) = self.player.read(cx).current_song().cloned() else {
            return;
        };
        self.lyrics_seq += 1;
        let seq = self.lyrics_seq;
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Lyrics(seq, app.lyrics(&song).await.map_err(|error| error.to_string())) });
    }

    /// 从歌单中移除指定歌曲
    fn remove_track(&mut self, listid: &str, fileid: i64) {
        let app = self.backend.app();
        let listid = listid.to_owned();
        self.backend.spawn(async move {
            Msg::TrackRemoved(app.remove(&listid, &[fileid]).await.map_err(|error| error.to_string()))
        });
    }

    /// 处理登录成功事件
    fn on_logged_in(&mut self, cx: &mut Context<Self>) {
        self.logged_in = true;
        self.set_toast("登录成功".to_owned(), cx);
        if let Some(views) = self.views.as_ref() {
            views.login.update(cx, LoginView::reset);
            views.playlists.update(cx, PlaylistsView::reload);
            views.daily.update(cx, DailyView::reload);
            views.settings.update(cx, |view, _cx| view.ensure());
        }
        self.nav = Nav::Playlists;
        self.nav_seq += 1;
        cx.notify();
    }

    /// 处理退出登录事件
    fn on_logged_out(&mut self, cx: &mut Context<Self>) {
        self.logged_in = false;
        self.nav = Nav::Login;
        self.nav_seq += 1;
        // 清除旧账号视图状态
        if let Some(views) = self.views.as_ref() {
            views.playlists.update(cx, PlaylistsView::reset);
            views.daily.update(cx, DailyView::reset);
            views.radio.update(cx, RadioView::reset);
            views.lyrics.update(cx, LyricsView::clear);
            views.search.update(cx, SearchView::reset);
        }
        self.set_toast("已登出".to_owned(), cx);
    }

    /// 展示全局轻提示
    fn set_toast(&mut self, text: String, cx: &mut Context<Self>) {
        self.toast_seq += 1;
        self.toast = Some(SharedString::from(text));
        self.toast_deadline = Some(Instant::now() + TOAST_TTL);
        cx.notify();
    }

    /// 清除已过期的轻提示
    fn expire_toast(&mut self, cx: &mut Context<Self>) {
        if self.toast_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            self.toast = None;
            self.toast_deadline = None;
            cx.notify();
        }
    }

    /// 渲染主内容区顶栏
    fn top_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let is_search = self.nav == Nav::Search;
        let can_go_back = self.history_idx > 0;
        let can_go_forward = self.history_idx + 1 < self.history.len();

        h_flex()
            .w_full()
            .h(px(46.0))
            .px_4()
            .items_center()
            .gap(px(12.0))
            .border_b_1()
            .border_color(CARD_BORDER)
            // 历史前进后退
            .child(
                h_flex()
                    .gap(px(4.0))
                    .items_center()
                    .child(
                        Button::new("nav_prev")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronLeft)
                            .tooltip("后退")
                            .disabled(!can_go_back)
                            .on_click(cx.listener(|shell, _event, _window, cx| shell.go_back(cx))),
                    )
                    .child(
                        Button::new("nav_next")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronRight)
                            .tooltip("前进")
                            .disabled(!can_go_forward)
                            .on_click(cx.listener(|shell, _event, _window, cx| shell.go_forward(cx))),
                    ),
            )
            // 胶囊搜索框
            .child(
                div()
                    .id("top_search_capsule")
                    .role(Role::Button)
                    .cursor_pointer()
                    .w(px(220.0))
                    .h(px(32.0))
                    .px_3()
                    .rounded(px(999.0))
                    .bg(if is_search { ACCENT_GREEN_LIGHT } else { hex_rgb(0xeef1f5) })
                    .border_1()
                    .border_color(if is_search { ACCENT_GREEN_BORDER } else { hex_rgb(0xe5e7eb) })
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .on_click(cx.listener(|shell, _event, _window, cx| shell.set_nav(Nav::Search, cx)))
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(14.0))
                            .text_color(if is_search { ACCENT_GREEN } else { TEXT_FAINT }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(if is_search { FontWeight::BOLD } else { FontWeight::NORMAL })
                            .text_color(if is_search { ACCENT_GREEN } else { TEXT_MUTED })
                            .child("搜索音乐"),
                    ),
            )
            .child(div().flex_1())
            // 右侧工具区
            .child(
                h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        Button::new("top_refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .tooltip("刷新当前数据")
                            .on_click(cx.listener(|shell, _event, _window, cx| shell.ensure_loaded(cx))),
                    ),
            )
    }

    /// 渲染左侧导航侧边栏
    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut column = v_flex().id("sidebar").w(px(216.0)).h_full().p_2().gap(px(10.0));

        // 用户身份卡片
        let username = self
            .backend
            .app()
            .store()
            .account()
            .ok()
            .flatten()
            .and_then(|a| if a.nickname.is_empty() { None } else { Some(a.nickname) })
            .unwrap_or_else(|| {
                if self.logged_in {
                    "已登录用户".to_owned()
                } else {
                    "点击登录账号".to_owned()
                }
            });

        let user_capsule = div()
            .id("user_capsule")
            .role(Role::Button)
            .cursor_pointer()
            .w_full()
            .px_2()
            .py_1()
            .rounded(px(10.0))
            .hover(|s| s.bg(SURFACE_HOVER))
            .on_click(cx.listener(|shell, _event, _window, cx| {
                if shell.logged_in {
                    shell.set_nav(Nav::Settings, cx);
                } else {
                    shell.set_nav(Nav::Login, cx);
                }
            }))
            .child(
                h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        div()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded_full()
                            .bg(if self.logged_in {
                                ACCENT_GREEN_LIGHT
                            } else {
                                hex_rgb(0xe5e7eb)
                            })
                            .border_1()
                            .border_color(if self.logged_in {
                                ACCENT_GREEN_BORDER
                            } else {
                                hex_rgb(0xd1d5db)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::CircleUser)
                                    .size(px(18.0))
                                    .text_color(if self.logged_in { ACCENT_GREEN } else { TEXT_MUTED }),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(TEXT_PRIMARY)
                                    .truncate()
                                    .child(username),
                            )
                            .when(self.logged_in, |this| {
                                this.child(div().text_xs().text_color(TEXT_FAINT).child("设置"))
                            }),
                    )
                    .when(self.logged_in, |this| {
                        this.child(Icon::new(IconName::Settings).size(px(14.0)).text_color(TEXT_FAINT))
                    }),
            );
        column = column.child(user_capsule);

        // 快捷导航栏
        let quick_items = [
            (Nav::Daily, IconName::House, "发现"),
            (Nav::Radio, IconName::Radio, "电台"),
            (Nav::Search, IconName::Search, "搜索"),
        ];

        let mut quick_row = h_flex().w_full().gap(px(6.0));
        for (idx, (nav, icon, label)) in quick_items.into_iter().enumerate() {
            let active = self.nav == nav;
            let btn = div()
                .id(("quick_nav", idx))
                .role(Role::Button)
                .cursor_pointer()
                .flex_1()
                .h(px(32.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(4.0))
                .when(active, |this| {
                    this.bg(hex_rgb(0x111827)).text_color(hex_rgb(0xffffff)).shadow_sm()
                })
                .when(!active, |this| {
                    this.bg(hex_rgb(0xe5e9ef))
                        .text_color(hex_rgb(0x4b5563))
                        .hover(|s| s.bg(hex_rgb(0xd9dfe7)).text_color(hex_rgb(0x111827)))
                })
                .on_click(cx.listener(move |shell, _event, _window, cx| shell.set_nav(nav, cx)))
                .child(Icon::new(icon).size(px(14.0)))
                .child(div().text_xs().font_weight(FontWeight::MEDIUM).child(label));

            quick_row = quick_row.child(btn);
        }
        column = column.child(quick_row);

        // 歌单胶囊切换器与侧边栏歌单列表
        let (tab, tab_seq, playlists, selected, loading_playlists) = if let Some(views) = self.views.as_ref() {
            let p = views.playlists.read(cx);
            (p.tab, p.tab_seq, p.playlists.clone(), p.selected, p.loading_playlists)
        } else {
            (PlaylistTab::Created, 1, Arc::new(Vec::new()), None, false)
        };

        let pill_base = div()
            .id("capsule_slider_pill")
            .absolute()
            .top(px(3.0))
            .w(px(97.0))
            .h(px(26.0))
            .rounded(px(999.0))
            .bg(CARD_BG)
            .shadow_sm();

        let pill: AnyElement = if tab_seq <= 1 {
            let left_pos = if tab == PlaylistTab::Created { px(3.0) } else { px(100.0) };
            pill_base.left(left_pos).into_any_element()
        } else {
            let (from_x, to_x) = if tab == PlaylistTab::Collected {
                (3.0, 100.0)
            } else {
                (100.0, 3.0)
            };
            crate::motion::capsule_pill_sliding(pill_base, ("capsule_pill_anim", tab_seq), from_x, to_x)
                .into_any_element()
        };

        let is_created = tab == PlaylistTab::Created;
        let is_collected = tab == PlaylistTab::Collected;

        let tab_capsule = div()
            .relative()
            .w_full()
            .h(px(32.0))
            .p(px(3.0))
            .bg(hex_rgb(0xe5e9ef))
            .rounded(px(999.0))
            .child(pill)
            .child(
                h_flex()
                    .relative()
                    .size_full()
                    .child(
                        div()
                            .id("sidebar_tab_created")
                            .role(Role::Button)
                            .cursor_pointer()
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .when(is_created, |s| {
                                s.text_color(TEXT_PRIMARY).font_weight(FontWeight::BOLD)
                            })
                            .when(!is_created, |s| {
                                s.text_color(TEXT_MUTED)
                                    .font_weight(FontWeight::MEDIUM)
                                    .hover(|h| h.text_color(TEXT_PRIMARY))
                            })
                            .on_click(cx.listener(|shell, _event, _window, cx| {
                                if let Some(views) = shell.views.as_ref() {
                                    views
                                        .playlists
                                        .update(cx, |p, cx| p.switch_tab(PlaylistTab::Created, cx));
                                }
                                shell.set_nav(Nav::Playlists, cx);
                            }))
                            .child("自建歌单"),
                    )
                    .child(
                        div()
                            .id("sidebar_tab_collected")
                            .role(Role::Button)
                            .cursor_pointer()
                            .flex_1()
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .when(is_collected, |s| {
                                s.text_color(TEXT_PRIMARY).font_weight(FontWeight::BOLD)
                            })
                            .when(!is_collected, |s| {
                                s.text_color(TEXT_MUTED)
                                    .font_weight(FontWeight::MEDIUM)
                                    .hover(|h| h.text_color(TEXT_PRIMARY))
                            })
                            .on_click(cx.listener(|shell, _event, _window, cx| {
                                if let Some(views) = shell.views.as_ref() {
                                    views
                                        .playlists
                                        .update(cx, |p, cx| p.switch_tab(PlaylistTab::Collected, cx));
                                }
                                shell.set_nav(Nav::Playlists, cx);
                            }))
                            .child("收藏歌单"),
                    ),
            );
        column = column.child(tab_capsule);

        let mut playlist_list = v_flex().w_full().gap(px(3.0));

        if loading_playlists && playlists.is_empty() {
            playlist_list = playlist_list.child(notice_with_icon(IconName::RefreshCw, "加载歌单中…", cx));
        }

        let filtered_playlists: Vec<(usize, &Playlist)> = playlists
            .iter()
            .enumerate()
            .filter(|(_, list)| match tab {
                PlaylistTab::Created => list.is_liked() || list.kind != 1,
                PlaylistTab::Collected => !list.is_liked() && list.kind == 1,
            })
            .collect();

        if !loading_playlists && filtered_playlists.is_empty() {
            playlist_list = playlist_list.child(div().py_6().child(notice_with_icon(
                IconName::ListMusic,
                if tab == PlaylistTab::Created {
                    "暂无自建歌单"
                } else {
                    "暂无收藏歌单"
                },
                cx,
            )));
        }

        for (index, list) in filtered_playlists {
            let active = self.nav == Nav::Playlists && selected == Some(index);
            let name = list.name.clone();
            let count_str = format!("{}", list.count);
            let is_liked = list.is_liked();

            playlist_list = playlist_list.child(
                div()
                    .id(("sidebar_playlist", index))
                    .role(Role::Button)
                    .w_full()
                    .px_2()
                    .py(px(5.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(active, |this| {
                        this.bg(ACCENT_GREEN_LIGHT)
                            .border_1()
                            .border_color(ACCENT_GREEN_BORDER)
                            .text_color(ACCENT_GREEN_HOVER)
                    })
                    .when(!active, |this| {
                        this.text_color(TEXT_MUTED)
                            .hover(|style| style.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
                    })
                    .on_click(cx.listener(move |shell, _event, _window, cx| {
                        if let Some(views) = shell.views.as_ref() {
                            views.playlists.update(cx, |p, cx| p.select(index, cx));
                        }
                        shell.set_nav(Nav::Playlists, cx);
                    }))
                    .child(if is_liked {
                        div()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(6.0))
                            .bg(hex_rgb(0xfee2e2))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Heart).size(px(14.0)).text_color(hex_rgb(0xef4444)))
                            .into_any_element()
                    } else {
                        cover_box(&name, 28.0, cx).into_any_element()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_xs()
                            .font_weight(if active { FontWeight::BOLD } else { FontWeight::NORMAL })
                            .truncate()
                            .child(name),
                    )
                    .child(badge(count_str, hex_rgb(0xeef1f5), TEXT_MUTED)),
            );
        }

        let playlist_scroll = div()
            .id("sidebar-playlists-scroll")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scrollbar()
            .child(playlist_list);

        column = column.child(playlist_scroll);

        column
    }

    /// 渲染浮动轻提示
    fn toast_element(&self, _cx: &App) -> Option<AnyElement> {
        let toast_id = ("toast_entrance", self.toast_seq);
        self.toast.clone().map(|text| {
            crate::motion::toast_entrance(
                div()
                    .absolute()
                    .top(px(16.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .px_5()
                            .py_2()
                            .rounded(px(999.0))
                            .bg(CARD_BG)
                            .border_1()
                            .border_color(CARD_BORDER)
                            .shadow_lg()
                            .child(Icon::new(IconName::Sparkles).size(px(16.0)).text_color(ACCENT_GREEN))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(TEXT_PRIMARY)
                                    .child(text),
                            ),
                    ),
                toast_id,
            )
            .into_any_element()
        })
    }

    /// 渲染右侧播放队列抽屉
    fn queue_drawer(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.show_queue {
            return None;
        }

        let (queue, current_idx, is_playing, mode) = {
            let model = self.player.read(cx);
            (
                model.queue.clone(),
                model.index,
                model.snapshot.state == lyra_audio::State::Playing,
                model.mode,
            )
        };

        let mut panel = main_card()
            .w(px(320.0))
            .h_full()
            .p_3()
            .gap(px(8.0))
            .flex()
            .flex_col()
            .shadow_lg();

        // 队列标题与控制按钮
        let header = h_flex()
            .w_full()
            .pb_2()
            .border_b_1()
            .border_color(CARD_BORDER)
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(Icon::new(IconName::ListMusic).size(px(18.0)).text_color(ACCENT_GREEN))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(TEXT_PRIMARY)
                            .child("播放队列"),
                    )
                    .child(badge(
                        format!("{} 首", queue.len()),
                        ACCENT_GREEN_LIGHT,
                        ACCENT_GREEN_HOVER,
                    )),
            )
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(badge(mode.label(), hex_rgb(0xf2f4f7), TEXT_MUTED))
                    .child(
                        Button::new("close_queue")
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .tooltip("收起队列")
                            .on_click(cx.listener(|shell, _event, _window, cx| shell.toggle_queue(cx))),
                    ),
            );

        panel = panel.child(header);

        if queue.is_empty() {
            panel = panel.child(notice_with_icon(IconName::ListMusic, "队列暂无歌曲", cx));
        } else {
            let mut list = v_flex()
                .id("queue_tracks_scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .gap(px(4.0));
            for (idx, song) in queue.iter().enumerate() {
                let is_current = idx == current_idx;
                let play_queue = queue.clone();
                let shell = cx.entity().downgrade();

                list = list.child(
                    h_flex()
                        .id(("q_track", idx))
                        .role(Role::Button)
                        .w_full()
                        .px_2()
                        .py_2()
                        .rounded(px(6.0))
                        .gap(px(8.0))
                        .items_center()
                        .cursor_pointer()
                        .when(is_current, |this| {
                            this.bg(ACCENT_GREEN_LIGHT).border_1().border_color(ACCENT_GREEN_BORDER)
                        })
                        .when(!is_current, |this| this.hover(|s| s.bg(SURFACE_HOVER)))
                        .on_click(move |_event, _window, cx| {
                            shell
                                .update(cx, |shell, cx| shell.play_queue(play_queue.clone(), idx, cx))
                                .ok();
                        })
                        .child(
                            div()
                                .w(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(is_current, |this| {
                                    if is_playing {
                                        this.child(crate::motion::pulsating_icon(
                                            Icon::new(IconName::AudioLines).size(px(14.0)).text_color(ACCENT_GREEN),
                                            ("q_track_wave", idx),
                                        ))
                                    } else {
                                        this.child(Icon::new(IconName::Music).size(px(14.0)).text_color(ACCENT_GREEN))
                                    }
                                })
                                .when(!is_current, |this| {
                                    this.child(div().text_xs().text_color(TEXT_FAINT).child(format!("{:02}", idx + 1)))
                                }),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(0.0))
                                .gap(px(1.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(if is_current {
                                            FontWeight::BOLD
                                        } else {
                                            FontWeight::MEDIUM
                                        })
                                        .text_color(if is_current { ACCENT_GREEN_HOVER } else { TEXT_PRIMARY })
                                        .truncate()
                                        .child(SharedString::from(song.name.clone())),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(TEXT_MUTED)
                                        .truncate()
                                        .child(SharedString::from(song.singer.clone())),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(TEXT_FAINT)
                                .child(SharedString::from(format_duration(song.duration_ms))),
                        ),
                );
            }
            panel = panel.child(list);
        }

        Some(crate::motion::drawer_entrance(panel, ("queue_drawer_panel", self.queue_seq)).into_any_element())
    }
}

impl Render for AppShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(views) = self.views.as_ref() else {
            return notice("初始化中…", cx);
        };
        // 全屏沉浸式歌词视图模式（非退出过渡期）
        if self.nav == Nav::Lyrics && !self.closing_lyrics {
            let lyrics_card = crate::motion::lyrics_open_entrance(
                main_card().size_full().overflow_hidden().child(views.lyrics.clone()),
                ("lyrics_fullscreen_card", self.nav_seq),
            );
            return v_flex()
                .size_full()
                .bg(CANVAS_BG)
                .text_color(TEXT_PRIMARY)
                .p(px(8.0))
                .gap(px(8.0))
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .child(lyrics_card)
                        .children(self.queue_drawer(cx))
                        .children(self.toast_element(cx)),
                )
                .child(views.player_bar.clone())
                .into_any_element();
        }

        let effective_nav = if self.nav == Nav::Lyrics {
            let mut idx = self.history_idx;
            while idx > 0 && self.history[idx] == Nav::Lyrics {
                idx -= 1;
            }
            if self.history[idx] == Nav::Lyrics {
                Nav::Playlists
            } else {
                self.history[idx]
            }
        } else {
            self.nav
        };

        let body = match effective_nav {
            Nav::Playlists => views.playlists.clone().into_any_element(),
            Nav::Daily => views.daily.clone().into_any_element(),
            Nav::Radio => views.radio.clone().into_any_element(),
            Nav::Search => views.search.clone().into_any_element(),
            Nav::Lyrics => views.lyrics.clone().into_any_element(),
            Nav::Settings => views.settings.clone().into_any_element(),
            Nav::Login => views.login.clone().into_any_element(),
        };

        let lyrics_exit_overlay = if self.closing_lyrics {
            let exiting_card = crate::motion::lyrics_close_exit(
                main_card().size_full().overflow_hidden().child(views.lyrics.clone()),
                ("lyrics_fullscreen_exit", self.closing_lyrics_seq),
            );
            Some(div().absolute().inset_0().child(exiting_card))
        } else {
            None
        };

        // 根容器填满窗口视口
        v_flex()
            .size_full()
            .bg(CANVAS_BG)
            .text_color(TEXT_PRIMARY)
            .p(px(8.0))
            .gap(px(8.0))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(px(8.0))
                    .child(self.sidebar(cx))
                    .child(
                        main_card()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .overflow_hidden()
                            .child(self.top_header(cx))
                            .child(div().flex_1().min_h(px(0.0)).child(body)),
                    )
                    .children(lyrics_exit_overlay)
                    .children(self.queue_drawer(cx))
                    .children(self.toast_element(cx)),
            )
            .child(views.player_bar.clone())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

    // 避免与 gpui_kit::* 宏冲突
    use super::{MAX_AUTO_SKIPS, should_skip_unplayable};

    #[test]
    fn auto_skip_stops_after_the_cap() {
        assert!(should_skip_unplayable(0), "第一次失败必须跳到下一首");
        assert!(should_skip_unplayable(MAX_AUTO_SKIPS - 1));
        assert!(
            !should_skip_unplayable(MAX_AUTO_SKIPS),
            "连续跳过到达上限后应停下，避免整队列空转"
        );
    }
}
