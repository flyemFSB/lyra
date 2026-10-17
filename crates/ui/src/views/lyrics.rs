//! 沉浸式透明彩胶唱机与动态滚动歌词视图。

use std::sync::Arc;

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
    _sub: Subscription,
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
        cx.notify();
    }

    /// 设置歌词行列表
    pub(crate) fn set_lines(&mut self, result: Result<Vec<LyricLine>, String>, cx: &mut Context<Self>) {
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

    /// 跳转至指定播放进度（毫秒）
    fn seek_to(&self, time_ms: u64) {
        if let Some(engine) = self.backend.engine() {
            engine.seek(time_ms);
        }
    }

    /// 刷新当前曲目标题与歌手
    fn refresh_title(&mut self, cx: &Context<Self>) {
        let (hash, title, singer) = self.player.read(cx).current_labels();
        if hash == self.current_hash {
            return;
        }
        self.current_hash = hash;
        self.title = title;
        self.singer = singer;
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
                    // 外圈辉光
                    .child(
                        div()
                            .absolute()
                            .size(px(380.0))
                            .rounded_full()
                            .bg(hsla(202.0 / 360.0, 0.95, 0.88, 0.45))
                            .shadow_xl(),
                    )
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
                    // 唱臂与唱针组件
                    .child(
                        div()
                            .absolute()
                            .top(if is_playing { px(12.0) } else { px(8.0) })
                            .right(if is_playing { px(24.0) } else { px(16.0) })
                            .w(px(90.0))
                            .h(px(220.0))
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (position, is_playing) = {
            let model = self.player.read(cx);
            (
                model.snapshot.position_ms,
                model.snapshot.state == lyra_audio::State::Playing,
            )
        };
        let current = lyric_index(&self.lines, position);
        let shell_exit = self.shell.clone();

        // 唱片区域
        let turntable_stage = self.vinyl_turntable_stage(is_playing, cx);

        // 歌词流区域
        let lyric_stream = if let Some(status) = &self.status {
            notice_with_icon(IconName::CircleAlert, status.clone(), cx)
        } else if self.lines.is_empty() {
            notice_with_icon(IconName::Mic, "暂无歌词（播放歌曲后将自动匹配歌词写真）", cx)
        } else {
            let mut list = v_flex().w_full().gap(px(12.0)).items_start();

            for (index, line) in self.lines.iter().enumerate() {
                let active = current == Some(index);
                let text = self.texts.get(index).cloned().unwrap_or_default();
                let time_ms = line.time_ms;

                list = list.child(
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
                        .child(
                            div()
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
                                .child(text),
                        ),
                );
            }

            div()
                .id("lyrics-scroll-body")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .pt(px(20.0))
                .pb(px(80.0))
                .overflow_y_scrollbar()
                .child(list)
                .into_any_element()
        };

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
                                shell_exit.update(cx, AppShell::go_back).ok();
                            }),
                    ),
            )
    }
}
