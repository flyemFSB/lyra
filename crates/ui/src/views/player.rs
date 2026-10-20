//! 底部播放控制条与共享播放模型。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::Song;
use lyra_audio::{Snapshot, State};

use crate::bridge::{Backend, Msg, Nav};
use crate::format::{format_duration, progress_percent};
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CARD_BG, CARD_BORDER, TEXT_MUTED, TEXT_PRIMARY, badge,
    cover_box,
};

pub(crate) use lyra_app::PlayMode;

/// 播放模式 UI 展示与切换扩展
pub(crate) trait PlayModeUi {
    /// 显示文本标签
    fn label(self) -> &'static str;
    /// 对应图标
    fn icon(self) -> IconName;
    /// 循环切换至下一模式
    fn next(self) -> Self;
}

impl PlayModeUi for PlayMode {
    fn label(self) -> &'static str {
        match self {
            Self::ListLoop => "列表循环",
            Self::SingleLoop => "单曲循环",
            Self::Shuffle => "随机播放",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::ListLoop => IconName::Repeat,
            Self::SingleLoop => IconName::Repeat1,
            Self::Shuffle => IconName::Shuffle,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::ListLoop => Self::SingleLoop,
            Self::SingleLoop => Self::Shuffle,
            Self::Shuffle => Self::ListLoop,
        }
    }
}

/// 播放状态共享数据模型
pub(crate) struct PlayerModel {
    /// 音频引擎状态快照
    pub(crate) snapshot: Snapshot,
    /// 是否存在可用音频输出设备
    pub(crate) has_engine: bool,
    /// 当前播放曲目队列
    pub(crate) queue: Arc<Vec<Song>>,
    /// 当前播放曲目在队列中的索引
    pub(crate) index: usize,
    /// 当前播放模式
    pub(crate) mode: PlayMode,
    /// 随机播放用的伪随机状态（xorshift64*）
    ///
    /// 不能用纳秒时间戳：相邻两次调用可能落在同一纳秒，
    /// 结果会重复成同一首歌。
    rng_state: AtomicU64,
}

impl PlayerModel {
    /// 创建初始播放状态
    pub(crate) fn new(has_engine: bool, mode: PlayMode) -> Self {
        Self {
            snapshot: Snapshot {
                state: State::Idle,
                track: None,
                position_ms: 0,
                duration_ms: 0,
                volume: 0.8,
            },
            has_engine,
            queue: Arc::new(Vec::new()),
            index: 0,
            mode,
            // 用当前时间播种一次；之后完全由自身状态推进
            rng_state: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64)
                    | 1,
            ),
        }
    }

    /// 取下一个伪随机数（xorshift64*）
    fn next_random(&self) -> u64 {
        let mut x = self.rng_state.load(Ordering::Relaxed);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng_state.store(x, Ordering::Relaxed);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 循环切换播放模式
    pub(crate) fn cycle_mode(&mut self) -> PlayMode {
        self.mode = self.mode.next();
        self.mode
    }

    /// 获取下一首曲目索引
    pub(crate) fn next_index(&self) -> Option<usize> {
        let len = self.queue.len();
        if len == 0 {
            return None;
        }
        match self.mode {
            PlayMode::SingleLoop => Some(self.index),
            PlayMode::ListLoop => Some((self.index + 1) % len),
            PlayMode::Shuffle => {
                if len == 1 {
                    Some(0)
                } else {
                    // 在除当前曲目之外的范围里随机挑一首
                    let offset = (self.next_random() as usize % (len - 1)) + 1;
                    Some((self.index + offset) % len)
                }
            }
        }
    }

    /// 获取上一首曲目索引
    pub(crate) fn prev_index(&self) -> Option<usize> {
        let len = self.queue.len();
        if len == 0 {
            return None;
        }
        match self.mode {
            PlayMode::SingleLoop => Some(self.index),
            PlayMode::ListLoop => Some((self.index + len - 1) % len),
            PlayMode::Shuffle => {
                if len == 1 {
                    Some(0)
                } else {
                    // 随机播放下的「上一首」必须往回挑，
                    // 否则会跳到后面的歌，与按钮语义相反。
                    let offset = (self.next_random() as usize % (len - 1)) + 1;
                    Some((self.index + len - offset) % len)
                }
            }
        }
    }

    /// 获取当前曲目的哈希（零分配，用于判断曲目是否变化）
    pub(crate) fn current_hash(&self) -> Option<&str> {
        self.current_song().map(|song| song.hash.as_str())
    }

    /// 获取当前曲目标题与歌手（仅在曲目真正变化时调用）
    pub(crate) fn current_labels(&self) -> (Option<String>, SharedString, SharedString) {
        self.current_song().map_or_else(
            || (None, SharedString::new_static("未在播放"), SharedString::new_static("")),
            |song| {
                (
                    Some(song.hash.clone()),
                    SharedString::from(song.display_title()),
                    SharedString::from(song.display_singer()),
                )
            },
        )
    }

    /// 获取当前播放歌曲
    pub(crate) fn current_song(&self) -> Option<&Song> {
        self.queue.get(self.index)
    }

    /// 设置播放队列与当前索引
    pub(crate) fn set_queue(&mut self, queue: Arc<Vec<Song>>, index: usize) {
        self.queue = queue;
        self.index = index;
        self.snapshot.position_ms = 0;
    }

    /// 应用引擎最新快照并返回是否发生变更
    pub(crate) fn apply_snapshot(&mut self, next: Snapshot) -> bool {
        let track_changed = self.snapshot.track.as_ref().map(|t| &t.hash) != next.track.as_ref().map(|t| &t.hash);
        let changed = track_changed
            || self.snapshot.state != next.state
            || self.snapshot.position_ms != next.position_ms
            || self.snapshot.duration_ms != next.duration_ms
            || (self.snapshot.volume - next.volume).abs() > f32::EPSILON;
        if changed {
            self.snapshot = next;
        }
        changed
    }
}

/// 底部播放条视图
pub(crate) struct PlayerBar {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    progress: Entity<SliderState>,
    volume: Entity<SliderState>,
    dragging: bool,
    volume_dragging: bool,
    playing: bool,
    title: SharedString,
    singer: SharedString,
    current_hash: Option<String>,
    /// 时长文本（仅在总时长变化时重建）
    duration_text: SharedString,
    /// 队列数量提示文本（仅在队列长度变化时重建）
    queue_tip: SharedString,
    /// 播放模式提示文本（仅在模式变化时重建）
    mode_tip: SharedString,
    /// 上一次用于生成展示文本的原始值（用于判断是否需要重建）
    last_duration: u64,
    last_queue_len: usize,
    last_mode: PlayMode,
    /// 进度文本复用缓冲区（进度每帧都在变）
    pos_buf: String,
    _subs: Vec<Subscription>,
}

impl PlayerBar {
    /// 创建播放条并启动状态同步任务
    pub(crate) fn new(
        backend: Backend,
        shell: WeakEntity<AppShell>,
        player: Entity<PlayerModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_volume = player.read(cx).snapshot.volume;
        let progress = cx.new(|_| SliderState::new().min(0.0).max(100.0).step(0.5));
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(100.0)
                .step(1.0)
                .default_value(initial_volume * 100.0)
        });

        let progress_sub = cx.subscribe(&progress, |this, _state, event: &SliderEvent, cx| match event {
            SliderEvent::Change(_) => this.dragging = true,
            SliderEvent::Release(value) => {
                this.dragging = false;
                this.seek_percent(value_single(*value), cx);
            }
        });
        let volume_sub = cx.subscribe(&volume, |this, _state, event: &SliderEvent, _cx| {
            let (value, persist) = match event {
                SliderEvent::Change(value) => {
                    this.volume_dragging = true;
                    (*value, false)
                }
                SliderEvent::Release(value) => {
                    this.volume_dragging = false;
                    (*value, true)
                }
            };
            this.apply_volume(value_single(value) / 100.0, persist);
        });

        if let Some(engine) = backend.engine() {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(180)).await;
                    let snapshot = engine.snapshot();
                    let alive = this
                        .update_in(cx, |bar, window, cx| bar.sync(snapshot, window, cx))
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
            })
            .detach();
        }

        Self {
            backend,
            shell,
            player,
            progress,
            volume,
            dragging: false,
            volume_dragging: false,
            playing: false,
            title: SharedString::new_static("未在播放"),
            singer: SharedString::new_static(""),
            current_hash: None,
            duration_text: SharedString::new_static("0:00"),
            queue_tip: SharedString::new_static("播放队列 (0 首)"),
            mode_tip: SharedString::new_static("播放模式"),
            last_duration: 0,
            last_queue_len: 0,
            last_mode: PlayMode::ListLoop,
            pos_buf: String::new(),
            _subs: vec![progress_sub, volume_sub],
        }
    }

    /// 同步音频快照状态
    fn sync(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self.player.update(cx, |model, model_cx| {
            let changed = model.apply_snapshot(snapshot);
            if changed {
                model_cx.notify();
            }
            changed
        });

        // 只有曲目真的变了才重新格式化标题 ——
        // 进度每 180ms 变一次，若每次都重建标题就会持续产生字符串分配。
        let retitled = {
            let model = self.player.read(cx);
            model.current_hash() != self.current_hash.as_deref()
        };
        if retitled {
            let (hash, title, singer) = self.player.read(cx).current_labels();
            self.current_hash = hash;
            self.title = title;
            self.singer = singer;
        }

        let (percent, volume, playing) = {
            let model = self.player.read(cx);
            (
                progress_percent(model.snapshot.position_ms, model.snapshot.duration_ms),
                model.snapshot.volume,
                model.snapshot.state == State::Playing,
            )
        };
        self.playing = playing;

        if !self.dragging {
            self.progress.update(cx, |state, state_cx| {
                if (value_single(state.value()) - percent).abs() > 0.05 {
                    state.set_value(percent, window, state_cx);
                }
            });
        }
        if !self.volume_dragging {
            self.volume.update(cx, |state, state_cx| {
                let current_slider_val = value_single(state.value());
                let target_slider_val = (volume * 100.0).clamp(0.0, 100.0);
                if (current_slider_val - target_slider_val).abs() > 1.0 {
                    state.set_value(target_slider_val, window, state_cx);
                }
            });
        }

        // 派生展示文本在这里统一刷新（每 180ms 一次，且只在内容变化时重建），
        // render 只读缓存，避免每帧 format!
        let (duration, queue_len, mode) = {
            let model = self.player.read(cx);
            (model.snapshot.duration_ms, model.queue.len(), model.mode)
        };
        if duration != self.last_duration {
            self.last_duration = duration;
            self.duration_text = format_duration(duration);
        }
        if queue_len != self.last_queue_len {
            self.last_queue_len = queue_len;
            self.queue_tip = SharedString::from(format!("播放队列 ({queue_len} 首)"));
        }
        if mode != self.last_mode {
            self.last_mode = mode;
            self.mode_tip = SharedString::from(format!("播放模式：{}", mode.label()));
        }

        if changed || retitled {
            cx.notify();
        }
    }

    /// 切换播放与暂停状态
    fn toggle(&self) {
        if let Some(engine) = self.backend.engine() {
            engine.toggle();
        } else {
            self.backend.send(Msg::Toast("没有可用的音频输出设备".to_owned()));
        }
    }

    /// 按进度百分比跳转播放位置
    fn seek_percent(&self, percent: f32, cx: &Context<Self>) {
        let Some(engine) = self.backend.engine() else {
            return;
        };
        let duration = self.player.read(cx).snapshot.duration_ms;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ms = (f64::from(percent.clamp(0.0, 100.0)) / 100.0 * duration as f64).round() as u64;
        engine.seek(ms);
    }

    /// 持久化播放模式
    fn persist_mode(&self, mode: PlayMode) {
        let app = self.backend.app();
        self.backend.spawn(async move {
            match app.settings().and_then(|mut settings| {
                settings.play_mode = mode;
                app.set_settings(&settings)
            }) {
                Ok(()) => Msg::Noop,
                Err(error) => Msg::Toast(format!("保存播放模式失败: {error}")),
            }
        });
    }

    /// 调整音量并在释放时持久化
    fn apply_volume(&self, value: f32, persist: bool) {
        let value = value.clamp(0.0, 1.0);
        if let Some(engine) = self.backend.engine() {
            engine.set_volume(value);
        }
        if persist {
            let app = self.backend.app();
            self.backend.spawn(async move {
                match app.settings().and_then(|mut settings| {
                    settings.volume = value;
                    app.set_settings(&settings)
                }) {
                    Ok(()) => Msg::Noop,
                    Err(error) => Msg::Toast(format!("保存音量失败: {error}")),
                }
            });
        }
    }
}

/// 提取滑块浮点数值
fn value_single(value: SliderValue) -> f32 {
    match value {
        SliderValue::Single(single) => single,
        SliderValue::Range(start, _) => start,
    }
}

impl Render for PlayerBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (position, has_engine, volume, queue, current_index, play_mode) = {
            let model = self.player.read(cx);
            (
                model.snapshot.position_ms,
                model.has_engine,
                model.snapshot.volume,
                // 队列是 Arc：克隆只是引用计数递增。
                // 曲目本体等到点击「收藏」时再克隆，渲染期不做整首拷贝。
                model.queue.clone(),
                model.index,
                model.mode,
            )
        };

        // 进度每帧都在变：复用缓冲区格式化，再转成可廉价克隆的 SharedString
        crate::format::write_duration(&mut self.pos_buf, position);
        let position_text = SharedString::new(self.pos_buf.as_str());

        let play_icon = if self.playing { IconName::Pause } else { IconName::Play };

        let is_muted = volume <= 0.001;
        let vol_icon = if is_muted { IconName::VolumeX } else { IconName::Volume2 };

        let shell_prev = self.shell.clone();
        let shell_next = self.shell.clone();
        let shell_lyrics_cover = self.shell.clone();
        let shell_queue = self.shell.clone();
        let like_backend = self.backend.clone();
        let like_queue = queue.clone();

        h_flex()
            .h(px(76.0))
            .w_full()
            .px_5()
            .gap(px(16.0))
            .bg(CARD_BG)
            .rounded(px(14.0))
            .border_1()
            .border_color(CARD_BORDER)
            .shadow_sm()
            // 封面、曲目信息与收藏按钮
            .child(
                h_flex()
                    .w(px(260.0))
                    .min_w(px(0.0))
                    .gap(px(12.0))
                    .items_center()
                    .child(
                        div()
                            .id("player_cover_link")
                            .role(Role::Button)
                            .cursor_pointer()
                            .on_click(move |_event, _window, cx| {
                                shell_lyrics_cover.update(cx, |shell, cx| {
                                    if shell.nav == Nav::Lyrics {
                                        shell.close_lyrics(cx);
                                    } else {
                                        shell.set_nav(Nav::Lyrics, cx);
                                    }
                                }).ok();
                            })
                            .hover(|s| s.opacity(0.85))
                            .child(cover_box(self.title.as_ref(), 46.0, cx)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(2.0))
                            .child(
                                h_flex()
                                    .gap(px(6.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(TEXT_PRIMARY)
                                            .truncate()
                                            .child(self.title.clone()),
                                    )
                                    .child(badge("SQ", ACCENT_GREEN_LIGHT, ACCENT_GREEN)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(TEXT_MUTED)
                                    .truncate()
                                    .child(self.singer.clone()),
                            ),
                    )
                    .child(
                        Button::new("player_like")
                            .ghost()
                            .small()
                            .icon(IconName::Heart)
                            .tooltip("收藏当前歌曲")
                            .on_click(move |_event, _window, _cx| {
                                let Some(song) = like_queue.get(current_index) else {
                                    return;
                                };
                                let song = song.clone();
                                let app = like_backend.app();
                                like_backend.spawn(async move {
                                    Msg::Toast(match app.like(&song).await {
                                        Ok(()) => "已加入「我喜欢」".to_owned(),
                                        Err(error) => format!("收藏失败: {error}"),
                                    })
                                });
                            }),
                    ),
            )
            // 核心控制区与进度条
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_center()
                    .justify_center()
                    .gap(px(3.0))
                    .child(
                        h_flex()
                            .gap(px(14.0))
                            .items_center()
                            // 切换播放模式
                            .child(
                                Button::new("mode_toggle")
                                    .ghost()
                                    .small()
                                    .icon(play_mode.icon())
                                    .tooltip(self.mode_tip.clone())
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        let next = this.player.update(cx, |model, cx| {
                                            let m = model.cycle_mode();
                                            cx.notify();
                                            m
                                        });
                                        this.persist_mode(next);
                                        this.backend.send(Msg::Toast(format!("已切换为：{}", next.label())));
                                    })),
                            )
                            .child(
                                Button::new("prev")
                                    .ghost()
                                    .small()
                                    .icon(IconName::SkipBack)
                                    .tooltip("上一首")
                                    .on_click(move |_event, _window, cx| {
                                        shell_prev.update(cx, AppShell::play_prev).ok();
                                    }),
                            )
                            // 播放/暂停按钮
                            .child(
                                div()
                                    .id("toggle_play_btn")
                                    .role(Role::Button)
                                    .cursor_pointer()
                                    .w(px(38.0))
                                    .h(px(38.0))
                                    .rounded_full()
                                    .bg(ACCENT_GREEN)
                                    .hover(|s| s.bg(ACCENT_GREEN_HOVER))
                                    .shadow_sm()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_click(cx.listener(|this, _event, _window, _cx| this.toggle()))
                                    .child(Icon::new(play_icon).size(px(20.0)).text_color(rgb(0xffffff))),
                            )
                            .child(
                                Button::new("next")
                                    .ghost()
                                    .small()
                                    .icon(IconName::SkipForward)
                                    .tooltip("下一首")
                                    .on_click(move |_event, _window, cx| {
                                        shell_next.update(cx, AppShell::play_next).ok();
                                    }),
                            ),
                    )
                    // 进度条
                    .child(
                        h_flex()
                            .w_full()
                            .gap(px(10.0))
                            .items_center()
                            .child(
                                div()
                                    .w(px(40.0))
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(TEXT_MUTED)
                                    .text_right()
                                    .child(position_text),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .child(Slider::new(&self.progress).w_full()),
                            )
                            .child(
                                div()
                                    .w(px(40.0))
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(TEXT_MUTED)
                                    .child(self.duration_text.clone()),
                            ),
                    ),
            )
            // 音量与播放队列
            .child(
                h_flex()
                    .w(px(260.0))
                    .justify_end()
                    .items_center()
                    .gap(px(10.0))
                    .when(!has_engine, |this| {
                        this.child(badge("无音频设备", hsla(0.0, 0.85, 0.95, 1.0), hsla(0.0, 0.85, 0.45, 1.0)))
                    })
                    // 音量调节
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                Button::new("vol_toggle")
                                    .ghost()
                                    .small()
                                    .icon(vol_icon)
                                    .tooltip(if is_muted { "恢复音量" } else { "静音" })
                                    .on_click(cx.listener(|this, _event, window, cx| {
                                        let current = this.player.read(cx).snapshot.volume;
                                        let target = if current > 0.001 { 0.0 } else { 0.8 };
                                        this.volume.update(cx, |state, state_cx| {
                                            state.set_value(target * 100.0, window, state_cx);
                                        });
                                        this.apply_volume(target, true);
                                    })),
                            )
                            .child(
                                div()
                                    .w(px(80.0))
                                    .child(Slider::new(&self.volume).w_full()),
                            ),
                    )
                    // 播放队列开关
                    .child(
                        Button::new("queue_drawer_toggle")
                            .ghost()
                            .small()
                            .icon(IconName::ListMusic)
                            .tooltip(self.queue_tip.clone())
                            .on_click(move |_event, _window, cx| {
                                shell_queue.update(cx, AppShell::toggle_queue).ok();
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]

    // 避免与 gpui_kit::* 宏冲突
    use std::sync::Arc;

    use lyra_api::model::Song;

    use super::{PlayMode, PlayerModel};

    /// 构建包含 3 首曲目的测试播放模型
    fn model(mode: PlayMode, index: usize) -> PlayerModel {
        let songs: Vec<Song> = ["a", "b", "c"]
            .iter()
            .map(|hash| Song {
                hash: (*hash).to_owned(),
                ..Song::default()
            })
            .collect();
        let mut model = PlayerModel::new(true, mode);
        model.set_queue(Arc::new(songs), index);
        model
    }

    #[test]
    fn persisted_mode_is_what_the_session_starts_with() {
        // 恢复持久化的播放模式
        assert_eq!(PlayerModel::new(true, PlayMode::Shuffle).mode, PlayMode::Shuffle);
        assert_eq!(PlayerModel::new(true, PlayMode::SingleLoop).mode, PlayMode::SingleLoop);
        assert_eq!(PlayerModel::new(true, PlayMode::ListLoop).mode, PlayMode::ListLoop);
    }

    #[test]
    fn cycling_visits_every_mode_and_wraps() {
        let mut model = PlayerModel::new(true, PlayMode::ListLoop);
        assert_eq!(model.cycle_mode(), PlayMode::SingleLoop);
        assert_eq!(model.cycle_mode(), PlayMode::Shuffle);
        assert_eq!(model.cycle_mode(), PlayMode::ListLoop);
    }

    #[test]
    fn next_index_follows_the_play_mode() {
        assert_eq!(model(PlayMode::ListLoop, 0).next_index(), Some(1));
        assert_eq!(
            model(PlayMode::ListLoop, 2).next_index(),
            Some(0),
            "列表循环到尾回到开头"
        );
        assert_eq!(model(PlayMode::SingleLoop, 1).next_index(), Some(1), "单曲循环原地重复");
        assert_ne!(
            model(PlayMode::Shuffle, 1).next_index(),
            Some(1),
            "随机不落在当前曲目上"
        );
    }

    #[test]
    fn empty_queue_has_no_next_or_previous() {
        let model = PlayerModel::new(true, PlayMode::ListLoop);
        assert_eq!(model.next_index(), None);
        assert_eq!(model.prev_index(), None);
        assert!(model.current_song().is_none());
    }
}
