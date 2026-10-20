//! 沉浸式透明彩胶唱机与动态滚动歌词视图。

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::LyricLine;

use crate::bridge::Backend;
use crate::format::lyric_index;
use crate::shell::AppShell;
use crate::theme::{CARD_BORDER, badge, cover_box, hex_rgb, notice_with_icon};
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
    _sub: Subscription,
}

/// 歌词滚动与切换动画时长
const LYRIC_ANIMATION: Duration = Duration::from_millis(320);

/// 让 `item` 的竖向中线落在 `viewport` 正中所需的滚动偏移
fn center_offset(viewport: Bounds<Pixels>, item: Bounds<Pixels>) -> Pixels {
    viewport.center().y - item.center().y
}

/// 离当前行越远的歌词越淡
fn lyric_fade(distance: usize) -> f32 {
    match distance {
        0 => 1.0,
        1 => 0.55,
        2 => 0.35,
        _ => 0.2,
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
            current_hash: None,
            status: None,
            scroll: ScrollHandle::new(),
            followed: None,
            scroll_from: 0.0,
            scroll_target: 0.0,
            scroll_settled: Rc::new(Cell::new(true)),
            follow_scheduled: false,
            _sub: sub,
        }
    }

    /// 清空歌词与曲目信息
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.lines = Arc::new(Vec::new());
        self.texts = Arc::new(Vec::new());
        self.title = SharedString::new_static("未在播放");
        self.singer = SharedString::new_static("");
        self.current_hash = None;
        self.status = None;
        self.followed = None;
        cx.notify();
    }

    /// 设置歌词行列表
    pub(crate) fn set_lines(&mut self, result: Result<Vec<LyricLine>, String>, cx: &mut Context<Self>) {
        // 换歌后行高全变了，等新布局出来再重新跟随
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
    ///
    /// 返回值表示状态是否发生变化（用于决定要不要重绘）。
    fn follow_lyrics(&mut self, current: Option<usize>) -> bool {
        if self.followed != current {
            let Some(index) = current else {
                self.followed = None;
                return true;
            };
            // 子元素边界来自上一帧布局，首帧或换歌当帧还没有，下一帧再跟随
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
            // 行边界来自上一帧：当前行放大、窗口缩放都会让目标偏移变，变了就再滚一次
            let target: f32 = center_offset(self.scroll.bounds(), item).into();
            // 亚像素的抖动没必要重新滚
            if (target - self.scroll_target).abs() > 0.5 {
                self.scroll_target = target;
                self.scroll_settled.set(false);
                return true;
            }
        }
        false
    }

    /// 安排下一帧执行跟随计算
    ///
    /// 跟随依赖上一帧的布局结果，属于「渲染之后的测量」，
    /// 必须在 render 之外进行（GPUI 要求 render 不改动自身状态）。
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

    /// 刷新当前曲目标题与歌手
    fn refresh_title(&mut self, cx: &Context<Self>) {
        // 播放器每 180ms 通知一次（进度在变），但歌名几乎不变：
        // 先零分配地比对哈希，确认换歌了才重新格式化标题。
        let changed = {
            let model = self.player.read(cx);
            model.current_hash() != self.current_hash.as_deref()
        };
        if !changed {
            return;
        }
        let (hash, title, singer) = self.player.read(cx).current_labels();
        self.current_hash = hash;
        self.title = title;
        self.singer = singer;
    }

    /// 渲染歌词流：当前行固定在滚动区正中，切换时平滑滚过去并淡入
    fn lyric_stream(&self, current: Option<usize>, cx: &Context<Self>) -> AnyElement {
        if let Some(status) = &self.status {
            return notice_with_icon(IconName::CircleAlert, status.clone(), cx).into_any_element();
        }
        if self.lines.is_empty() {
            return notice_with_icon(IconName::Mic, "暂无歌词（播放歌曲后将自动匹配歌词写真）", cx).into_any_element();
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
            .gap(px(12.0))
            .pt(pad)
            .pb(pad)
            .overflow_y_scroll()
            .track_scroll(&self.scroll);

        for (index, line) in self.lines.iter().enumerate() {
            let active = current == Some(index);
            let text = self.texts.get(index).cloned().unwrap_or_default();
            let time_ms = line.time_ms;

            let label = div()
                .when(active, |this| {
                    this.text_2xl()
                        .font_weight(FontWeight::BOLD)
                        .text_color(hex_rgb(0x0284c7))
                })
                .when(!active, |this| {
                    this.text_base()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(hex_rgb(0x94a3b8))
                        .hover(|s| s.text_color(hex_rgb(0x334155)))
                })
                .child(text);

            // 当前行淡入并轻放大，其余行越远越淡
            let label = if active {
                label
                    .with_animation(
                        ("lyric_active", index),
                        Animation::new(LYRIC_ANIMATION).with_easing(ease_in_out),
                        |label, delta| label.text_size(rems(1.25 + 0.25 * delta)).opacity(delta),
                    )
                    .into_any_element()
            } else {
                label
                    .opacity(lyric_fade(
                        current.map_or(usize::MAX, |current| index.abs_diff(current)),
                    ))
                    .into_any_element()
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
                    .gap(px(10.0))
                    .on_click(cx.listener(move |this, _event, _window, _cx| {
                        this.seek_to(time_ms);
                    }))
                    .child(label),
            );
        }

        div()
            .id("lyrics-scroll")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .relative()
            .child(body.with_animation(
                ("lyrics-scroll-follow", self.followed.unwrap_or(usize::MAX)),
                Animation::new(LYRIC_ANIMATION).with_easing(ease_in_out),
                move |body, delta| {
                    // 落位后不再干预偏移，把滚动交还给用户
                    if delta < 1.0 || !settled.get() {
                        scroll.set_offset(point(px(0.0), px(from + (to - from) * delta)));
                        settled.set(delta >= 1.0);
                    }
                    body
                },
            ))
            .vertical_scrollbar(&self.scroll)
            .into_any_element()
    }

    /// 渲染黑胶唱机拟态舞台
    fn vinyl_turntable_stage(&self, is_playing: bool, cx: &Context<Self>) -> impl IntoElement {
        let title_label = self.title.as_ref();

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
                    .w(px(380.0))
                    .h(px(380.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    // 外圈辉光（播放中自然呼吸微光）
                    .child({
                        let glow = div()
                            .absolute()
                            .size(px(380.0))
                            .rounded_full()
                            .bg(hsla(202.0 / 360.0, 0.95, 0.88, 0.45))
                            .shadow_xl();
                        if is_playing {
                            glow.with_animation(
                                "vinyl_glow",
                                Animation::new(Duration::from_millis(2400))
                                    .repeat()
                                    .with_easing(pulsating_between(0.25, 0.65)),
                                gpui_kit::Styled::opacity,
                            )
                            .into_any_element()
                        } else {
                            glow.into_any_element()
                        }
                    })
                    // 彩胶外环
                    .child(
                        div()
                            .size(px(340.0))
                            .rounded_full()
                            .bg(hsla(204.0 / 360.0, 0.85, 0.62, 0.45))
                            .border_2()
                            .border_color(hsla(200.0 / 360.0, 0.95, 0.80, 0.65))
                            .shadow_lg()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(280.0))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(hsla(200.0 / 360.0, 0.90, 0.85, 0.35))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .size(px(220.0))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(hsla(200.0 / 360.0, 0.85, 0.90, 0.40))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            // 唱片中心封面
                                            .child(
                                                div()
                                                    .size(px(160.0))
                                                    .rounded_full()
                                                    .overflow_hidden()
                                                    .border_2()
                                                    .border_color(hex_rgb(0xffffff))
                                                    .shadow_md()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(cover_box(title_label, 160.0, cx)),
                                            ),
                                    ),
                            ),
                    )
                    // 中心轴芯
                    .child(
                        div()
                            .absolute()
                            .size(px(26.0))
                            .rounded_full()
                            .bg(hex_rgb(0xffffff))
                            .border_2()
                            .border_color(hex_rgb(0xcfd8dc))
                            .shadow_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(8.0))
                                    .rounded_full()
                                    .bg(hex_rgb(0x475569)),
                            ),
                    )
                    // 唱臂与唱针组件（播放/暂停平滑切入与复位）
                    .child(
                        div()
                            .absolute()
                            .w(px(90.0))
                            .h(px(220.0))
                            .with_animation(
                                if is_playing { "tonearm_playing" } else { "tonearm_paused" },
                                Animation::new(Duration::from_millis(360))
                                    .with_easing(gpui_kit::base::animation::ease_out_cubic),
                                move |this, delta| {
                                    let (start_top, target_top) = if is_playing { (8.0, 12.0) } else { (12.0, 8.0) };
                                    let (start_right, target_right) = if is_playing { (16.0, 24.0) } else { (24.0, 16.0) };
                                    this.top(px(start_top + (target_top - start_top) * delta))
                                        .right(px(start_right + (target_right - start_right) * delta))
                                },
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .size(px(32.0))
                                    .rounded_full()
                                    .bg(hex_rgb(0xffffff))
                                    .border_1()
                                    .border_color(hex_rgb(0xe2e8f0))
                                    .shadow_md()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .size(px(12.0))
                                            .rounded_full()
                                            .bg(hex_rgb(0x94a3b8)),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(26.0))
                                    .right(px(12.0))
                                    .w(px(6.0))
                                    .h(px(140.0))
                                    .rounded_full()
                                    .bg(hex_rgb(0xf1f5f9))
                                    .border_1()
                                    .border_color(hex_rgb(0xe2e8f0))
                                    .shadow_sm(),
                            )
                            // 唱头与唱针
                            .child(
                                div()
                                    .absolute()
                                    .top(px(154.0))
                                    .right(if is_playing { px(18.0) } else { px(10.0) })
                                    .w(px(26.0))
                                    .h(px(16.0))
                                    .rounded(px(5.0))
                                    .bg(hex_rgb(0xffffff))
                                    .border_1()
                                    .border_color(hex_rgb(0xcfd8dc))
                                    .shadow_md()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .w(px(3.0))
                                            .h(px(8.0))
                                            .rounded_full()
                                            .bg(hex_rgb(0x0284c7)),
                                    ),
                            ),
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
        // 跟随计算依赖上一帧的布局，放到下一帧执行（render 内不改状态）
        self.schedule_follow(current, window, cx);
        let shell_exit = self.shell.clone();

        // 唱片区域
        let turntable_stage = self.vinyl_turntable_stage(is_playing, cx);

        // 歌词流区域
        let lyric_stream = self.lyric_stream(current, cx);

        // 曲目信息与歌词主体
        let right_lyrics_section = v_flex()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .pl_8()
            .pr_12()
            .py_10()
            .justify_center()
            // 曲名与歌手
            .child(
                v_flex()
                    .w_full()
                    .gap(px(6.0))
                    .pb_4()
                    .border_b_1()
                    .border_color(CARD_BORDER)
                    .child(
                        h_flex()
                            .gap(px(10.0))
                            .items_baseline()
                            .child(
                                div()
                                    .text_3xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(hex_rgb(0x0f172a))
                                    .truncate()
                                    .child(self.title.clone()),
                            )
                            .child(badge("SQ 无损母带", hex_rgb(0xf0fdf4), hex_rgb(0x16a34a))),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hex_rgb(0x64748b))
                            .truncate()
                            .child(self.singer.clone()),
                    ),
            )
            .child(lyric_stream);

        // 主容器
        div()
            .size_full()
            .bg(hex_rgb(0xfcfdfe))
            .relative()
            .flex()
            .flex_row()
            .items_center()
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

    // 避免与 gpui_kit::* 宏冲突
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
        // 行中线 320 在视口中线 300 下方 → 向上滚 20
        assert_eq!(center_offset(viewport, bounds(300.0, 40.0)), px(-20.0));
        // 行中线 220 在视口中线 300 上方 → 向下滚 80
        assert_eq!(center_offset(viewport, bounds(200.0, 40.0)), px(80.0));
    }
}
