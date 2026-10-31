//! 底部播放控制条与共享播放模型。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::Song;
use lyra_audio::{Snapshot, State};

use crate::bridge::{Backend, Msg, Nav};
use crate::format::{format_duration, progress_percent};
use crate::shell::AppShell;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_HOVER, CARD_BG, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, badge, hex_rgb, quality_badge,
    song_cover,
};
use crate::views::tracks::{TrackRow, build_rows};

pub(crate) use lyra_app::PlayMode;

/// 音量滑轨宽度（像素）
///
/// 100 级音量铺在这条滑轨上：越宽，每像素对应的音量越细，
/// 拖拽才能精确落到目标值上。
const VOLUME_TRACK_WIDTH: f32 = 112.0;

/// 播放模式的展示文本
pub(crate) fn mode_label(mode: PlayMode) -> &'static str {
    match mode {
        PlayMode::ListLoop => "列表循环",
        PlayMode::SingleLoop => "单曲循环",
        PlayMode::Shuffle => "随机播放",
    }
}

/// 播放模式对应的图标
pub(crate) fn mode_icon(mode: PlayMode) -> IconName {
    match mode {
        PlayMode::ListLoop => IconName::Repeat,
        PlayMode::SingleLoop => IconName::Repeat1,
        PlayMode::Shuffle => IconName::Shuffle,
    }
}

/// 循环切换至下一播放模式
pub(crate) fn next_mode(mode: PlayMode) -> PlayMode {
    match mode {
        PlayMode::ListLoop => PlayMode::SingleLoop,
        PlayMode::SingleLoop => PlayMode::Shuffle,
        PlayMode::Shuffle => PlayMode::ListLoop,
    }
}

/// 播放与暂停状态的矢量图形（流体平滑圆角几何，无硬切顿挫感）
const PLAY_MORPH_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor">
  <path d="M 7.5 5.2 C 6.6 4.7 5.5 5.3 5.5 6.4 L 5.5 17.6 C 5.5 18.7 6.6 19.3 7.5 18.8 L 17.5 13.2 C 18.3 12.7 18.3 11.3 17.5 10.8 Z" />
</svg>"#;

const PAUSE_MORPH_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor">
  <rect x="6" y="4.5" width="3.6" height="15" rx="1.6" />
  <rect x="14.4" y="4.5" width="3.6" height="15" rx="1.6" />
</svg>"#;

fn play_pause_icon(playing: bool) -> AnyElement {
    svg()
        .data(if playing { PAUSE_MORPH_SVG } else { PLAY_MORPH_SVG })
        .size(px(16.0))
        .text_color(rgb(0xffffff))
        .into_any_element()
}

/// 循环切换至下一档音质（从标准逐级升级，到顶回到标准）
pub(crate) fn next_quality(current: &str) -> &'static str {
    let choices = crate::format::QUALITY_CHOICES;
    let index = choices
        .iter()
        .position(|(id, _)| *id == current)
        .map_or(0, |index| (index + 1) % choices.len());
    choices[index].0
}

/// 播放条上的紧凑音质标签（完整文案太长，放不下右侧控制区）
fn quality_short(quality: &str) -> &'static str {
    match quality {
        "high" => "Hi-Res",
        "flac" => "无损",
        "320" => "320K",
        "128" => "128K",
        "viper_clear" => "超清",
        "viper_tape" => "母带",
        "viper_atmos" => "全景",
        "multitrack" => "多轨",
        _ => "未知",
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
    /// 队列的预格式化展示模型（渲染期零分配）
    pub(crate) rows: Arc<Vec<TrackRow>>,
    /// 当前播放曲目在队列中的索引
    pub(crate) index: usize,
    /// 当前播放模式
    pub(crate) mode: PlayMode,
    /// 当前全局音质偏好（播放取址与全局切换共用）
    pub(crate) quality: String,
    /// 是否处于红心漫游（私人电台）：此时播放模式固定为列表循环
    pub(crate) radio: bool,
    /// 歌曲真实最高音质缓存（hash → Hi-Res / SQ / 空串）
    pub(crate) qualities: Arc<HashMap<String, &'static str>>,
    /// 随机播放用的伪随机状态（xorshift64*）
    ///
    /// 不能用纳秒时间戳：相邻两次调用可能落在同一纳秒，
    /// 结果会重复成同一首歌。
    rng_state: AtomicU64,
}

impl PlayerModel {
    /// 创建初始播放状态
    pub(crate) fn new(has_engine: bool, mode: PlayMode, volume: f32, quality: String) -> Self {
        Self {
            snapshot: Snapshot {
                state: State::Idle,
                track: None,
                position_ms: 0,
                duration_ms: 0,
                volume: volume.clamp(0.0, 1.0),
            },
            has_engine,
            queue: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            index: 0,
            mode,
            quality,
            radio: false,
            qualities: Arc::new(HashMap::new()),
            // 用当前时间播种一次；之后完全由自身状态推进
            rng_state: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64)
                    | 1,
            ),
        }
    }

    /// 合并一批歌曲的真实最高音质
    pub(crate) fn apply_qualities(&mut self, list: Vec<(String, &'static str)>) {
        let map = Arc::make_mut(&mut self.qualities);
        for (hash, label) in list {
            map.insert(hash, label);
        }
    }

    /// 获取曲目真实最高音质标签（未知时为 `None`，不展示徽标）
    pub(crate) fn quality_of(&self, hash: &str) -> Option<&'static str> {
        self.qualities.get(hash).copied().filter(|label| !label.is_empty())
    }

    /// 切换全局音质偏好
    pub(crate) fn cycle_quality(&mut self) -> String {
        self.quality = next_quality(&self.quality).to_owned();
        self.quality.clone()
    }

    /// 更新音量大小
    pub(crate) fn set_volume(&mut self, volume: f32) {
        self.snapshot.volume = volume.clamp(0.0, 1.0);
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

    /// 循环切换播放模式（红心漫游期间锁定为列表循环）
    pub(crate) fn cycle_mode(&mut self) -> PlayMode {
        if self.radio {
            self.mode = PlayMode::ListLoop;
            return self.mode;
        }
        self.mode = next_mode(self.mode);
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

    /// 电台顺序播放：只前进一位，绝不回绕到队首
    ///
    /// 红心电台是无限流：播放模式对它无效，队列用尽时返回 `None`，
    /// 由上层继续拉取下一批。
    pub(crate) fn sequential_next(&self) -> Option<usize> {
        (self.index + 1 < self.queue.len()).then_some(self.index + 1)
    }

    /// 电台顺序回退：只后退一位，队首不再回绕
    pub(crate) fn sequential_prev(&self) -> Option<usize> {
        self.index.checked_sub(1)
    }

    /// 获取当前曲目的哈希（零分配，用于判断曲目是否变化）
    pub(crate) fn current_hash(&self) -> Option<&str> {
        self.current_song().map(|song| song.hash.as_str())
    }

    /// 获取当前曲目标题、歌手与封面（仅在曲目真正变化时调用）
    pub(crate) fn current_labels(&self) -> (Option<String>, SharedString, SharedString, SharedString) {
        self.current_song().map_or_else(
            || {
                (
                    None,
                    SharedString::new_static("未在播放"),
                    SharedString::new_static(""),
                    SharedString::new_static(""),
                )
            },
            |song| {
                (
                    Some(song.hash.clone()),
                    SharedString::from(song.display_title()),
                    SharedString::from(song.display_singer()),
                    SharedString::from(song.cover_at(480)),
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
        // 展示文本在这里预格式化：队列抽屉每帧都要渲染，不能在渲染期拼字符串
        self.rows = Arc::new(build_rows(&queue));
        self.queue = queue;
        self.index = index;
        self.snapshot.position_ms = 0;
    }

    /// 追加新曲目至队列末尾（用于红心电台连续流推荐）
    ///
    /// 跳过队列里已有的 hash：上游偶发重复推荐时，去重是“不重样”的兜底。
    /// 返回第一首新歌的下标；全是旧歌时返回 `None`。
    pub(crate) fn append_songs(&mut self, new_songs: Vec<Song>) -> Option<usize> {
        let mut seen: HashSet<String> = self.queue.iter().map(|song| song.hash.clone()).collect();
        let start = self.queue.len();
        let mut queue = (*self.queue).clone();
        for song in new_songs {
            if !song.hash.is_empty() && !seen.insert(song.hash.clone()) {
                continue;
            }
            queue.push(song);
        }
        if queue.len() == start {
            return None;
        }
        self.rows = Arc::new(build_rows(&queue));
        self.queue = Arc::new(queue);
        Some(start)
    }

    /// 只切换当前索引（进度归零，避免沿用上一首的位置）
    pub(crate) fn set_index(&mut self, index: usize) {
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
    volume_hovered: bool,
    progress_hovered: bool,
    hover_scrub_x: f32,
    hover_scrub_ratio: Option<f32>,
    play_btn_clicked_seq: u64,
    playing: bool,
    title: SharedString,
    singer: SharedString,
    cover: SharedString,
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
    /// 记录最近一次非静音时的音量（供再次点击静音按钮时无损恢复）
    last_unmuted_volume: f32,
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
        let backend_volume = backend.app().volume();
        let last_unmuted_volume = if backend_volume > 0.001 {
            backend_volume
        } else if initial_volume > 0.001 {
            initial_volume
        } else {
            0.8
        };
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
        let volume_sub = cx.subscribe(&volume, |this, _state, event: &SliderEvent, cx| {
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
            let val = (value_single(value) / 100.0).clamp(0.0, 1.0);
            if val > 0.001 {
                this.last_unmuted_volume = val;
                this.set_volume_and_muted(val, false, persist, cx);
            } else {
                this.set_volume_and_muted(this.last_unmuted_volume, true, persist, cx);
            }
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
            volume_hovered: false,
            progress_hovered: false,
            hover_scrub_x: 0.0,
            hover_scrub_ratio: None,
            play_btn_clicked_seq: 0,
            playing: false,
            title: SharedString::new_static("未在播放"),
            singer: SharedString::new_static(""),
            cover: SharedString::new_static(""),
            current_hash: None,
            duration_text: SharedString::new_static("0:00"),
            queue_tip: SharedString::new_static("播放队列 (0 首)"),
            mode_tip: SharedString::new_static("播放模式"),
            last_duration: 0,
            last_queue_len: 0,
            last_mode: PlayMode::ListLoop,
            last_unmuted_volume,
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
            let (hash, title, singer, cover) = self.player.read(cx).current_labels();
            self.current_hash = hash;
            self.title = title;
            self.singer = singer;
            self.cover = cover;
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

        if volume > 0.001 {
            self.last_unmuted_volume = volume;
        }

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
            self.mode_tip = SharedString::from(format!("播放模式：{}", mode_label(mode)));
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
            match app.update_settings(|settings| settings.play_mode = mode) {
                Ok(()) => Msg::Noop,
                Err(error) => Msg::Toast(format!("保存播放模式失败: {error}")),
            }
        });
    }

    /// 统一调整音量与静音状态，即时通知 UI 并按需持久化
    fn set_volume_and_muted(&mut self, volume: f32, muted: bool, persist: bool, cx: &mut Context<Self>) {
        let volume = volume.clamp(0.0, 1.0);
        let effective_volume = if muted { 0.0 } else { volume };
        self.player.update(cx, |model, model_cx| {
            model.set_volume(effective_volume);
            model_cx.notify();
        });
        if let Some(engine) = self.backend.engine() {
            engine.set_volume(effective_volume);
        }
        if persist {
            let app = self.backend.app();
            self.backend.spawn(async move {
                match app.update_settings(|settings| {
                    settings.volume = volume;
                    settings.muted = muted;
                }) {
                    Ok(()) => Msg::Noop,
                    Err(error) => Msg::Toast(format!("保存音量失败: {error}")),
                }
            });
        }
        cx.notify();
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
        let (position, duration, has_engine, volume, queue, current_index, play_mode, radio) = {
            let model = self.player.read(cx);
            (
                model.snapshot.position_ms,
                model.snapshot.duration_ms,
                model.has_engine,
                model.snapshot.volume,
                // 队列是 Arc：克隆只是引用计数递增。
                // 曲目本体等到点击「收藏」时再克隆，渲染期不做整首拷贝。
                model.queue.clone(),
                model.index,
                model.mode,
                model.radio,
            )
        };
        // 当前曲目的真实最高音质与全局音质偏好（一次读取，渲染期零分配）
        let (quality_badge_label, quality_pref) = {
            let model = self.player.read(cx);
            (
                model.current_song().and_then(|song| model.quality_of(&song.hash)),
                model.quality.clone(),
            )
        };
        let has_song = queue.get(current_index).is_some();
        // 未悬停时也要看得见进度：已播部分的比例（0..=1）
        let played_ratio = progress_percent(position, duration) / 100.0;

        // 进度每帧都在变：复用缓冲区格式化，再转成可廉价克隆的 SharedString
        crate::format::write_duration(&mut self.pos_buf, position);
        let position_text = SharedString::new(self.pos_buf.as_str());

        let is_muted = volume <= 0.001;
        let vol_icon = if is_muted { IconName::VolumeX } else { IconName::Volume2 };

        let shell_prev = self.shell.clone();
        let shell_next = self.shell.clone();
        let shell_lyrics_cover = self.shell.clone();
        let shell_queue = self.shell.clone();
        let like_backend = self.backend.clone();
        let like_queue = queue.clone();

        let vol_num = if is_muted {
            0
        } else {
            value_single(self.volume.read(cx).value()).round().clamp(0.0, 100.0) as u32
        };

        h_flex()
            .h(px(72.0))
            .w_full()
            .px_5()
            .gap(px(16.0))
            .bg(CARD_BG)
            .border_t_1()
            .border_color(hex_rgb(0xe5e7eb))
            .items_center()
            // 封面、曲目信息与收藏按钮
            .child(
                h_flex()
                    .w(px(250.0))
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
                            .child(if has_song {
                                song_cover(self.cover.as_ref(), self.title.as_ref(), 44.0)
                            } else {
                                div()
                                    .size(px(44.0))
                                    .rounded(px(8.0))
                                    .bg(linear_gradient(
                                        135.0,
                                        linear_color_stop(hex_rgb(0xf3f4f6), 0.0),
                                        linear_color_stop(hex_rgb(0xe5e7eb), 1.0),
                                    ))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Icon::new(IconName::Music).size(px(20.0)).text_color(TEXT_FAINT))
                                    .into_any_element()
                            }),
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
                                    .when(quality_badge_label.is_some(), |this| {
                                        this.child(quality_badge(quality_badge_label.unwrap_or_default()))
                                    }),
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
                            .tooltip("收藏当前歌曲到「我喜欢」")
                            .on_click(move |_event, _window, _cx| {
                                let Some(song) = like_queue.get(current_index) else {
                                    return;
                                };
                                let song = song.clone();
                                let app = like_backend.app();
                                like_backend.spawn(async move {
                                    Msg::Liked(app.like(&song).await.map_err(|error| error.to_string()))
                                });
                            }),
                    )
                    .child(
                        Button::new("player_add_to")
                            .ghost()
                            .small()
                            .icon(IconName::ListPlus)
                            .tooltip("添加到歌单")
                            .when(has_song, |this| {
                                let shell = self.shell.clone();
                                let queue = queue.clone();
                                this.on_click(move |_event, _window, cx| {
                                    let Some(song) = queue.get(current_index) else {
                                        return;
                                    };
                                    let songs = vec![song.clone()];
                                    shell.update(cx, |shell, cx| shell.open_add_to(songs, cx)).ok();
                                })
                            }),
                    ),
            )
            // 核心控制区与进度条（紧凑居中平衡布局，固定槽高防止 Hover 晃动）
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .items_center()
                    .justify_center()
                    .gap(px(2.0))
                    .child(
                        h_flex()
                            .h(px(38.0))
                            .gap(px(12.0))
                            .items_center()
                            // 切换播放模式（红心漫游期间固定列表循环，不可切换）
                            .child(
                                Button::new("mode_toggle")
                                    .ghost()
                                    .small()
                                    .icon(if radio { IconName::Repeat } else { mode_icon(play_mode) })
                                    .tooltip(self.mode_tip.clone())
                                    .disabled(radio)
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        let next = this.player.update(cx, |model, cx| {
                                            let m = model.cycle_mode();
                                            cx.notify();
                                            m
                                        });
                                        this.persist_mode(next);
                                    })),
                            )
                            .child(
                                Button::new("prev")
                                    .ghost()
                                    .small()
                                    .icon(IconName::SkipBack)
                                    .tooltip(if radio { "红心漫游不提供上一首" } else { "上一首" })
                                    .disabled(radio)
                                    .on_click(move |_event, _window, cx| {
                                        shell_prev.update(cx, AppShell::play_prev).ok();
                                    }),
                            )
                            // 播放/暂停按钮（形态变换 Morphing 与微弹性回缩 Scale: 1.0 -> 0.94 -> 1.0）
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
                                    .active(|s| s.bg(hex_rgb(0x047857)))
                                    .shadow_sm()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.play_btn_clicked_seq += 1;
                                        this.toggle();
                                        cx.notify();
                                    }))
                                    .with_animation(
                                        ("play_btn_press", self.play_btn_clicked_seq),
                                        Animation::new(crate::motion::DURATION_MICRO).with_easing(crate::motion::apple_damping_spring()),
                                        |btn, delta| {
                                            let s = if delta < 0.5 {
                                                38.0 - 2.4 * (delta * 2.0)
                                            } else {
                                                35.6 + 2.4 * ((delta - 0.5) * 2.0)
                                            };
                                            btn.w(px(s)).h(px(s))
                                        },
                                    )
                                    .child(
                                        div()
                                            .with_animation(
                                                ("play_icon_morph", usize::from(self.playing)),
                                                Animation::new(crate::motion::DURATION_MICRO).with_easing(crate::motion::apple_damping_spring()),
                                                |icon, delta| icon.opacity(0.70 + 0.30 * delta),
                                            )
                                            .child(play_pause_icon(self.playing)),
                                    ),
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
                    // 进度条与对齐时间（固定高度槽位与精准时间点跟随预览 Timeline Scrubbing）
                    .child(
                        h_flex()
                            .h(px(20.0))
                            .w_full()
                            .gap(px(8.0))
                            .items_center()
                            .child(
                                div()
                                    .w(px(38.0))
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(TEXT_FAINT)
                                    .text_right()
                                    .child(position_text),
                            )
                            .child({
                                div()
                                    .id("progress_scrub_wrap")
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .h(px(20.0))
                                    .flex()
                                    .items_center()
                                    .relative()
                                    .when(has_song, |this| {
                                        this.cursor_pointer()
                                            .on_hover(cx.listener(|this, hovered, _window, cx| {
                                                this.progress_hovered = *hovered;
                                                if !*hovered {
                                                    this.hover_scrub_ratio = None;
                                                }
                                                cx.notify();
                                            }))
                                            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                                                let bounds = this.progress.read(cx).bounds();
                                                let width = f32::from(bounds.size.width);
                                                if width > 0.0 {
                                                    let offset_x = (f32::from(event.position.x) - f32::from(bounds.origin.x)).clamp(0.0, width);
                                                    let ratio = (offset_x / width).clamp(0.0, 1.0);
                                                    this.hover_scrub_x = offset_x;
                                                    this.hover_scrub_ratio = Some(ratio);
                                                    cx.notify();
                                                }
                                            }))
                                    })
                                    .when(!has_song, |this| {
                                        // 未播放状态（Idle State）：细灰线（opacity: 0.35），无滑块手柄，禁用交互
                                        this.child(
                                            div()
                                                .w_full()
                                                .h(px(2.5))
                                                .rounded_full()
                                                .bg(hex_rgb(0xe5e7eb))
                                                .opacity(0.35),
                                        )
                                    })
                                    .when(has_song && !self.progress_hovered && !self.dragging, |this| {
                                        // 常态：3px 极细灰轨 + 已播进度填充 + 进度点（不悬停也看得到播放位置）
                                        this.child(
                                            div()
                                                .w_full()
                                                .h(px(3.0))
                                                .rounded_full()
                                                .bg(hex_rgb(0xe5e7eb))
                                                .relative()
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .left(px(0.0))
                                                        .top(px(0.0))
                                                        .h_full()
                                                        .rounded_full()
                                                        .bg(ACCENT_GREEN)
                                                        .w(relative(played_ratio)),
                                                )
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .left(relative(played_ratio))
                                                        .top(px(-2.5))
                                                        .ml(px(-3.0))
                                                        .size(px(8.0))
                                                        .rounded_full()
                                                        .bg(ACCENT_GREEN),
                                                ),
                                        )
                                    })
                                    .when(has_song && (self.progress_hovered || self.dragging), |this| {
                                        // 悬停/拖拽状态：进度条加粗至 5px，浮现滑块与精准时间点跟随气泡 (Timeline Scrubbing)
                                        this.child(
                                            Slider::new(&self.progress)
                                                .w_full()
                                                // 必须显式着色：默认主题的滑块是近白色、手柄是黑色；
                                                // 这里统一走灰色，不保留绿色填充
                                                .bg(hex_rgb(0xe5e7eb))
                                                .text_color(hex_rgb(0xcbd5e1)),
                                        )
                                            .when_some(self.hover_scrub_ratio, |this, ratio| {
                                                let duration = self.player.read(cx).snapshot.duration_ms;
                                                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                                                let scrub_ms = (f64::from(ratio) * duration as f64).round() as u64;
                                                this.child(
                                                    div()
                                                        .absolute()
                                                        .bottom(px(20.0))
                                                        .left(px((self.hover_scrub_x - 18.0).max(0.0)))
                                                        .px_1p5()
                                                        .py(px(1.5))
                                                        .rounded(px(4.0))
                                                        .bg(hex_rgb(0x1f2937))
                                                        .shadow_md()
                                                        .text_color(hex_rgb(0xffffff))
                                                        .text_size(px(10.5))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(format_duration(scrub_ms)),
                                                )
                                            })
                                    })
                            })
                            .child(
                                div()
                                    .w(px(38.0))
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(TEXT_FAINT)
                                    .child(self.duration_text.clone()),
                            ),
                    ),
            )
            // 音量与播放队列
            .child(
                h_flex()
                    // 比左侧宽：音量滑轨 112px + 音质切换按钮 + 队列按钮都挤在这一侧
                    .w(px(312.0))
                    .justify_end()
                    .items_center()
                    .gap(px(8.0))
                    .when(!has_engine, |this| {
                        this.child(badge("无音频设备", hsla(0.0, 0.85, 0.95, 1.0), hsla(0.0, 0.85, 0.45, 1.0)))
                    })
                    // 音量调节（支持悬浮滚轮直接微调与数值提示气泡 Tooltip）
                    .child(
                        h_flex()
                            .id("vol_control_wrapper")
                            .relative()
                            .items_center()
                            .gap(px(4.0))
                            .on_hover(cx.listener(|this, hovered, _window, cx| {
                                this.volume_hovered = *hovered;
                                cx.notify();
                            }))
                            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                                let dy = match event.delta {
                                    ScrollDelta::Pixels(p) => f32::from(p.y),
                                    ScrollDelta::Lines(p) => p.y * 16.0,
                                };
                                if dy.abs() > 0.01 {
                                    let step = if dy > 0.0 { 0.03 } else { -0.03 };
                                    let cur = this.player.read(cx).snapshot.volume;
                                    let base = if cur <= 0.001 && dy > 0.0 {
                                        this.last_unmuted_volume
                                    } else {
                                        cur
                                    };
                                    let next = (base + step).clamp(0.0, 1.0);
                                    if next > 0.001 {
                                        this.last_unmuted_volume = next;
                                        this.volume.update(cx, |state, state_cx| {
                                            state.set_value(next * 100.0, window, state_cx);
                                        });
                                        this.set_volume_and_muted(next, false, true, cx);
                                    } else {
                                        this.volume.update(cx, |state, state_cx| {
                                            state.set_value(0.0, window, state_cx);
                                        });
                                        this.set_volume_and_muted(this.last_unmuted_volume, true, true, cx);
                                    }
                                    this.volume_hovered = true;
                                }
                            }))
                            .child(
                                Button::new("vol_toggle")
                                    .ghost()
                                    .small()
                                    .icon(vol_icon)
                                    .tooltip(if is_muted { "恢复音量" } else { "静音" })
                                    .on_click(cx.listener(|this, _event, window, cx| {
                                        let current = this.player.read(cx).snapshot.volume;
                                        let is_currently_muted = current <= 0.001;
                                        if is_currently_muted {
                                            let target = if this.last_unmuted_volume > 0.001 {
                                                this.last_unmuted_volume
                                            } else {
                                                0.8
                                            };
                                            this.volume.update(cx, |state, state_cx| {
                                                state.set_value(target * 100.0, window, state_cx);
                                            });
                                            this.set_volume_and_muted(target, false, true, cx);
                                        } else {
                                            this.last_unmuted_volume = current;
                                            this.volume.update(cx, |state, state_cx| {
                                                state.set_value(0.0, window, state_cx);
                                            });
                                            this.set_volume_and_muted(current, true, true, cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .w(px(VOLUME_TRACK_WIDTH))
                                    .relative()
                                    .child(
                                        Slider::new(&self.volume)
                                            .w_full()
                                            // 默认主题的滑轨是近白色，白底上看不见已填充的部分；
                                            // 统一走灰色，不保留绿色填充
                                            .bg(hex_rgb(0xe5e7eb))
                                            .text_color(hex_rgb(0xcbd5e1)),
                                    )
                                    // 数值气泡跟随滑块，拖拽时能直接看到落在哪个值上
                                    .when(self.volume_hovered || self.volume_dragging, |this| {
                                        let knob = VOLUME_TRACK_WIDTH * (vol_num as f32 / 100.0);
                                        this.child(
                                            div()
                                                .absolute()
                                                .bottom(px(18.0))
                                                .left(px((knob - 14.0).clamp(0.0, VOLUME_TRACK_WIDTH - 30.0)))
                                                .px_2()
                                                .py(px(1.5))
                                                .rounded(px(4.0))
                                                .bg(hex_rgb(0x1f2937))
                                                .shadow_md()
                                                .text_color(hex_rgb(0xffffff))
                                                .text_size(px(10.5))
                                                .font_weight(FontWeight::BOLD)
                                                .child(SharedString::from(format!("{vol_num}%"))),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(32.0))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if is_muted { TEXT_FAINT } else { TEXT_MUTED })
                                    .child(format!("{vol_num}%")),
                            ),
                    )
                    // 全局音质切换（播放区域直接切换，持久化到设置）
                    .child(
                        Button::new("quality_toggle")
                            .ghost()
                            .small()
                            .label(quality_short(&quality_pref))
                            .tooltip("点击切换全局音质偏好")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                let next = this.player.update(cx, |model, model_cx| {
                                    let quality = model.cycle_quality();
                                    model_cx.notify();
                                    quality
                                });
                                let app = this.backend.app();
                                this.backend.spawn(async move {
                                    match app.update_settings(|settings| settings.quality = next) {
                                        Ok(()) => Msg::Noop,
                                        Err(error) => Msg::Toast(format!("保存音质失败: {error}")),
                                    }
                                });
                                // 当前曲目立即用新音质重播，不用等下一首
                                this.shell.update(cx, AppShell::replay_current).ok();
                            })),
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

    use super::{PlayMode, PlayerModel, next_quality};

    /// 构建包含 3 首曲目的测试播放模型
    fn model(mode: PlayMode, index: usize) -> PlayerModel {
        let songs: Vec<Song> = ["a", "b", "c"]
            .iter()
            .map(|hash| Song {
                hash: (*hash).to_owned(),
                ..Song::default()
            })
            .collect();
        let mut model = PlayerModel::new(true, mode, 0.8, "flac".to_owned());
        model.set_queue(Arc::new(songs), index);
        model
    }

    #[test]
    fn persisted_mode_is_what_the_session_starts_with() {
        // 恢复持久化的播放模式
        assert_eq!(
            PlayerModel::new(true, PlayMode::Shuffle, 0.8, "flac".to_owned()).mode,
            PlayMode::Shuffle
        );
        assert_eq!(
            PlayerModel::new(true, PlayMode::SingleLoop, 0.8, "flac".to_owned()).mode,
            PlayMode::SingleLoop
        );
        assert_eq!(
            PlayerModel::new(true, PlayMode::ListLoop, 0.8, "flac".to_owned()).mode,
            PlayMode::ListLoop
        );
    }

    #[test]
    fn persisted_volume_is_what_the_session_starts_with() {
        // 恢复持久化的音量设置（含范围限制）
        assert!(
            (PlayerModel::new(true, PlayMode::ListLoop, 0.35, "flac".to_owned())
                .snapshot
                .volume
                - 0.35)
                .abs()
                < f32::EPSILON
        );
        assert!(
            (PlayerModel::new(true, PlayMode::ListLoop, 1.5, "flac".to_owned())
                .snapshot
                .volume
                - 1.0)
                .abs()
                < f32::EPSILON
        );
        assert!(
            (PlayerModel::new(true, PlayMode::ListLoop, -0.2, "flac".to_owned())
                .snapshot
                .volume
                - 0.0)
                .abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn cycling_visits_every_mode_and_wraps() {
        let mut model = PlayerModel::new(true, PlayMode::ListLoop, 0.8, "flac".to_owned());
        assert_eq!(model.cycle_mode(), PlayMode::SingleLoop);
        assert_eq!(model.cycle_mode(), PlayMode::Shuffle);
        assert_eq!(model.cycle_mode(), PlayMode::ListLoop);
    }

    #[test]
    fn quality_cycles_through_the_settings_choices_and_wraps() {
        assert_eq!(next_quality("128"), "320");
        assert_eq!(next_quality("320"), "flac");
        assert_eq!(next_quality("flac"), "high");
        assert_eq!(next_quality("high"), "viper_clear");
        assert_eq!(next_quality("multitrack"), "128", "最末一档回到标准");
        // 未知/空值从第一档开始，不能停在一个非法音质上
        assert_eq!(next_quality(""), "128");
        let mut model = PlayerModel::new(true, PlayMode::ListLoop, 0.8, "flac".to_owned());
        assert_eq!(model.cycle_quality(), "high");
        assert_eq!(model.quality, "high", "切换结果必须写回共享模型（全局生效）");
    }

    #[test]
    fn radio_locks_the_mode_to_list_loop() {
        let mut model = PlayerModel::new(true, PlayMode::Shuffle, 0.8, "flac".to_owned());
        model.radio = true;
        assert_eq!(model.cycle_mode(), PlayMode::ListLoop);
        assert_eq!(model.cycle_mode(), PlayMode::ListLoop, "漫游期间模式不得被改走");
    }

    #[test]
    fn quality_labels_hide_the_badge_when_unknown() {
        let mut model = PlayerModel::new(true, PlayMode::ListLoop, 0.8, "flac".to_owned());
        assert_eq!(model.quality_of("abc"), None, "未知歌曲不展示徽标");
        model.apply_qualities(vec![("abc".to_owned(), ""), ("sq".to_owned(), "SQ")]);
        assert_eq!(model.quality_of("abc"), None, "查过但无高音质的歌也不展示");
        assert_eq!(model.quality_of("sq"), Some("SQ"));
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
        let model = PlayerModel::new(true, PlayMode::ListLoop, 0.8, "flac".to_owned());
        assert_eq!(model.next_index(), None);
        assert_eq!(model.prev_index(), None);
        assert!(model.current_song().is_none());
    }

    fn songs(hashes: &[&str]) -> Vec<Song> {
        hashes
            .iter()
            .map(|hash| Song {
                hash: (*hash).to_owned(),
                ..Song::default()
            })
            .collect()
    }

    #[test]
    fn radio_advances_sequentially_without_wrapping() {
        // 红心电台不理播放模式：单曲循环 / 随机也必须往后走，且到队尾不回绕
        for mode in [PlayMode::ListLoop, PlayMode::SingleLoop, PlayMode::Shuffle] {
            let mut model = model(mode, 0);
            assert_eq!(model.sequential_next(), Some(1), "{mode:?}");
            model.set_index(2);
            assert_eq!(model.sequential_next(), None, "{mode:?} 不得回到队首");
            assert_eq!(model.sequential_prev(), Some(1));
            model.set_index(0);
            assert_eq!(model.sequential_prev(), None, "队首不再回绕");
        }
    }

    #[test]
    fn append_skips_songs_already_in_the_queue() {
        let mut model = model(PlayMode::ListLoop, 0);
        // 上游重复推荐同一批歌时，队列不能长出重复曲目
        assert_eq!(model.append_songs(songs(&["b", "d", "d"])), Some(3));
        let hashes: Vec<&str> = model.queue.iter().map(|song| song.hash.as_str()).collect();
        assert_eq!(hashes, vec!["a", "b", "c", "d"]);
        assert_eq!(model.append_songs(songs(&["a", "b"])), None, "全是旧歌时不推进");
        assert_eq!(model.queue.len(), 4);
    }
}
