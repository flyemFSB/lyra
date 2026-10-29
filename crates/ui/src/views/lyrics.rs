//! 沉浸式透明彩胶唱机与动态滚动歌词视图。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::LyricLine;

use crate::bridge::Backend;
use crate::format::lyric_index;
use crate::palette::{AlbumPalette, fallback_palette};
use crate::shell::AppShell;
use crate::theme::{TEXT_MUTED, TEXT_PRIMARY, hex_rgb, notice_with_icon, quality_badge, song_cover};
use crate::views::player::PlayerModel;

/// 歌词视图
pub(crate) struct LyricsView {
    backend: Backend,
    shell: WeakEntity<AppShell>,
    player: Entity<PlayerModel>,
    lines: Arc<Vec<LyricLine>>,
    /// 预渲染歌词文本缓存
    texts: Arc<Vec<SharedString>>,
    title: SharedString,
    singer: SharedString,
    cover: SharedString,
    current_hash: Option<String>,
    status: Option<SharedString>,
    /// 歌词滚动区控制柄
    scroll: ScrollHandle,
    /// 已跟随到的歌词行；None 表示布局还没就绪，下一帧重试
    followed: Option<usize>,
    /// 居中滚动的起点与当前目标偏移
    scroll_from: f32,
    scroll_target: f32,
    /// 本次滚动是否已落位，落位后不再接管偏移，让用户自由滚动
    scroll_settled: Rc<Cell<bool>>,
    /// 是否已安排下一帧的跟随回调（避免重复排队）
    follow_scheduled: bool,
    /// 当前专辑提取的光晕调色盘（滤除脏灰，动态自适应封面色调）
    palette: AlbumPalette,
    _sub: Subscription,
}

/// 歌词滚动与切换动画时长
const LYRIC_ANIMATION: Duration = Duration::from_millis(320);

/// 让 `item` 的竖向中线落在 `viewport` 正中所需的滚动偏移
fn center_offset(viewport: Bounds<Pixels>, item: Bounds<Pixels>) -> Pixels {
    viewport.center().y - item.center().y
}

/// 离当前行越远的歌词严格按照距离衰减模型衰减：
/// 距离当前句最近的上一句/下一句透明度为 60%，上下第二句为 35%，更远的句为 15%
fn lyric_fade(distance: usize) -> f32 {
    match distance {
        0 => 1.0,
        1 => 0.60,
        2 => 0.35,
        _ => 0.15,
    }
}

impl LyricsView {
    /// 创建歌词视图并订阅播放进度
    pub(crate) fn new(
        backend: Backend,
        shell: WeakEntity<AppShell>,
        player: Entity<PlayerModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.observe(&player, |this, _player, cx| {
            this.refresh_title(cx);
            cx.notify();
        });
        Self {
            backend,
            shell,
            player,
            lines: Arc::new(Vec::new()),
            texts: Arc::new(Vec::new()),
            title: SharedString::new_static("未在播放"),
            singer: SharedString::new_static(""),
            cover: SharedString::new_static(""),
            current_hash: None,
            status: None,
            scroll: ScrollHandle::new(),
            followed: None,
            scroll_from: 0.0,
            scroll_target: 0.0,
            scroll_settled: Rc::new(Cell::new(true)),
            follow_scheduled: false,
            palette: AlbumPalette::default(),
            _sub: sub,
        }
    }

    /// 清空歌词与曲目信息
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.lines = Arc::new(Vec::new());
        self.texts = Arc::new(Vec::new());
        self.title = SharedString::new_static("未在播放");
        self.singer = SharedString::new_static("");
        self.cover = SharedString::new_static("");
        self.current_hash = None;
        self.status = None;
        self.followed = None;
        self.palette = AlbumPalette::default();
        cx.notify();
    }

    /// 设置歌词行列表
    pub(crate) fn set_lines(&mut self, result: Result<Vec<LyricLine>, String>, cx: &mut Context<Self>) {
        self.followed = None;
        match result {
            Ok(lines) => {
                self.texts = Arc::new(
                    lines
                        .iter()
                        .map(|line| SharedString::from(line.text.as_str()))
                        .collect(),
                );
                self.lines = Arc::new(lines);
                self.status = None;
            }
            Err(error) => {
                self.lines = Arc::new(Vec::new());
                self.texts = Arc::new(Vec::new());
                self.status = Some(error.into());
            }
        }
        cx.notify();
    }

    /// 跟随当前行：行号变化时重新开始一次居中滚动，目标偏移逐帧按最新布局校正
    fn follow_lyrics(&mut self, current: Option<usize>) -> bool {
        if self.followed != current {
            let Some(index) = current else {
                self.followed = None;
                return true;
            };
            let Some(item) = self.scroll.bounds_for_item(index) else {
                return false;
            };
            self.scroll_from = self.scroll.offset().y.into();
            self.scroll_target = center_offset(self.scroll.bounds(), item).into();
            self.followed = current;
            self.scroll_settled.set(false);
            return true;
        }
        if let Some(index) = current
            && let Some(item) = self.scroll.bounds_for_item(index)
        {
            let target: f32 = center_offset(self.scroll.bounds(), item).into();
            if (target - self.scroll_target).abs() > 0.5 {
                self.scroll_target = target;
                self.scroll_settled.set(false);
                return true;
            }
        }
        false
    }

    /// 安排下一帧执行跟随计算
    fn schedule_follow(&mut self, current: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.follow_scheduled {
            return;
        }
        self.follow_scheduled = true;
        cx.on_next_frame(window, move |this, _window, cx| {
            this.follow_scheduled = false;
            if this.follow_lyrics(current) {
                cx.notify();
            }
        });
    }

    /// 跳转至指定播放进度（毫秒）
    fn seek_to(&self, time_ms: u64) {
        if let Some(engine) = self.backend.engine() {
            engine.seek(time_ms);
        }
    }

    /// 刷新当前曲目标题、歌手与专辑调色盘
    fn refresh_title(&mut self, cx: &Context<Self>) {
        let changed = {
            let model = self.player.read(cx);
            model.current_hash() != self.current_hash.as_deref()
        };
        if !changed {
            return;
        }
        let (hash, title, singer, cover) = self.player.read(cx).current_labels();
        self.current_hash = hash;
        self.title = title.clone();
        self.singer = singer.clone();
        self.cover = cover.clone();

        // 优先使用高品质亮温调色盘即刻铺底
        self.palette = fallback_palette(title.as_ref(), singer.as_ref());

        // 异步提取封面真实量化主色调
        //
        // 请求必须交给 Tokio 运行时：GPUI 前台执行器没有 Tokio reactor，
        // 在 `cx.spawn` 里直接 await reqwest 会 panic
        // （release 开了 panic = "abort"，一播放带封面的歌曲就整个进程闪退）。
        let cover_url = cover.to_string();
        if cover_url.starts_with("http") {
            let runtime = self.backend.runtime();
            cx.spawn(async move |this, cx| {
                let fetched = runtime
                    .spawn(async move {
                        let client = reqwest::Client::builder()
                            .timeout(Duration::from_secs(5))
                            .build()
                            .ok()?;
                        let resp = client.get(&cover_url).send().await.ok()?;
                        resp.bytes().await.ok()
                    })
                    .await;
                let Ok(Some(bytes)) = fetched else {
                    return;
                };
                let Some(extracted) = crate::palette::extract_palette_from_bytes(&bytes) else {
                    return;
                };
                let _ = this.update(cx, |view, cx| {
                    view.palette = extracted;
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// 渲染歌词流：当前行放大至 26px 加粗，非活动行按距离模型渐隐，上下双端柔光蒙版
    fn lyric_stream(&self, current: Option<usize>, cx: &Context<Self>) -> AnyElement {
        if let Some(status) = &self.status {
            return notice_with_icon(IconName::CircleAlert, status.clone()).into_any_element();
        }
        if self.lines.is_empty() {
            return notice_with_icon(IconName::Mic, "暂无歌词（播放歌曲后将自动匹配歌词写真）").into_any_element();
        }

        // 上下各留半个视口，首尾歌词也能停到正中
        let pad = px(f32::from(self.scroll.bounds().size.height) / 2.0);
        let (from, to) = (self.scroll_from, self.scroll_target);
        let scroll = self.scroll.clone();
        let settled = self.scroll_settled.clone();

        let mut body = v_flex()
            .id("lyrics-scroll-body")
            .size_full()
            .items_start()
            .gap(px(14.0))
            .pt(pad)
            .pb(pad)
            .overflow_y_scroll()
            .track_scroll(&self.scroll);

        for (index, line) in self.lines.iter().enumerate() {
            let active = current == Some(index);
            let text = self.texts.get(index).cloned().unwrap_or_default();
            let time_ms = line.time_ms;
            let distance = current.map_or(usize::MAX, |current| index.abs_diff(current));

            // 当前正在播放的歌词放大至 26px Semi-Bold（深色 #1C1C1E），未播放的保持 16px Regular
            let label = div()
                .when(active, |this| {
                    this.text_size(px(26.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(hex_rgb(0x1c1c1e))
                })
                .when(distance == 1, |this| {
                    this.text_size(px(18.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(hex_rgb(0x1c1c1e))
                })
                .when(distance == 2, |this| {
                    this.text_size(px(16.0))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(TEXT_MUTED)
                })
                .when(distance >= 3, |this| {
                    this.text_size(px(15.0))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(TEXT_MUTED)
                })
                .hover(|s| s.text_color(hex_rgb(0x1c1c1e)))
                .child(text);

            let label = if active {
                label
                    .with_animation(
                        ("lyric_active", index),
                        Animation::new(LYRIC_ANIMATION).with_easing(crate::motion::apple_damping_spring()),
                        |label, delta| label.opacity(0.80 + 0.20 * delta),
                    )
                    .into_any_element()
            } else {
                label.opacity(lyric_fade(distance)).into_any_element()
            };

            body = body.child(
                div()
                    .id(("lyric_line", index))
                    .role(Role::Button)
                    .cursor_pointer()
                    .w_full()
                    .py(px(4.0))
                    .flex()
                    .items_center()
                    .on_click(cx.listener(move |this, _event, _window, _cx| {
                        this.seek_to(time_ms);
                    }))
                    .child(label),
            );
        }

        // 背景色由整页的调色盘渐变提供，这里不再盖不透明的上下渐变蒙版：
        // 那两条与底层光晕不同色的矩形会硬切背景，看起来就像浮在页面上的白框
        div()
            .id("lyrics-scroll")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .relative()
            .child(body.with_animation(
                ("lyrics-scroll-follow", self.followed.unwrap_or(usize::MAX)),
                Animation::new(LYRIC_ANIMATION).with_easing(crate::motion::fluid_scroll_spring()),
                move |body, delta| {
                    if delta < 1.0 || !settled.get() {
                        scroll.set_offset(point(px(0.0), px(from + (to - from) * delta)));
                        settled.set(delta >= 1.0);
                    }
                    body
                },
            ))
            .into_any_element()
    }

    /// 渲染大尺寸封面与自适应柔光漫反射舞台
    fn vinyl_turntable_stage(&self, is_playing: bool) -> impl IntoElement {
        let title_label = self.title.as_ref();
        let glow_color = self.palette.glow;

        div()
            .id("vinyl-turntable-container")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(px(320.0))
                    .h(px(320.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    // 封面边缘微弱同色柔光漫反射投影（随呼吸舒缓律动）
                    .child({
                        let ambient_glow = div()
                            .absolute()
                            .size(px(350.0))
                            .rounded(px(32.0))
                            .bg(glow_color)
                            .shadow_2xl();
                        if is_playing {
                            ambient_glow
                                .with_animation(
                                    "ambient_glow",
                                    Animation::new(Duration::from_millis(3200))
                                        .repeat()
                                        .with_easing(pulsating_between(0.18, 0.42)),
                                    gpui_kit::Styled::opacity,
                                )
                                .into_any_element()
                        } else {
                            ambient_glow.opacity(0.18).into_any_element()
                        }
                    })
                    // 大尺寸质感封面（圆角柔和投影）
                    .child(
                        div()
                            .size(px(320.0))
                            .rounded(px(16.0))
                            .overflow_hidden()
                            .shadow_2xl()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(song_cover(self.cover.as_ref(), title_label, 320.0)),
                    ),
            )
    }
}

impl Render for LyricsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (position, is_playing) = {
            let model = self.player.read(cx);
            (
                model.snapshot.position_ms,
                model.snapshot.state == lyra_audio::State::Playing,
            )
        };
        let current = lyric_index(&self.lines, position);
        self.schedule_follow(current, window, cx);
        let shell_exit = self.shell.clone();
        // 当前曲目的真实最高音质（Hi-Res / SQ / 不展示）
        let quality = self
            .current_hash
            .as_deref()
            .and_then(|hash| self.player.read(cx).quality_of(hash))
            .unwrap_or_default();

        let turntable_stage = self.vinyl_turntable_stage(is_playing);
        let lyric_stream = self.lyric_stream(current, cx);

        // 曲目信息与歌词主体（彻底移除违和的白色半透明矩形与生硬边界）
        let right_lyrics_section = v_flex()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .pl_8()
            .pr_12()
            .py_10()
            .justify_center()
            .child(
                v_flex()
                    .w_full()
                    .gap(px(4.0))
                    .pb_4()
                    .child(
                        h_flex()
                            .gap(px(10.0))
                            .items_center()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(TEXT_PRIMARY)
                                    .truncate()
                                    .child(self.title.clone()),
                            )
                            .child(quality_badge(quality)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(TEXT_MUTED)
                            .truncate()
                            .child(self.singer.clone()),
                    ),
            )
            .child(lyric_stream);

        // 自适应动态弥散光晕底层（Ambient Mesh Blur）：大半径双核柔和漫射球，彻底告别脏色暗块
        let mesh_glow_1 = div()
            .absolute()
            .top(px(-80.0))
            .left(px(30.0))
            .size(px(520.0))
            .rounded_full()
            .bg(self.palette.primary)
            .shadow_2xl();

        let mesh_glow_2 = div()
            .absolute()
            .bottom(px(-70.0))
            .right(px(80.0))
            .size(px(480.0))
            .rounded_full()
            .bg(self.palette.secondary)
            .shadow_2xl();

        // 沉浸页顶层容器：微光浅渐变透亮底色 + 自适应双核弥散光晕
        div()
            .size_full()
            .bg(self.palette.surface_tint)
            .relative()
            .overflow_hidden()
            .flex()
            .flex_row()
            .items_center()
            .child(mesh_glow_1)
            .child(mesh_glow_2)
            .child(turntable_stage)
            .child(right_lyrics_section)
            // 返回按钮
            .child(
                div()
                    .absolute()
                    .top(px(18.0))
                    .left(px(20.0))
                    .child(
                        Button::new("lyrics_back_btn")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronLeft)
                            .tooltip("返回主界面")
                            .on_click(move |_event, _window, cx| {
                                shell_exit.update(cx, crate::shell::AppShell::close_lyrics).ok();
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::pedantic)]

    use gpui_kit::{Bounds, Pixels, point, px, size};

    use super::center_offset;

    fn bounds(top: f32, height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.0), px(top)),
            size: size(px(300.0), px(height)),
        }
    }

    #[test]
    fn center_offset_puts_item_middle_on_viewport_middle() {
        let viewport = bounds(100.0, 400.0);
        assert_eq!(center_offset(viewport, bounds(300.0, 40.0)), px(-20.0));
        assert_eq!(center_offset(viewport, bounds(200.0, 40.0)), px(80.0));
    }
}
