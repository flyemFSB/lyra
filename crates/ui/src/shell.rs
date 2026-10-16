//! 应用主外壳：导航、内容承载与全局播放控制。

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::Song;

use crate::bridge::{Backend, Msg, Nav};
use crate::theme::notice;
use crate::views::daily::DailyView;
use crate::views::login::LoginView;
use crate::views::lyrics::LyricsView;
use crate::views::player::{PlayerBar, PlayerModel};
use crate::views::playlists::PlaylistsView;
use crate::views::radio::RadioView;
use crate::views::search::SearchView;
use crate::views::settings::SettingsView;

/// 侧边栏导航条目列表。
const NAV_ITEMS: &[(Nav, &str)] = &[
    (Nav::Playlists, "歌单"),
    (Nav::Daily, "每日推荐"),
    (Nav::Radio, "电台"),
    (Nav::Search, "搜索"),
    (Nav::Lyrics, "歌词"),
    (Nav::Settings, "设置"),
];

/// 轻提示默认展示时长。
const TOAST_TTL: Duration = Duration::from_secs(4);

/// 缓存的子视图实体集合。
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

/// 主应用外壳状态结构。
pub(crate) struct AppShell {
    backend: Backend,
    nav: Nav,
    logged_in: bool,
    player: Entity<PlayerModel>,
    views: Option<Views>,
    toast: Option<SharedString>,
    toast_deadline: Option<Instant>,
    /// 歌词请求序号：只有最新一次请求的响应才允许落地。
    lyrics_seq: u64,
    _drain: Task<()>,
}

impl AppShell {
    /// 创建主外壳状态。
    pub(crate) fn new(backend: Backend, receiver: Receiver<Msg>, cx: &mut Context<Self>) -> Self {
        let logged_in = backend.app().is_logged_in();
        let has_engine = backend.has_engine();
        let player = cx.new(|_| PlayerModel::new(has_engine));
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
        Self {
            backend,
            nav: if logged_in { Nav::Playlists } else { Nav::Login },
            logged_in,
            player,
            views: None,
            toast: None,
            toast_deadline: None,
            lyrics_seq: 0,
            _drain: drain,
        }
    }

    /// 初始化并挂载各子视图。
    pub(crate) fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.views.is_some() {
            return;
        }
        let shell = cx.entity().downgrade();
        let backend = self.backend.clone();
        let player = self.player.clone();

        let player_bar = cx.new(|cx| PlayerBar::new(backend.clone(), shell.clone(), player.clone(), window, cx));
        let playlists = cx.new(|cx| PlaylistsView::new(backend.clone(), shell.clone(), cx));
        let daily = cx.new(|cx| DailyView::new(backend.clone(), shell.clone(), cx));
        let radio = cx.new(|cx| RadioView::new(backend.clone(), shell.clone(), cx));
        let search = cx.new(|cx| SearchView::new(backend.clone(), shell.clone(), window, cx));
        let settings = cx.new(|cx| SettingsView::new(backend.clone(), window, cx));
        let login = cx.new(|cx| LoginView::new(backend, window, cx));
        let lyrics = cx.new(|cx| LyricsView::new(player, cx));

        self.views = Some(Views {
            player_bar,
            playlists,
            daily,
            radio,
            search,
            settings,
            login,
            lyrics,
        });
        self.ensure_loaded(cx);
    }

    /// 分发处理后台异步事件。
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
                // 快速切歌时丢弃旧请求的歌词，避免上一首覆盖当前曲目。
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
            Msg::AudioEnded => self.play_next(cx),
            // 播放失败时弹出轻提示通知用户。
            Msg::AudioFailed(message) => self.set_toast(format!("播放失败: {message}"), cx),
            Msg::Noop => {}
            Msg::Toast(text) => self.set_toast(text, cx),
            Msg::LoggedOut => self.on_logged_out(cx),
            Msg::RemoveTrack { listid, fileid } => self.remove_track(&listid, fileid),
        }
    }

    /// 切换导航页面并懒加载数据。
    fn set_nav(&mut self, nav: Nav, cx: &mut Context<Self>) {
        self.nav = nav;
        self.ensure_loaded(cx);
        cx.notify();
    }

    /// 页面激活时触发数据加载。
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

    /// 设置播放队列并播放指定索引歌曲。
    pub(crate) fn play_queue(&mut self, songs: Arc<Vec<Song>>, index: usize, cx: &mut Context<Self>) {
        if index >= songs.len() {
            return;
        }
        self.player.update(cx, |model, model_cx| {
            model.set_queue(songs, index);
            model_cx.notify();
        });
        self.play_index(cx);
        self.load_lyrics(cx);
    }

    /// 触发播放当前选中的歌曲。
    fn play_index(&mut self, cx: &mut Context<Self>) {
        let Some(song) = self.player.read(cx).current_song().cloned() else {
            return;
        };
        let app = self.backend.app();
        let quality = app
            .settings()
            .map_or_else(|_| "flac".to_owned(), |settings| settings.quality);
        self.backend.spawn(async move {
            match app.play(&song, &quality).await {
                Ok(()) => Msg::Noop,
                Err(error) => Msg::AudioFailed(error.to_string()),
            }
        });
    }

    /// 播放下一首歌曲。
    pub(crate) fn play_next(&mut self, cx: &mut Context<Self>) {
        let has_next = {
            let model = self.player.read(cx);
            model.index + 1 < model.queue.len()
        };
        if has_next {
            self.player.update(cx, |model, _cx| model.index += 1);
            self.play_index(cx);
            self.load_lyrics(cx);
        }
    }

    /// 播放上一首歌曲。
    pub(crate) fn play_prev(&mut self, cx: &mut Context<Self>) {
        let has_prev = self.player.read(cx).index > 0;
        if has_prev {
            self.player.update(cx, |model, _cx| model.index -= 1);
            self.play_index(cx);
            self.load_lyrics(cx);
        }
    }

    /// 请求加载当前播放歌曲的歌词。
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

    /// 从歌单中移除指定歌曲。
    fn remove_track(&mut self, listid: &str, fileid: i64) {
        let app = self.backend.app();
        let listid = listid.to_owned();
        self.backend.spawn(async move {
            Msg::TrackRemoved(app.remove(&listid, &[fileid]).await.map_err(|error| error.to_string()))
        });
    }

    /// 处理登录成功事件。
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
        cx.notify();
    }

    /// 处理退出登录事件。
    fn on_logged_out(&mut self, cx: &mut Context<Self>) {
        self.logged_in = false;
        self.nav = Nav::Login;
        // 清空上一账号的内容，避免换账号后仍显示旧歌单与旧 listid。
        if let Some(views) = self.views.as_ref() {
            views.playlists.update(cx, PlaylistsView::reset);
            views.daily.update(cx, DailyView::reset);
            views.radio.update(cx, RadioView::reset);
            views.lyrics.update(cx, LyricsView::clear);
            views.search.update(cx, SearchView::reset);
        }
        self.set_toast("已登出".to_owned(), cx);
    }

    /// 展示全局轻提示。
    fn set_toast(&mut self, text: String, cx: &mut Context<Self>) {
        self.toast = Some(SharedString::from(text));
        self.toast_deadline = Some(Instant::now() + TOAST_TTL);
        cx.notify();
    }

    /// 检查并清除已过期的轻提示。
    fn expire_toast(&mut self, cx: &mut Context<Self>) {
        if self.toast_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            self.toast = None;
            self.toast_deadline = None;
            cx.notify();
        }
    }

    /// 渲染左侧导航侧边栏。
    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut column = v_flex()
            .id("sidebar")
            .w(px(200.0))
            .h_full()
            .p_2()
            .gap(px(2.0))
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border);

        for (index, (nav, label)) in NAV_ITEMS.iter().enumerate() {
            let nav = *nav;
            let active = self.nav == nav;
            column = column.child(
                div()
                    .id(("nav", index))
                    .role(Role::Button)
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded(px(6.0))
                    .text_sm()
                    .cursor_pointer()
                    .when(active, |this| this.bg(theme.list_active).text_color(theme.foreground))
                    .when(!active, |this| {
                        this.text_color(theme.muted_foreground)
                            .hover(|style| style.bg(theme.list_hover))
                    })
                    .on_click(cx.listener(move |shell, _event, _window, cx| shell.set_nav(nav, cx)))
                    .child(SharedString::new_static(label)),
            );
        }

        column = column.child(div().flex_1());
        if self.logged_in {
            column = column.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("已登录"),
            );
        } else {
            column = column.child(
                Button::new("go_login")
                    .small()
                    .primary()
                    .label("登录")
                    .on_click(cx.listener(|shell, _event, _window, cx| shell.set_nav(Nav::Login, cx))),
            );
        }
        column
    }

    /// 渲染顶部轻提示条。
    fn toast_element(&self, cx: &App) -> Option<AnyElement> {
        let theme = cx.theme();
        self.toast.clone().map(|text| {
            div()
                .w_full()
                .px_4()
                .py_2()
                .bg(theme.muted)
                .text_color(theme.foreground)
                .text_sm()
                .child(text)
                .into_any_element()
        })
    }
}

impl Render for AppShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(views) = self.views.as_ref() else {
            return notice("初始化中…", cx);
        };
        let theme = cx.theme();
        let body = match self.nav {
            Nav::Playlists => views.playlists.clone().into_any_element(),
            Nav::Daily => views.daily.clone().into_any_element(),
            Nav::Radio => views.radio.clone().into_any_element(),
            Nav::Search => views.search.clone().into_any_element(),
            Nav::Lyrics => views.lyrics.clone().into_any_element(),
            Nav::Settings => views.settings.clone().into_any_element(),
            Nav::Login => views.login.clone().into_any_element(),
        };

        // gpui-pre 0.3.9 实测：列布局中排在 `flex_1` 项之后的固定高度子项会被压成 0 高，
        // 所以播放条与提示条必须排在内容行**之前**（视觉上也随之位于顶部）。
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(views.player_bar.clone())
            .children(self.toast_element(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.sidebar(cx))
                    .child(div().w(px(1.0)).h_full().bg(theme.border))
                    .child(div().flex().flex_col().flex_1().min_w(px(0.0)).h_full().child(body)),
            )
            .into_any_element()
    }
}
