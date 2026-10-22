//! 系统设置与账号管理视图。

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::VipStatus;

use crate::bridge::{Backend, Msg};
use crate::format::QUALITY_CHOICES;
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_BORDER, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CANVAS_BG, CARD_BG, CARD_BORDER,
    TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY, VIP_GOLD_BG, VIP_GOLD_BORDER, VIP_GOLD_ICON_BG, VIP_GOLD_ICON_FG,
    VIP_GOLD_MUTED, VIP_GOLD_TEXT, badge, hex_rgb, title,
};

/// 缓存容量预设选项
const CACHE_PRESETS: &[(&str, u64)] = &[
    ("512 MiB", 512 * 1024 * 1024),
    ("1 GiB", 1 << 30),
    ("2 GiB", 2 << 30),
    ("4 GiB", 4 << 30),
];

/// 默认最大缓存字节数
const DEFAULT_MAX_CACHE_BYTES: u64 = CACHE_PRESETS[2].1;

/// 设置视图
pub(crate) struct SettingsView {
    backend: Backend,
    quality: String,
    cache_dir: Entity<InputState>,
    saved_cache_dir: String,
    max_cache_bytes: u64,
    vip: Option<VipStatus>,
    loaded: bool,
    /// 最近一次操作的状态提示
    last_action: Option<SharedString>,
}

impl SettingsView {
    /// 创建设置视图
    pub(crate) fn new(backend: Backend, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = backend.app().settings().ok();
        let quality = settings
            .as_ref()
            .map_or_else(|| "flac".to_owned(), |value| value.quality.clone());
        let cache_dir_value = settings
            .as_ref()
            .and_then(|value| value.cache_dir.clone())
            .unwrap_or_default();
        let max_cache_bytes = settings
            .as_ref()
            .map_or(DEFAULT_MAX_CACHE_BYTES, |value| value.max_cache_bytes);
        let saved_cache_dir = cache_dir_value.clone();
        let cache_dir = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("留空使用默认系统音乐缓存目录")
                .default_value(cache_dir_value)
        });
        Self {
            backend,
            quality,
            cache_dir,
            saved_cache_dir,
            max_cache_bytes,
            vip: None,
            loaded: false,
            last_action: None,
        }
    }

    /// 确保会员状态已加载
    pub(crate) fn ensure(&mut self, cx: &mut Context<Self>) {
        if self.loaded {
            return;
        }
        if !self.backend.app().is_logged_in() {
            // 必须显式通知：未置脏的实体会复用上一帧的渲染结果，提示不会显示
            self.last_action = Some(SharedString::new_static("登录后可查看会员状态"));
            cx.notify();
            return;
        }
        self.loaded = true;
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Vip(app.vip().await.map_err(|error| error.to_string())) });
    }

    /// 重新加载会员状态
    #[allow(dead_code)]
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.loaded = false;
        self.ensure(cx);
    }

    /// 处理会员状态响应结果
    pub(crate) fn on_vip(&mut self, result: Result<VipStatus, String>, cx: &mut Context<Self>) {
        match result {
            Ok(vip) => self.vip = Some(vip),
            Err(error) => {
                tracing::warn!(%error, "获取 VIP 状态失败");
                self.last_action = Some(SharedString::new_static("获取会员状态失败，请稍后重试"));
            }
        }
        cx.notify();
    }

    /// 处理领取 VIP 响应结果
    pub(crate) fn on_claim(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        match result {
            Ok(()) => {
                self.last_action = Some(SharedString::new_static("已成功领取今日畅听 VIP"));
                self.backend.send(Msg::Toast("已成功领取今日畅听 VIP".to_owned()));
                self.backend.app().clear_cache();
                self.loaded = false;
                self.ensure(cx);
            }
            Err(error) => {
                tracing::warn!(%error, "领取 VIP 失败");
                self.last_action = Some(SharedString::new_static("网络连接不稳定，请稍后重试"));
                self.backend
                    .send(Msg::Toast("领取失败: 网络连接不稳定，请稍后重试".to_owned()));
            }
        }
        cx.notify();
    }

    /// 处理设置保存响应结果
    pub(crate) fn on_saved(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.last_action = Some(match result {
            Ok(()) => SharedString::new_static("设置已成功保存"),
            Err(error) => {
                tracing::warn!(%error, "保存设置失败");
                self.backend.send(Msg::Toast("保存设置失败，请稍后重试".to_owned()));
                SharedString::new_static("保存失败，请稍后重试")
            }
        });
        cx.notify();
    }

    /// 修改音质偏好并持久化
    fn set_quality(&mut self, quality: &str, cx: &mut Context<Self>) {
        quality.clone_into(&mut self.quality);
        let app = self.backend.app();
        let chosen = self.quality.clone();
        self.backend.spawn(async move {
            Msg::SettingsSaved(
                app.update_settings(|settings| settings.quality = chosen)
                    .map_err(|error| error.to_string()),
            )
        });
        cx.notify();
    }

    /// 保存自定义缓存目录
    fn save_cache_dir(&mut self, cx: &mut Context<Self>) {
        let value = self.cache_dir.read(cx).value().to_string();
        self.saved_cache_dir.clone_from(&value);
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::SettingsSaved(
                app.update_settings(|settings| {
                    settings.cache_dir = if value.trim().is_empty() { None } else { Some(value) };
                })
                .map_err(|error| error.to_string()),
            )
        });
        cx.notify();
    }

    /// 设置最大缓存容量并保存
    fn set_cache_limit(&mut self, bytes: u64, cx: &mut Context<Self>) {
        self.max_cache_bytes = bytes;
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::SettingsSaved(
                app.update_settings(|settings| settings.max_cache_bytes = bytes)
                    .map_err(|error| error.to_string()),
            )
        });
        cx.notify();
    }

    /// 申请领取每日畅听 VIP
    fn claim_vip(&mut self, cx: &mut Context<Self>) {
        self.last_action = Some(SharedString::new_static("正在申请领取…"));
        cx.notify();
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::VipClaim(
                app.claim_daily_vip()
                    .await
                    .map(|_claim| ())
                    .map_err(|error| error.to_string()),
            )
        });
    }

    /// 退出当前账号登录
    fn logout(&mut self) {
        let app = self.backend.app();
        self.backend.spawn(async move {
            match app.logout().await {
                Ok(()) => Msg::LoggedOut,
                Err(error) => Msg::Toast(format!("登出失败: {error}")),
            }
        });
    }

    /// 清理网络临时缓存
    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        self.backend.app().clear_cache();
        self.last_action = Some(SharedString::new_static("已清理网络临时缓存"));
        self.backend.send(Msg::Toast("网络临时缓存已成功清除".to_owned()));
        cx.notify();
    }

    /// 获取会员状态展示文案
    fn vip_label(&self) -> String {
        match &self.vip {
            Some(vip) if vip.is_concept_active() => {
                if let Some(exp) = vip.concept_expires_str() {
                    format!("今日畅听 VIP 权益生效中（自然日有效，有效期至 {exp}）")
                } else {
                    "今日畅听 VIP 权益生效中（自然日内有效）".to_string()
                }
            }
            Some(_) => "每日可领取一次畅听权益（自然日内有效，享无损高品质音乐）".to_string(),
            None => "会员状态同步中…".to_string(),
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 音质偏好选择（点击选中即可体现状态，去除多余标签）
        let mut quality_row = h_flex().gap(px(8.0)).flex_wrap().items_center();
        for (id, label) in QUALITY_CHOICES.iter().copied() {
            let active = self.quality == id;
            quality_row = quality_row.child(
                Button::new(id)
                    .small()
                    .label(label)
                    .when(active, ButtonVariants::primary)
                    .when(!active, ButtonVariants::ghost)
                    .on_click(cx.listener(move |view, _event, _window, cx| view.set_quality(id, cx))),
            );
        }

        // VIP 权益卡片（低饱和度浅金/暖灰微渐变底色，搭配深金褐色文字与极细柔和边框，层级收敛且具品质感）
        let is_vip_active = self
            .vip
            .as_ref()
            .is_some_and(lyra_api::model::VipStatus::is_concept_active);
        let vip_card = h_flex()
            .w_full()
            .p_4()
            .rounded(px(12.0))
            .bg(VIP_GOLD_BG)
            .border_1()
            .border_color(VIP_GOLD_BORDER)
            .shadow_xs()
            .items_center()
            .gap(px(14.0))
            .child(
                div()
                    .w(px(40.0))
                    .h(px(40.0))
                    .rounded_full()
                    .bg(VIP_GOLD_ICON_BG)
                    .border_1()
                    .border_color(hex_rgb(0xfde68a))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Crown).size(px(20.0)).text_color(VIP_GOLD_ICON_FG)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(2.0))
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(VIP_GOLD_TEXT)
                                    .child("畅听 VIP"),
                            )
                            .child(badge(
                                if is_vip_active {
                                    "权益生效中"
                                } else {
                                    "今日未领取"
                                },
                                if is_vip_active {
                                    hex_rgb(0xfef3c7)
                                } else {
                                    hex_rgb(0xf3f4f6)
                                },
                                if is_vip_active {
                                    hex_rgb(0xb45309)
                                } else {
                                    hex_rgb(0x6b7280)
                                },
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(VIP_GOLD_MUTED)
                            .child(SharedString::from(self.vip_label())),
                    ),
            )
            .child(
                Button::new("claim_vip")
                    .when(is_vip_active, |b| {
                        b.ghost()
                            .disabled(true)
                            .icon(IconName::Sparkles)
                            .label("今日已领取")
                            .tooltip("今日畅听 VIP 权益已生效")
                    })
                    .when(!is_vip_active, |b| {
                        b.primary()
                            .icon(IconName::Sparkles)
                            .label("一键免费领取")
                            .tooltip("每日可领取一次畅听权益")
                            .on_click(cx.listener(|view, _event, _window, cx| view.claim_vip(cx)))
                    }),
            );

        // 缓存容量预设（选项激活态即语义表达，无需冗余文本）
        let mut preset_row = h_flex().gap(px(8.0)).flex_wrap().items_center();
        for (label, bytes) in CACHE_PRESETS.iter().copied() {
            let active = self.max_cache_bytes == bytes;
            preset_row = preset_row.child(
                Button::new(label)
                    .small()
                    .label(label)
                    .when(active, ButtonVariants::primary)
                    .when(!active, ButtonVariants::ghost)
                    .on_click(cx.listener(move |view, _event, _window, cx| view.set_cache_limit(bytes, cx))),
            );
        }

        // 缓存路径（内置操作按钮 Inside Action Button：未保存时高亮，已保存时转为灰色只读状态）
        let current_cache_dir_val = self.cache_dir.read(cx).value().to_string();
        let is_path_modified = current_cache_dir_val != self.saved_cache_dir;

        let inside_action_btn = if is_path_modified {
            Button::new("save_cache_dir")
                .primary()
                .small()
                .icon(IconName::Check)
                .label("保存")
                .tooltip("保存更改的路径")
                .on_click(cx.listener(|view, _event, _window, cx| view.save_cache_dir(cx)))
                .into_any_element()
        } else {
            h_flex()
                .gap(px(3.0))
                .items_center()
                .px_2()
                .py(px(2.0))
                .text_xs()
                .text_color(TEXT_FAINT)
                .child(Icon::new(IconName::Check).size(px(13.0)))
                .child("已保存")
                .into_any_element()
        };

        let cache_row = h_flex()
            .w_full()
            .px_3()
            .py(px(4.0))
            .rounded(px(8.0))
            .bg(CANVAS_BG)
            .border_1()
            .border_color(if is_path_modified {
                ACCENT_GREEN_BORDER
            } else {
                CARD_BORDER
            })
            .gap(px(8.0))
            .items_center()
            .child(
                Icon::new(IconName::Folder)
                    .size(px(16.0))
                    .text_color(if is_path_modified { ACCENT_GREEN } else { TEXT_MUTED }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(Input::new(&self.cache_dir).appearance(false)),
            )
            .child(inside_action_btn);

        // 账号与维护
        let account_row = h_flex()
            .gap(px(10.0))
            .items_center()
            .child(
                Button::new("clear_cache")
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .label("清除网络缓存")
                    .tooltip("清除网络临时接口缓存，不影响已下载曲目")
                    .on_click(cx.listener(|view, _event, _window, cx| view.clear_cache(cx))),
            )
            .child(
                Button::new("logout")
                    .danger()
                    .small()
                    .icon(IconName::LogOut)
                    .label("退出当前登录")
                    .on_click(cx.listener(|view, _event, _window, _cx| view.logout())),
            );

        v_flex()
            .size_full()
            .child(
                div()
                    .w_full()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(CARD_BORDER)
                    .child(title("设置")),
            )
            .child(
                v_flex()
                    .id("settings-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .p_5()
                    .gap(px(16.0))
                    .overflow_y_scroll()
                    .child(vip_card)
                    .child(
                        v_flex()
                            .p_4()
                            .rounded(px(12.0))
                            .bg(CARD_BG)
                            .border_1()
                            .border_color(CARD_BORDER)
                            .shadow_sm()
                            .gap(px(10.0))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::SlidersHorizontal)
                                            .size(px(16.0))
                                            .text_color(ACCENT_GREEN),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(TEXT_PRIMARY)
                                            .child("音质偏好"),
                                    ),
                            )
                            .child(quality_row),
                    )
                    .child(
                        v_flex()
                            .p_4()
                            .rounded(px(12.0))
                            .bg(CARD_BG)
                            .border_1()
                            .border_color(CARD_BORDER)
                            .shadow_sm()
                            .gap(px(12.0))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .child(Icon::new(IconName::HardDrive).size(px(16.0)).text_color(ACCENT_GREEN))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(TEXT_PRIMARY)
                                            .child("本地缓存与存储"),
                                    ),
                            )
                            .child(cache_row)
                            .child(
                                v_flex()
                                    .gap(px(6.0))
                                    .child(div().text_xs().text_color(TEXT_FAINT).child("缓存容量上限"))
                                    .child(preset_row),
                            ),
                    )
                    .child(
                        v_flex()
                            .p_4()
                            .rounded(px(12.0))
                            .bg(CARD_BG)
                            .border_1()
                            .border_color(CARD_BORDER)
                            .shadow_sm()
                            .gap(px(10.0))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .child(Icon::new(IconName::User).size(px(16.0)).text_color(ACCENT_GREEN))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(TEXT_PRIMARY)
                                            .child("账户与维护"),
                                    ),
                            )
                            .child(account_row),
                    )
                    .child(
                        v_flex()
                            .p_4()
                            .rounded(px(12.0))
                            .bg(CARD_BG)
                            .border_1()
                            .border_color(CARD_BORDER)
                            .shadow_sm()
                            .gap(px(12.0))
                            .child(
                                h_flex()
                                    .gap(px(12.0))
                                    .items_center()
                                    .child(crate::logo::lyra_logo_icon(44.0))
                                    .child(
                                        v_flex()
                                            .gap(px(2.0))
                                            .child(
                                                h_flex()
                                                    .gap(px(8.0))
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(TEXT_PRIMARY)
                                                            .child("Lyra音乐"),
                                                    )
                                                    .child(badge("v0.1.0", ACCENT_GREEN_LIGHT, ACCENT_GREEN_HOVER)),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(TEXT_MUTED)
                                                    .child("基于 Rust 与 GPUI 构建的高性能、低开销轻量桌面音乐客户端"),
                                            ),
                                    ),
                            ),
                    ),
            )
    }
}
