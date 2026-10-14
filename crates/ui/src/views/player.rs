//! 底部控制条与跨视图共享播放状态。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::Song;
use lyra_audio::{Snapshot, State};

use crate::bridge::{Backend, Msg};
use crate::format::{format_duration, progress_percent};
use crate::shell::AppShell;
use crate::theme::cover_box;

/// 播放控制与歌词共享的数据模型。
pub(crate) struct PlayerModel {
    /// 音频引擎运行状态快照。
    pub(crate) snapshot: Snapshot,
    /// 当前是否存在可用的音频输出硬件。
    pub(crate) has_engine: bool,
    /// 当前播放列表曲目队列。
    pub(crate) queue: Arc<Vec<Song>>,
    /// 当前播放曲目在队列中的下标。
    pub(crate) index: usize,
}

impl PlayerModel {
    /// 创建默认初始状态。
    pub(crate) fn new(has_engine: bool) -> Self {
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
        }
    }

    /// 获取当前曲目的展示元数据：`(哈希, 标题, 歌手)`。
    pub(crate) fn current_labels(&self) -> (Option<String>, SharedString, SharedString) {
        self.current_song().map_or_else(
            || (None, SharedString::new_static("未在播放"), SharedString::new_static("")),
            |song| {
                (
                    Some(song.hash.clone()),
                    SharedString::from(song.name.as_str()),
                    SharedString::from(song.singer.as_str()),
                )
            },
        )
    }

    /// 获取当前播放的歌曲实体。
    pub(crate) fn current_song(&self) -> Option<&Song> {
        self.queue.get(self.index)
    }

    /// 替换播放队列并跳转至指定索引曲目。
    pub(crate) fn set_queue(&mut self, queue: Arc<Vec<Song>>, index: usize) {
        self.queue = queue;
        self.index = index;
        self.snapshot.position_ms = 0;
    }

    /// 合并音频引擎最新快照，返回是否有可见属性变更。
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

/// 底部浮动播放条组件。
pub(crate) struct PlayerBar {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    progress: Entity<SliderState>,
    volume: Entity<SliderState>,
    dragging: bool,
    playing: bool,
    title: SharedString,
    singer: SharedString,
    current_hash: Option<String>,
    _subs: Vec<Subscription>,
}

impl PlayerBar {
    /// 创建播放条实例并注册滑块监听与快照轮询任务。
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
                .max(1.0)
                .step(0.01)
                .default_value(initial_volume)
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
                SliderEvent::Change(value) => (*value, false),
                SliderEvent::Release(value) => (*value, true),
            };
            this.apply_volume(value_single(value), persist);
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
            playing: false,
            title: SharedString::new_static("未在播放"),
            singer: SharedString::new_static(""),
            current_hash: None,
            _subs: vec![progress_sub, volume_sub],
        }
    }

    /// 将音频引擎状态快照同步至滑块与状态缓存。
    fn sync(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self.player.update(cx, |model, model_cx| {
            let changed = model.apply_snapshot(snapshot);
            if changed {
                model_cx.notify();
            }
            changed
        });

        let (hash, title, singer) = self.player.read(cx).current_labels();
        let retitled = hash != self.current_hash;
        if retitled {
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
        self.volume.update(cx, |state, state_cx| {
            if (value_single(state.value()) - volume).abs() > 0.001 {
                state.set_value(volume, window, state_cx);
            }
        });

        if changed || retitled {
            cx.notify();
        }
    }

    /// 切换播放与暂停状态。
    fn toggle(&self) {
        if let Some(engine) = self.backend.engine() {
            engine.toggle();
        } else {
            self.backend.send(Msg::Toast("没有可用的音频输出设备".to_owned()));
        }
    }

    /// 按百分比跳转播放进度。
    fn seek_percent(&self, percent: f32, cx: &Context<Self>) {
        let Some(engine) = self.backend.engine() else {
            return;
        };
        let duration = self.player.read(cx).snapshot.duration_ms;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ms = (f64::from(percent.clamp(0.0, 100.0)) / 100.0 * duration as f64).round() as u64;
        engine.seek(ms);
    }

    /// 设置音量并在松开时持久化设置。
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

/// 提取单值滑块的浮点数值。
fn value_single(value: SliderValue) -> f32 {
    match value {
        SliderValue::Single(single) => single,
        SliderValue::Range(start, _) => start,
    }
}

impl Render for PlayerBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (position, duration, has_engine) = {
            let model = self.player.read(cx);
            (model.snapshot.position_ms, model.snapshot.duration_ms, model.has_engine)
        };
        let play_label = if self.playing {
            SharedString::new_static("❚❚")
        } else {
            SharedString::new_static("▶")
        };
        let shell_prev = self.shell.clone();
        let shell_next = self.shell.clone();

        h_flex()
            .h(px(78.0))
            .w_full()
            .px_4()
            .gap(px(16.0))
            .bg(theme.background)
            .border_t_1()
            .border_color(theme.border)
            .child(cover_box(self.title.as_ref(), 48.0, cx))
            .child(
                v_flex()
                    .w(px(220.0))
                    .min_w(px(0.0))
                    .child(div().text_sm().truncate().child(self.title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(self.singer.clone()),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.0))
                    .child(
                        Button::new("prev")
                            .ghost()
                            .small()
                            .label("⏮")
                            .on_click(move |_event, _window, cx| {
                                shell_prev.update(cx, AppShell::play_prev).ok();
                            }),
                    )
                    .child(
                        Button::new("toggle")
                            .primary()
                            .small()
                            .label(play_label)
                            .on_click(cx.listener(|this, _event, _window, _cx| this.toggle())),
                    )
                    .child(
                        Button::new("next")
                            .ghost()
                            .small()
                            .label("⏭")
                            .on_click(move |_event, _window, cx| {
                                shell_next.update(cx, AppShell::play_next).ok();
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(10.0))
                    .child(
                        div()
                            .w(px(48.0))
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format_duration(position))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(Slider::new(&self.progress).w_full()),
                    )
                    .child(
                        div()
                            .w(px(48.0))
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format_duration(duration))),
                    ),
            )
            .child(
                h_flex()
                    .w(px(170.0))
                    .gap(px(8.0))
                    .child(div().text_xs().text_color(theme.muted_foreground).child("音量"))
                    .child(div().flex_1().child(Slider::new(&self.volume).w_full())),
            )
            .when(!has_engine, |this| {
                this.child(div().text_xs().text_color(theme.danger).child("无音频设备"))
            })
    }
}
