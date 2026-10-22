//! 应用主外壳：导航、内容承载、播放模式调度与播放队列抽屉

use std::sync::Arc;
use std::time::Duration;

use async_channel::Receiver;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::{Page, Song};

use crate::bridge::{Backend, Msg, Nav};
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_BORDER, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CANVAS_BG, CARD_BG, CARD_BORDER,
    SURFACE_HOVER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, badge, cover_box, friendly_error_message, hex_rgb, main_card,
    notice, notice_with_icon, song_cover,
};
use crate::views::daily::DailyView;
use crate::views::login::LoginView;
use crate::views::lyrics::LyricsView;
use crate::views::player::{PlayerBar, PlayerModel, mode_label};
use crate::views::playlists::{PlaylistEntry, PlaylistTab, PlaylistsView};
use crate::views::search::SearchView;
use crate::views::settings::SettingsView;

/// 检查连续自动跳过次数是否未超限
fn should_skip_unplayable(skips: usize) -> bool {
    skips < MAX_AUTO_SKIPS
}

/// 轻提示默认展示时长（保持紧凑克制）
const TOAST_TTL: Duration = Duration::from_millis(2600);

/// 自动续播允许连续跳过的曲目上限（避免死循环）
const MAX_AUTO_SKIPS: usize = 5;

/// 缓存的子视图实体集合
struct Views {
    player_bar: Entity<PlayerBar>,
    playlists: Entity<PlaylistsView>,
    daily: Entity<DailyView>,
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
    /// 账号昵称（登录态变化时读一次，渲染期直接复用）
    nickname: Option<String>,
    /// 真实用户头像地址（启动后异步拉取，取不到时回退为占位图标）
    avatar: Option<SharedString>,
    player: Entity<PlayerModel>,
    views: Option<Views>,
    toast: Option<SharedString>,
    /// 是否展开右侧播放队列抽屉
    show_queue: bool,
    /// 歌词大屏正在收起退出过渡
    closing_lyrics: bool,
    /// 歌词收起动效序号
    closing_lyrics_seq: u64,
    lyrics_close_task: Option<Task<()>>,
    /// 导航切换序号
    pub(crate) nav_seq: u64,
    /// 轻提示变更序号（驱动浮动提示下落动效）
    toast_seq: u64,
    /// 播放队列抽屉切换序号（驱动抽屉滑入动效）
    queue_seq: u64,
    /// 歌词请求序列号（用于丢弃过期响应）
    lyrics_seq: u64,
    /// 播放请求序列号（用于丢弃过期的取址结果）
    play_seq: u64,
    /// 连续自动跳过的曲目计数
    auto_skips: usize,
    /// 轻提示过期计时任务（重复提示时自动取消前一个）
    toast_task: Option<Task<()>>,
    top_search: Option<Entity<InputState>>,
    top_search_sub: Option<Subscription>,
    /// 搜索框是否获焦（获焦时空格属于输入，不切换播放）
    top_search_focused: bool,
    personal_fm_mark: Option<String>,
    is_personal_fm_active: bool,
    is_fetching_personal_fm: bool,
    /// 电台首批数据尚未返回（区分首启与后台预取）
    is_starting_personal_fm: bool,
    /// 队列已播完，等待下一批数据自动续播
    awaiting_personal_fm: bool,
    /// 侧边栏“创建的歌单”折叠状态
    created_playlists_expanded: bool,
    /// 侧边栏“收藏的歌单”折叠状态
    collected_playlists_expanded: bool,
    _drain: Task<()>,
}

/// 用户头像占位（未取到真实头像或加载失败时的降级样式）
fn avatar_placeholder(logged_in: bool) -> AnyElement {
    div()
        .w(px(24.0))
        .h(px(24.0))
        .flex_shrink_0()
        .rounded_full()
        .bg(if logged_in {
            ACCENT_GREEN_LIGHT
        } else {
            hex_rgb(0xe5e7eb)
        })
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(IconName::User).size(px(13.0)).text_color(if logged_in {
            ACCENT_GREEN
        } else {
            TEXT_MUTED
        }))
        .into_any_element()
}

/// 读取当前账号昵称（只在登录态变化时调用，渲染期不得碰数据库）
fn read_nickname(app: &lyra_app::Kugou) -> Option<String> {
    app.store()
        .account()
        .ok()
        .flatten()
        .map(|account| account.nickname)
        .filter(|nickname| !nickname.is_empty())
}

impl AppShell {
    /// 创建主外壳状态
    pub(crate) fn new(backend: Backend, receiver: Receiver<Msg>, cx: &mut Context<Self>) -> Self {
        let logged_in = backend.app().is_logged_in();
        let has_engine = backend.has_engine();
        // 恢复持久化的播放模式与音量设置
        let mode = backend.app().play_mode();
        let volume = backend.app().volume();
        let muted = backend.app().is_muted();
        let initial_volume = if muted { 0.0 } else { volume };
        let player = cx.new(|_| PlayerModel::new(has_engine, mode, initial_volume));
        // 事件驱动收取后台消息：通道为空时任务挂起，不占用 CPU，
        // 也不需要用固定间隔去轮询。
        let drain = cx.spawn(async move |this, cx| {
            while let Ok(msg) = receiver.recv().await {
                if this.update(cx, |shell, cx| shell.apply(msg, cx)).is_err() {
                    return;
                }
            }
        });
        let initial_nav = if logged_in { Nav::Daily } else { Nav::Login };
        let nickname = read_nickname(&backend.app());
        Self {
            backend,
            nav: initial_nav,
            history: vec![initial_nav],
            history_idx: 0,
            logged_in,
            nickname,
            avatar: None,
            player,
            views: None,
            toast: None,
            show_queue: false,
            closing_lyrics: false,
            closing_lyrics_seq: 1,
            lyrics_close_task: None,
            nav_seq: 1,
            toast_seq: 1,
            queue_seq: 1,
            lyrics_seq: 0,
            play_seq: 0,
            auto_skips: 0,
            toast_task: None,
            top_search: None,
            top_search_sub: None,
            top_search_focused: false,
            personal_fm_mark: None,
            is_personal_fm_active: false,
            is_fetching_personal_fm: false,
            is_starting_personal_fm: false,
            awaiting_personal_fm: false,
            created_playlists_expanded: true,
            collected_playlists_expanded: true,
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
        let search = cx.new(|cx| SearchView::new(backend.clone(), shell.clone(), player.clone(), window, cx));
        let settings = cx.new(|cx| SettingsView::new(backend.clone(), window, cx));
        let login = cx.new(|cx| LoginView::new(backend.clone(), window, cx));
        let lyrics = cx.new(|cx| LyricsView::new(backend, shell.clone(), player, cx));

        // 顶部全局可直接输入的搜索框
        let top_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索音乐、歌手、专辑…"));
        let search_for_sub = search.clone();
        // 订阅回调拿到的 `this` 就是处于更新中的 shell：这里再调一次
        // `shell.update(..)` 会被判定为重入，直接 panic 导致进程退出。
        let top_search_sub = cx.subscribe(&top_search, move |this, state, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } => {
                let kw = state.read(cx).value().to_string();
                if !kw.trim().is_empty() {
                    this.top_search_focused = false;
                    this.set_nav(Nav::Search, cx);
                    search_for_sub.update(cx, |view, cx| view.execute_search(&kw, cx));
                }
            }
            InputEvent::Focus => {
                this.top_search_focused = true;
                cx.notify();
            }
            InputEvent::Blur => {
                this.top_search_focused = false;
                cx.notify();
            }
            InputEvent::Change => {
                cx.notify();
            }
        });
        self.top_search = Some(top_search);
        self.top_search_sub = Some(top_search_sub);

        self.views = Some(Views {
            player_bar,
            playlists: playlists.clone(),
            daily,
            search,
            settings,
            login,
            lyrics,
        });
        self.ensure_loaded(cx);
        playlists.update(cx, PlaylistsView::ensure);
        // 启动即拉取一次真实头像（昵称与头像以服务端为准）
        self.request_profile();
    }

    /// 拉取登录用户资料（昵称与真实头像）
    fn request_profile(&self) {
        if !self.logged_in {
            return;
        }
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Profile(app.profile().await.map_err(|error| error.to_string())) });
    }

    /// 分发处理后台异步事件
    fn apply(&mut self, msg: Msg, cx: &mut Context<Self>) {
        let (playlists, daily, search, settings, login, lyrics) = {
            let Some(views) = self.views.as_ref() else {
                return;
            };
            (
                views.playlists.clone(),
                views.daily.clone(),
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
            Msg::PersonalFm(result) => self.on_personal_fm(result, cx),
            Msg::Search(seq, result) => search.update(cx, |view, cx| view.on_search(seq, result, cx)),
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
                }
                Err(error) => {
                    tracing::warn!(%error, "移除歌曲失败");
                    self.set_toast(friendly_error_message(&error), cx);
                }
            },
            // 播放请求：过期的响应必须丢弃，否则旧歌会抢在新歌之前起播
            Msg::PlayReady {
                seq,
                hash,
                track,
                preview_ms,
            } => {
                if seq != self.play_seq {
                    return;
                }
                // 曲目必须仍是当前曲目才起播
                let is_current = self
                    .player
                    .read(cx)
                    .current_song()
                    .is_some_and(|song| song.hash == hash);
                if !is_current {
                    return;
                }
                if let Some(engine) = self.backend.engine() {
                    engine.play(track);
                }
                // 上游仅授权试听片段
                if preview_ms > 0 {
                    self.set_toast(format!("仅试听 {} 秒（完整版需会员或购买）", preview_ms / 1000), cx);
                }
            }
            Msg::PlayFailed { seq, hash, message } => {
                if seq != self.play_seq {
                    return;
                }
                self.on_play_failed(&hash, &message, cx);
            }
            Msg::AudioEnded => {
                // 正常播放结束，重置连续跳过计数
                self.auto_skips = 0;
                self.play_next(cx);
            }
            // 播放失败处理：提示并尝试跳过
            Msg::EngineFailed(message) => self.on_engine_failed(&message, cx),
            // 当前登录用户资料（昵称与真实头像）
            Msg::Profile(result) => match result {
                Ok(profile) => {
                    if !profile.nickname.is_empty() {
                        self.nickname = Some(profile.nickname);
                    }
                    self.avatar = Some(SharedString::from(profile.avatar)).filter(|url| !url.is_empty());
                    cx.notify();
                }
                Err(error) => tracing::warn!(%error, "获取用户资料失败"),
            },
            Msg::Noop => {}
            Msg::Toast(text) => self.set_toast(text, cx),
            Msg::LoggedOut => self.on_logged_out(cx),
            Msg::RemoveTrack { listid, fileid } => self.remove_track(&listid, fileid),
        }
    }

    /// 切换导航页面并记录历史栈
    pub(crate) fn set_nav(&mut self, nav: Nav, cx: &mut Context<Self>) {
        self.closing_lyrics = false;
        self.lyrics_close_task = None;
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
                shell.lyrics_close_task = None;
                while shell.history_idx > 0 && shell.history[shell.history_idx] == Nav::Lyrics {
                    shell.history_idx -= 1;
                }
                if shell.history[shell.history_idx] == Nav::Lyrics {
                    shell.nav = Nav::Playlists;
                } else {
                    shell.nav = shell.history[shell.history_idx];
                }
                shell.ensure_loaded(cx);
                cx.notify();
            })
            .ok();
        });
        self.lyrics_close_task = Some(task);
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
            Nav::Settings => views.settings.update(cx, SettingsView::ensure),
            Nav::Login | Nav::Search | Nav::Lyrics => {}
        }
    }

    /// 重新加载当前页面的数据
    #[allow(dead_code)]
    pub(crate) fn reload_current(&mut self, cx: &mut Context<Self>) {
        let Some(views) = self.views.as_ref() else {
            return;
        };
        match self.nav {
            Nav::Playlists => views.playlists.update(cx, PlaylistsView::reload),
            Nav::Daily => views.daily.update(cx, DailyView::reload),
            Nav::Settings => views.settings.update(cx, SettingsView::reload),
            Nav::Login | Nav::Search | Nav::Lyrics => {}
        }
    }

    /// 设置播放队列并播放指定索引歌曲
    pub(crate) fn play_queue(&mut self, songs: Arc<Vec<Song>>, index: usize, cx: &mut Context<Self>) {
        if index >= songs.len() {
            return;
        }
        self.is_personal_fm_active = false;
        self.is_starting_personal_fm = false;
        self.awaiting_personal_fm = false;
        self.auto_skips = 0;
        self.player.update(cx, |model, model_cx| {
            model.set_queue(songs, index);
            model_cx.notify();
        });
        self.play_index(cx);
        self.load_lyrics(cx);
    }

    /// 在现有队列内跳转播放
    ///
    /// 队列不变，所以不退出红心电台（否则随手点一首队列里的歌
    /// 就会把无限流变成有限队列，配上列表循环就是“几首歌来回转”）。
    pub(crate) fn play_current_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.player.read(cx).queue.len() {
            return;
        }
        self.auto_skips = 0;
        self.awaiting_personal_fm = false;
        self.player.update(cx, |model, model_cx| {
            model.set_index(index);
            model_cx.notify();
        });
        self.play_index(cx);
        self.load_lyrics(cx);
        self.check_fm_prefetch(cx);
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
        // 携带歌曲哈希与请求序号：过期的取址结果必须被丢弃，
        // 否则先发后到的旧请求会抢在新歌之前起播。
        let hash = song.hash.clone();
        self.play_seq += 1;
        let seq = self.play_seq;
        self.backend.spawn(async move {
            match app.prepare_play(&song, &quality).await {
                Ok((track, playback)) => Msg::PlayReady {
                    seq,
                    hash,
                    track,
                    preview_ms: playback.preview_ms,
                },
                Err(error) => Msg::PlayFailed {
                    seq,
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
        tracing::warn!(%hash, %message, "曲目取址播放失败");
        if should_skip_unplayable(self.auto_skips) {
            // 静默跳过下一首，避免连续自动跳歌时反复弹出 Toast 打扰用户
            self.skip_unplayable(cx);
        } else {
            self.set_toast(format!("播放失败: {}", friendly_error_message(message)), cx);
        }
    }

    /// 处理音频引擎拉流或解码失败事件
    fn on_engine_failed(&mut self, message: &str, cx: &mut Context<Self>) {
        tracing::warn!(%message, "音频引擎解码失败");
        if should_skip_unplayable(self.auto_skips) {
            self.skip_unplayable(cx);
        } else {
            self.set_toast(format!("播放失败: {}", friendly_error_message(message)), cx);
        }
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

    /// 播放下一首歌曲（遵循当前播放模式；红心电台强制顺序）
    pub(crate) fn play_next(&mut self, cx: &mut Context<Self>) {
        let next_idx = if self.is_personal_fm_active {
            // 电台是无限流：播放模式对它无效，只按顺序前进且不回绕队首
            self.player.read(cx).sequential_next()
        } else {
            self.player.read(cx).next_index()
        };
        if let Some(idx) = next_idx {
            self.player.update(cx, |model, model_cx| {
                model.set_index(idx);
                model_cx.notify();
            });
            self.play_index(cx);
            self.load_lyrics(cx);
            self.check_fm_prefetch(cx);
        } else if self.is_personal_fm_active {
            // 队尾：拉下一批，数据到位后由 on_personal_fm 自动续播
            self.awaiting_personal_fm = true;
            self.prefetch_personal_fm(cx);
        }
    }

    /// 开启红心 Radio（无限流漫游电台）
    pub(crate) fn start_personal_fm(&mut self, cx: &mut Context<Self>) {
        // 每次都从干净的一页开始，避免上一轮残留的队列把 remain/index 算错
        self.is_personal_fm_active = true;
        self.is_starting_personal_fm = true;
        self.awaiting_personal_fm = false;
        self.personal_fm_mark = None;
        self.prefetch_personal_fm(cx);
    }

    /// 拉取红心电台歌曲
    ///
    /// 首启不带当前曲目；续拉带上本地队列最后一首与剩余首数，
    /// 上游靠这两个字段才能接着往下推荐。
    fn prefetch_personal_fm(&mut self, cx: &mut Context<Self>) {
        if self.is_fetching_personal_fm {
            return;
        }
        self.is_fetching_personal_fm = true;
        let (previous, remain) = if self.is_starting_personal_fm {
            (None, 0)
        } else {
            let player = self.player.read(cx);
            let remain = player.queue.len().saturating_sub(player.index + 1);
            (player.queue.last().cloned(), remain as u32)
        };
        let mark = self.personal_fm_mark.clone();
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::PersonalFm(
                app.personal_fm(previous.as_ref(), remain, mark.as_deref())
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }

    /// 处理红心电台返回数据
    fn on_personal_fm(&mut self, result: Result<Page<Song>, String>, cx: &mut Context<Self>) {
        self.is_fetching_personal_fm = false;
        match result {
            Ok(page) => {
                self.personal_fm_mark = page.cursor;
                let songs = page.items;
                if self.is_starting_personal_fm {
                    self.is_starting_personal_fm = false;
                    if songs.is_empty() {
                        self.is_personal_fm_active = false;
                        if self.player.read(cx).queue.is_empty() {
                            self.set_toast("未获取到推荐电台歌曲，请稍后重试".to_owned(), cx);
                        }
                        return;
                    }
                    self.auto_skips = 0;
                    self.player.update(cx, |model, model_cx| {
                        model.set_queue(Arc::new(songs), 0);
                        model_cx.notify();
                    });
                    self.play_index(cx);
                    self.load_lyrics(cx);
                    self.check_fm_prefetch(cx);
                    return;
                }
                // 用户已切到别的队列：丢弃迟到的预取结果，别污染当前播放
                if !self.is_personal_fm_active {
                    return;
                }
                let first_new = self.player.update(cx, |player, model_cx| {
                    let first = player.append_songs(songs);
                    model_cx.notify();
                    first
                });
                // 队尾空窗期拿到新歌：立即续播，而不是停在原地等下一次切歌
                if self.awaiting_personal_fm {
                    if let Some(idx) = first_new {
                        self.awaiting_personal_fm = false;
                        self.player.update(cx, |model, model_cx| {
                            model.set_index(idx);
                            model_cx.notify();
                        });
                        self.play_index(cx);
                        self.load_lyrics(cx);
                        self.check_fm_prefetch(cx);
                    } else {
                        // 整页都是队里已有的歌：不装作在播，给个明确提示
                        self.awaiting_personal_fm = false;
                        self.set_toast("红心电台暂时没有新歌，请稍后重试".to_owned(), cx);
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "获取红心电台失败");
                if self.is_starting_personal_fm {
                    self.is_starting_personal_fm = false;
                    self.is_personal_fm_active = false;
                }
                self.set_toast(friendly_error_message(&error), cx);
            }
        }
    }

    /// 检查是否需要静默预取下一批红心电台歌曲
    fn check_fm_prefetch(&mut self, cx: &mut Context<Self>) {
        if !self.is_personal_fm_active || self.is_starting_personal_fm || self.is_fetching_personal_fm {
            return;
        }
        let player = self.player.read(cx);
        let current = player.index;
        let total = player.queue.len();
        // 当距离队尾还剩 2 首或更少时，在后台静默拉取下一批
        if total.saturating_sub(current) <= 2 {
            self.prefetch_personal_fm(cx);
        }
    }

    /// 切换主播放/暂停（响应全局 Space 快捷键）
    pub(crate) fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if let Some(engine) = self.backend.engine() {
            engine.toggle();
            cx.notify();
        }
    }

    /// 播放上一首歌曲（遵循当前播放模式；红心电台强制顺序）
    pub(crate) fn play_prev(&mut self, cx: &mut Context<Self>) {
        self.auto_skips = 0;
        self.awaiting_personal_fm = false;
        let prev_idx = if self.is_personal_fm_active {
            self.player.read(cx).sequential_prev()
        } else {
            self.player.read(cx).prev_index()
        };
        if let Some(idx) = prev_idx {
            self.player.update(cx, |model, model_cx| {
                model.set_index(idx);
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
        self.nickname = read_nickname(&self.backend.app());
        self.avatar = None;
        self.request_profile();
        if let Some(views) = self.views.as_ref() {
            views.login.update(cx, LoginView::reset);
            views.playlists.update(cx, PlaylistsView::reload);
            views.daily.update(cx, DailyView::reload);
            views.settings.update(cx, SettingsView::ensure);
        }
        // 必须走 set_nav：否则历史栈与当前页不同步，后退/前进会失效
        self.set_nav(Nav::Playlists, cx);
        cx.notify();
    }

    /// 处理退出登录事件
    fn on_logged_out(&mut self, cx: &mut Context<Self>) {
        self.logged_in = false;
        self.nickname = None;
        self.avatar = None;
        // 清除旧账号视图状态
        if let Some(views) = self.views.as_ref() {
            views.playlists.update(cx, PlaylistsView::reset);
            views.daily.update(cx, DailyView::reset);
            views.lyrics.update(cx, LyricsView::clear);
            views.search.update(cx, SearchView::reset);
        }
        self.set_nav(Nav::Login, cx);
    }

    /// 展示全局轻提示
    fn set_toast(&mut self, text: String, cx: &mut Context<Self>) {
        self.toast_seq += 1;
        self.toast = Some(SharedString::from(text));
        cx.notify();
        // 到期后再收一次提示：一次性计时任务，重复提示时会自动取消上一个
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_TTL).await;
            let _ = this.update(cx, |shell, cx| {
                shell.toast = None;
                shell.toast_task = None;
                cx.notify();
            });
        }));
    }

    /// 手动关闭全局轻提示弹窗
    pub(crate) fn dismiss_toast(&mut self, cx: &mut Context<Self>) {
        if self.toast.is_some() {
            self.toast = None;
            self.toast_task = None;
            cx.notify();
        }
    }

    /// 渲染主内容区顶栏（含沉浸式窗口拖拽与 Windows 11 风格窗口控制按钮）
    fn top_header(&self, window: &mut Window, cx: &Context<Self>) -> impl IntoElement {
        let is_search = self.nav == Nav::Search;
        let can_go_back = self.history_idx > 0;
        let can_go_forward = self.history_idx + 1 < self.history.len();
        let is_maximized = window.is_maximized();

        h_flex()
            .w_full()
            .h(px(48.0))
            .pl_4()
            .items_center()
            .border_b_1()
            .border_color(CARD_BORDER)
            // 不能把整条顶栏标记为拖拽区：窗口控件的命中区会覆盖子元素，
            // 标题栏区域一旦被标成 HTCAPTION，里面的搜索框就永远收不到点击。
            // 可拖拽范围只保留中间的空白填充块（见下方 flex_1 占位）。
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
            // 顶部全局搜索框（不挂热门搜索下拉浮层）
            .child({
                div()
                    .w(px(260.0))
                    .h(px(34.0))
                    .ml_3()
                    .when_some(self.top_search.as_ref(), |this, search_state| {
                        this.child(
                            Input::new(search_state)
                                .appearance(false)
                                .w_full()
                                .h_full()
                                .px_3()
                                .rounded(px(999.0))
                                .bg(if is_search { ACCENT_GREEN_LIGHT } else { hex_rgb(0xf2f4f7) })
                                .border_1()
                                .border_color(if is_search { ACCENT_GREEN_BORDER } else { hex_rgb(0xe5e7eb) })
                                .prefix(
                                    Icon::new(IconName::Search)
                                        .size(px(14.0))
                                        .text_color(if is_search { ACCENT_GREEN } else { TEXT_FAINT }),
                                )
                                .cleanable(true),
                        )
                    })
            })
            // 窗口可拖动中间区域（原生支持双击缩放/最大化）
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            // Windows 11 原生贴合无边框窗口控制按钮
            .child(
                h_flex()
                    .h_full()
                    .items_center()
                    .ml_1()
                    // 最小化
                    .child(
                        div()
                            .id("win_min")
                            .w(px(44.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(TEXT_MUTED)
                            .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
                            .window_control_area(WindowControlArea::Min)
                            .on_click(|_event, window, _cx| {
                                window.minimize_window();
                            })
                            .child(Icon::new(IconName::WindowMinimize).size(px(12.0))),
                    )
                    // 最大化 / 还原
                    .child(
                        div()
                            .id("win_max")
                            .w(px(44.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(TEXT_MUTED)
                            .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
                            .window_control_area(WindowControlArea::Max)
                            .on_click(|_event, window, _cx| {
                                window.zoom_window();
                            })
                            .child(
                                Icon::new(if is_maximized {
                                    IconName::WindowRestore
                                } else {
                                    IconName::WindowMaximize
                                })
                                .size(px(12.0)),
                            ),
                    )
                    // 关闭
                    .child(
                        div()
                            .id("win_close")
                            .w(px(44.0))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(TEXT_MUTED)
                            .hover(|s| s.bg(hex_rgb(0xe81123)).text_color(rgb(0xffffff)))
                            .window_control_area(WindowControlArea::Close)
                            .on_click(|_event, window, _cx| {
                                window.remove_window();
                            })
                            .child(Icon::new(IconName::WindowClose).size(px(12.0))),
                    ),
            )
    }

    /// 渲染左侧导航侧边栏
    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut column = v_flex().id("sidebar").w(px(204.0)).h_full().p_2().gap(px(8.0));

        // 顶部品牌区（兼具窗口拖拽区，提升无边框体验）
        column = column.child(
            div()
                .w_full()
                .window_control_area(WindowControlArea::Drag)
                .child(crate::logo::lyra_brand_header()),
        );

        // 标准纵向主导航菜单（清晰的垂直排布，替代拥挤的横向胶囊）
        let nav_items = [(Nav::Daily, IconName::Compass, "发现")];

        let mut nav_column = v_flex().w_full().gap(px(2.0));
        for (idx, (nav, icon, label)) in nav_items.into_iter().enumerate() {
            let active = self.nav == nav;
            let btn = div()
                .id(("side_nav", idx))
                .role(Role::Button)
                .cursor_pointer()
                .w_full()
                .h(px(32.0))
                .px_2()
                .rounded(px(6.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .when(active, |this| {
                    this.bg(CARD_BG)
                        .text_color(TEXT_PRIMARY)
                        .font_weight(FontWeight::BOLD)
                        .shadow_xs()
                })
                .when(!active, |this| {
                    this.text_color(TEXT_MUTED)
                        .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
                })
                .on_click(cx.listener(move |shell, _event, _window, cx| shell.set_nav(nav, cx)))
                .child(
                    Icon::new(icon)
                        .size(px(15.0))
                        .text_color(if active { ACCENT_GREEN } else { TEXT_MUTED }),
                )
                .child(div().text_xs().child(label));

            nav_column = nav_column.child(btn);
        }
        column = column.child(nav_column);

        // 歌单分类与列表区（折叠面板 Accordion）
        let (created_entries, collected_entries, selected, loading_playlists) = if let Some(views) = self.views.as_ref()
        {
            let p = views.playlists.read(cx);
            let mut created = Vec::new();
            let mut collected = Vec::new();
            for (index, list) in p.playlists.iter().enumerate() {
                let entry = PlaylistEntry {
                    index,
                    name: SharedString::from(list.name.as_str()),
                    count: SharedString::from(list.count.to_string()),
                    liked: list.is_liked(),
                };
                if list.is_liked() || list.kind != 1 {
                    created.push(entry);
                } else {
                    collected.push(entry);
                }
            }
            (created, collected, p.selected, p.loading_playlists)
        } else {
            (Vec::new(), Vec::new(), None, false)
        };

        // 歌单区域标题与轻量“+”新建图标
        let playlist_section_header = h_flex()
            .w_full()
            .pt_3()
            .pb_1()
            .px_2()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(TEXT_FAINT)
                    .child("我的歌单"),
            )
            .child(
                Button::new("sidebar_new_playlist")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("新建歌单")
                    .on_click(cx.listener(|shell, _event, _window, cx| {
                        shell.set_toast("新建歌单功能开发中".to_owned(), cx);
                    })),
            );
        column = column.child(playlist_section_header);

        macro_rules! render_entry {
            ($entry:expr, $tab:expr) => {{
                let index = $entry.index;
                let active = self.nav == Nav::Playlists && selected == Some(index);
                let name = $entry.name.clone();
                let is_liked = $entry.liked;
                let count = $entry.count.clone();

                div()
                    .id(("sidebar_playlist", index))
                    .role(Role::Button)
                    .w_full()
                    .px_2()
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(active, |this| {
                        this.bg(hex_rgb(0xf3f4f6))
                            .text_color(ACCENT_GREEN)
                            .font_weight(FontWeight::BOLD)
                    })
                    .when(!active, |this| {
                        this.text_color(TEXT_MUTED)
                            .hover(|style| style.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
                    })
                    .on_click(cx.listener(move |shell, _event, _window, cx| {
                        if let Some(views) = shell.views.as_ref() {
                            views.playlists.update(cx, |p, cx| {
                                p.tab = $tab;
                                p.select(index, cx);
                            });
                        }
                        shell.set_nav(Nav::Playlists, cx);
                        cx.notify();
                    }))
                    .child(if is_liked {
                        div()
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(5.0))
                            .bg(if active {
                                ACCENT_GREEN_LIGHT
                            } else {
                                hex_rgb(0xf3f4f6)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Heart).size(px(13.0)).text_color(if active {
                                ACCENT_GREEN
                            } else {
                                hex_rgb(0xef4444)
                            }))
                            .into_any_element()
                    } else {
                        cover_box(&name, 26.0).into_any_element()
                    })
                    .child(div().flex_1().min_w(px(0.0)).text_xs().truncate().child(name))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hex_rgb(0x8e8e93))
                            .child(count),
                    )
            }};
        }

        let mut playlist_tree = v_flex().w_full().gap(px(4.0));

        if loading_playlists && created_entries.is_empty() && collected_entries.is_empty() {
            playlist_tree = playlist_tree.child(notice_with_icon(IconName::RefreshCw, "加载歌单…"));
        } else {
            // 分组一：创建的歌单 Accordion
            let created_count = created_entries.len();
            let is_created_open = self.created_playlists_expanded;
            let created_header = h_flex()
                .id("accordion_created_header")
                .w_full()
                .py(px(4.0))
                .px_2()
                .rounded(px(5.0))
                .items_center()
                .justify_between()
                .cursor_pointer()
                .hover(|h| h.bg(SURFACE_HOVER))
                .on_click(cx.listener(|shell, _event, _window, cx| {
                    shell.created_playlists_expanded = !shell.created_playlists_expanded;
                    cx.notify();
                }))
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            Icon::new(if is_created_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(12.0))
                            .text_color(TEXT_FAINT),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT_MUTED)
                                .child("创建的歌单"),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(TEXT_FAINT)
                        .child(format!("{created_count}")),
                );

            playlist_tree = playlist_tree.child(created_header);

            if is_created_open {
                let mut created_list = v_flex().w_full().pl(px(16.0)).gap(px(2.0));
                if created_entries.is_empty() && !loading_playlists {
                    created_list = created_list.child(
                        div()
                            .py_1()
                            .px_2()
                            .text_size(px(11.0))
                            .text_color(TEXT_FAINT)
                            .child("暂无创建歌单"),
                    );
                } else {
                    for entry in &created_entries {
                        created_list = created_list.child(render_entry!(entry, PlaylistTab::Created));
                    }
                }
                playlist_tree = playlist_tree.child(created_list);
            }

            // 分组二：收藏的歌单 Accordion
            let collected_count = collected_entries.len();
            let is_collected_open = self.collected_playlists_expanded;
            let collected_header = h_flex()
                .id("accordion_collected_header")
                .w_full()
                .mt_1()
                .py(px(4.0))
                .px_2()
                .rounded(px(5.0))
                .items_center()
                .justify_between()
                .cursor_pointer()
                .hover(|h| h.bg(SURFACE_HOVER))
                .on_click(cx.listener(|shell, _event, _window, cx| {
                    shell.collected_playlists_expanded = !shell.collected_playlists_expanded;
                    cx.notify();
                }))
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            Icon::new(if is_collected_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(12.0))
                            .text_color(TEXT_FAINT),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT_MUTED)
                                .child("收藏的歌单"),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(TEXT_FAINT)
                        .child(format!("{collected_count}")),
                );

            playlist_tree = playlist_tree.child(collected_header);

            if is_collected_open {
                let mut collected_list = v_flex().w_full().pl(px(16.0)).gap(px(2.0));
                if collected_entries.is_empty() && !loading_playlists {
                    collected_list = collected_list.child(
                        div()
                            .py_1()
                            .px_2()
                            .text_size(px(11.0))
                            .text_color(TEXT_FAINT)
                            .child("暂无收藏歌单"),
                    );
                } else {
                    for entry in &collected_entries {
                        collected_list = collected_list.child(render_entry!(entry, PlaylistTab::Collected));
                    }
                }
                playlist_tree = playlist_tree.child(collected_list);
            }
        }

        let playlist_scroll = div()
            .id("sidebar-playlists-scroll")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scrollbar()
            .child(playlist_tree);

        column = column.child(playlist_scroll);

        // 底部用户胶囊与设置区（固定于侧边栏底部）
        let username = self.nickname.clone().unwrap_or_else(|| {
            if self.logged_in {
                "已登录".to_owned()
            } else {
                "点击登录".to_owned()
            }
        });

        let avatar_element = match self.avatar.clone() {
            Some(url) => {
                let logged_in = self.logged_in;
                img(url)
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || avatar_placeholder(logged_in))
                    .into_any_element()
            }
            None => avatar_placeholder(self.logged_in),
        };

        let user_capsule = div()
            .id("sidebar_user_capsule")
            .role(Role::Button)
            .cursor_pointer()
            .flex_1()
            .min_w(px(0.0))
            .h(px(32.0))
            .flex()
            .items_center()
            .px_2()
            .rounded(px(6.0))
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
                    .w_full()
                    .gap(px(8.0))
                    .items_center()
                    .child(avatar_element)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(TEXT_PRIMARY)
                            .truncate()
                            .child(username),
                    ),
            );

        let is_settings = self.nav == Nav::Settings;
        let settings_btn = div()
            .id("sidebar_settings_btn")
            .role(Role::Button)
            .cursor_pointer()
            .w(px(32.0))
            .h(px(32.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .when(is_settings, |s| s.bg(CARD_BG).shadow_xs())
            .when(!is_settings, |s| s.hover(|h| h.bg(SURFACE_HOVER)))
            .on_click(cx.listener(|shell, _event, _window, cx| shell.set_nav(Nav::Settings, cx)))
            .child(Icon::new(IconName::Settings).size(px(15.0)).text_color(if is_settings {
                ACCENT_GREEN
            } else {
                TEXT_MUTED
            }));

        let bottom_bar = h_flex()
            .w_full()
            .pt_2()
            .border_t_1()
            .border_color(hex_rgb(0xe5e7eb))
            .items_center()
            .justify_between()
            .gap(px(4.0))
            .child(user_capsule)
            .child(settings_btn);

        column = column.child(bottom_bar);
        column
    }

    /// 渲染顶部浮动提示弹窗（遵循设计系统 Level 1 质感，具备语义化图标与轻量关闭交互）
    fn toast_element(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let text = self.toast.clone()?;
        let toast_id = ("top_toast_entrance", self.toast_seq);
        let shell = cx.entity().downgrade();

        let is_error = text.contains("失败")
            || text.contains("错误")
            || text.contains("未能")
            || text.contains("异常")
            || text.contains("不稳定")
            || text.contains("超时");

        let is_success = text.contains("成功")
            || text.contains("已加入")
            || text.contains("已收藏")
            || text.contains("已保存")
            || text.contains("已清除")
            || text.contains("已从");

        let (icon_name, icon_col, icon_bg) = if is_error {
            (IconName::CircleAlert, hex_rgb(0xef4444), hex_rgb(0xfef2f2))
        } else if is_success {
            (IconName::Check, ACCENT_GREEN, ACCENT_GREEN_LIGHT)
        } else {
            (IconName::Sparkles, ACCENT_GREEN, ACCENT_GREEN_LIGHT)
        };

        let content = h_flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(10.0))
            .pr(px(8.0))
            .py(px(6.0))
            .max_w(px(520.0))
            .rounded(px(999.0))
            .bg(CARD_BG)
            .border_1()
            .border_color(CARD_BORDER)
            .shadow_lg()
            // 语义化状态图标指示器（带柔和圆形背景色）
            .child(
                div()
                    .w(px(22.0))
                    .h(px(22.0))
                    .rounded_full()
                    .bg(icon_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon_name).size(px(13.0)).text_color(icon_col)),
            )
            // 提示文案内容
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT_PRIMARY)
                    .truncate()
                    .child(text),
            )
            // 快速手动关闭按钮
            .child(
                div()
                    .id("toast_close_btn")
                    .role(Role::Button)
                    .cursor_pointer()
                    .w(px(18.0))
                    .h(px(18.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(TEXT_FAINT)
                    .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
                    .on_click(move |_event, _window, cx| {
                        shell.update(cx, AppShell::dismiss_toast).ok();
                    })
                    .child(Icon::new(IconName::X).size(px(11.0))),
            );

        Some(
            crate::motion::toast_entrance(
                div()
                    .absolute()
                    .top(px(14.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(content),
                toast_id,
            )
            .into_any_element(),
        )
    }

    /// 渲染右侧播放队列抽屉
    fn queue_drawer(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.show_queue {
            return None;
        }

        let (queue, rows, current_idx, is_playing, mode) = {
            let model = self.player.read(cx);
            (
                model.queue.clone(),
                model.rows.clone(),
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
                    .child(badge(
                        if self.is_personal_fm_active {
                            "红心电台 · 顺序"
                        } else {
                            mode_label(mode)
                        },
                        CANVAS_BG,
                        TEXT_MUTED,
                    ))
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
            panel = panel.child(notice_with_icon(IconName::ListMusic, "队列暂无歌曲"));
        } else {
            let mut list = v_flex()
                .id("queue_tracks_scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .gap(px(4.0));
            for idx in 0..queue.len() {
                let is_current = idx == current_idx;
                let shell = cx.entity().downgrade();
                // 展示文本已在队列入队时预格式化：渲染期不再拼字符串
                let row = rows.get(idx);

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
                        .when(is_current, |this| this.bg(SURFACE_HOVER))
                        .when(!is_current, |this| this.hover(|s| s.bg(SURFACE_HOVER)))
                        .on_click(move |_event, _window, cx| {
                            shell.update(cx, |shell, cx| shell.play_current_index(idx, cx)).ok();
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
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(TEXT_FAINT)
                                            .child(crate::format::index_label(idx)),
                                    )
                                }),
                        )
                        .child(song_cover(
                            row.map_or("", |r| r.cover.as_ref()),
                            row.map_or("", |r| r.title.as_ref()),
                            28.0,
                        ))
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
                                        .text_color(TEXT_PRIMARY)
                                        .truncate()
                                        .child(
                                            row.map_or_else(|| SharedString::new_static(""), |row| row.title.clone()),
                                        ),
                                )
                                .child(
                                    div().text_xs().text_color(TEXT_MUTED).truncate().child(
                                        row.map_or_else(|| SharedString::new_static(""), |row| row.singer.clone()),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(TEXT_FAINT)
                                .child(row.map_or_else(|| SharedString::new_static(""), |row| row.duration.clone())),
                        ),
                );
            }
            panel = panel.child(list);
        }

        Some(crate::motion::drawer_entrance(panel, ("queue_drawer_panel", self.queue_seq)).into_any_element())
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(views) = self.views.as_ref() else {
            return notice("初始化中…");
        };
        // 全屏沉浸式歌词视图模式（非退出过渡期）
        if self.nav == Nav::Lyrics && !self.closing_lyrics {
            let lyrics_card = crate::motion::lyrics_open_entrance(
                main_card().size_full().overflow_hidden().child(views.lyrics.clone()),
                ("lyrics_fullscreen_card", self.nav_seq),
            );
            return v_flex()
                .relative()
                .size_full()
                .bg(CANVAS_BG)
                .text_color(TEXT_PRIMARY)
                .overflow_hidden()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                    if event.keystroke.key == "space" {
                        this.toggle_play(cx);
                    }
                }))
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .p(px(8.0))
                        .child(lyrics_card)
                        .children(self.queue_drawer(cx)),
                )
                .child(views.player_bar.clone())
                .children(self.toast_element(cx))
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
            .relative()
            .size_full()
            .bg(CANVAS_BG)
            .text_color(TEXT_PRIMARY)
            .overflow_hidden()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if !this.top_search_focused && event.keystroke.key == "space" {
                    this.toggle_play(cx);
                }
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .p(px(8.0))
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
                            .child(self.top_header(window, cx))
                            .child(div().flex_1().min_h(px(0.0)).child(crate::motion::page_entrance(
                                div().relative().size_full().child(body),
                                ("main_route_page", self.nav_seq),
                            ))),
                    )
                    .children(lyrics_exit_overlay)
                    .children(self.queue_drawer(cx)),
            )
            .child(views.player_bar.clone())
            .children(self.toast_element(cx))
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
