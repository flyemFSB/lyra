//! 系统设置与账号管理视图。

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::VipStatus;

use crate::bridge::{Backend, Msg};
use crate::format::{QUALITY_CHOICES, format_bytes, quality_label};
use crate::theme::{
    ACCENT_GREEN, ACCENT_GREEN_HOVER, ACCENT_GREEN_LIGHT, CARD_BG, CARD_BORDER, TEXT_FAINT, TEXT_MUTED, TEXT_PRIMARY,
    VIP_GOLD, badge, hex_rgb, title,
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
        let cache_dir = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("留空使用默认系统音乐缓存目录")
                .default_value(cache_dir_value)
        });
        Self {
            backend,
            quality,
            cache_dir,
            max_cache_bytes,
            vip: None,
            loaded: false,
            last_action: None,
        }
    }

    /// 确保会员状态已加载
    pub(crate) fn ensure(&mut self) {
        if self.loaded {
            return;
        }
        if !self.backend.app().is_logged_in() {
            self.last_action = Some(SharedString::new_static("登录后可查看会员状态"));
            return;
        }
        self.loaded = true;
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Vip(app.vip().await.map_err(|error| error.to_string())) });
    }

    /// 处理会员状态响应结果
    pub(crate) fn on_vip(&mut self, result: Result<VipStatus, String>, cx: &mut Context<Self>) {
        match result {
            Ok(vip) => self.vip = Some(vip),
            Err(error) => self.last_action = Some(error.into()),
        }
        cx.notify();
    }

    /// 处理领取 VIP 响应结果
    pub(crate) fn on_claim(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        match result {
            Ok(()) => {
                self.last_action = Some(SharedString::new_static("已成功领取今日概念版畅听 VIP"));
                self.loaded = false;
                self.ensure();
            }
            Err(error) => self.last_action = Some(SharedString::from(format!("领取失败: {error}"))),
        }
        cx.notify();
    }

    /// 处理设置保存响应结果
    pub(crate) fn on_saved(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.last_action = Some(match result {
            Ok(()) => SharedString::new_static("设置已成功保存"),
            Err(error) => SharedString::from(format!("保存失败: {error}")),
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
                app.settings()
                    .and_then(|mut settings| {
                        settings.quality = chosen;
                        app.set_settings(&settings)
                    })
                    .map_err(|error| error.to_string()),
            )
        });
        cx.notify();
    }

    /// 保存自定义缓存目录
    fn save_cache_dir(&mut self, cx: &mut Context<Self>) {
        let value = self.cache_dir.read(cx).value().to_string();
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::SettingsSaved(
                app.settings()
                    .and_then(|mut settings| {
                        settings.cache_dir = if value.trim().is_empty() { None } else { Some(value) };
                        app.set_settings(&settings)
                    })
                    .map_err(|error| error.to_string()),
            )
        });
    }

    /// 设置最大缓存容量并保存
    fn set_cache_limit(&mut self, bytes: u64, cx: &mut Context<Self>) {
        self.max_cache_bytes = bytes;
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::SettingsSaved(
                app.settings()
                    .and_then(|mut settings| {
                        settings.max_cache_bytes = bytes;
                        app.set_settings(&settings)
                    })
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
            app.logout()
                .map_or_else(|error| Msg::Toast(format!("登出失败: {error}")), |()| Msg::LoggedOut)
        });
    }

    /// 清理内存接口缓存
    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        self.backend.app().clear_cache();
        self.last_action = Some(SharedString::new_static("已清理接口缓存"));
        cx.notify();
    }

    /// 获取会员状态展示文案
    fn vip_label(&self) -> &'static str {
        match &self.vip {
            Some(vip) if vip.is_concept_active() => "畅听 VIP 权益生效中",
            Some(_) => "未开通畅听 VIP（点击右侧一键领取）",
            None => "会员状态获取中…",
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 音质偏好选择
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
        quality_row = quality_row.child(badge(
            format!("当前: {}", quality_label(self.quality.as_str())),
            ACCENT_GREEN_LIGHT,
            ACCENT_GREEN_HOVER,
        ));

        // VIP 权益卡片
        let is_vip_active = self
            .vip
            .as_ref()
            .is_some_and(lyra_api::model::VipStatus::is_concept_active);
        let vip_card = h_flex()
            .w_full()
            .p_4()
            .rounded(px(12.0))
            .bg(hex_rgb(0xfffbeb))
            .border_1()
            .border_color(hex_rgb(0xfde68a))
            .shadow_sm()
            .items_center()
            .gap(px(16.0))
            .child(
                div()
                    .w(px(44.0))
                    .h(px(44.0))
                    .rounded_full()
                    .bg(hex_rgb(0xfef3c7))
                    .border_1()
                    .border_color(hex_rgb(0xfcd34d))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Crown).size(px(22.0)).text_color(VIP_GOLD)),
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
                                    .text_color(TEXT_PRIMARY)
                                    .child("酷狗概念版畅听 VIP"),
                            )
                            .child(badge(
                                if is_vip_active { "有效" } else { "未开通" },
                                if is_vip_active {
                                    ACCENT_GREEN_LIGHT
                                } else {
                                    hex_rgb(0xfee2e2)
                                },
                                if is_vip_active {
                                    ACCENT_GREEN_HOVER
                                } else {
                                    hex_rgb(0xef4444)
                                },
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(TEXT_MUTED)
                            .child(SharedString::from(self.vip_label())),
                    ),
            )
            .child(
                Button::new("claim_vip")
                    .primary()
                    .icon(IconName::Sparkles)
                    .label("一键免费领取 VIP")
                    .tooltip("每日可领取一次概念版畅听权益")
                    .on_click(cx.listener(|view, _event, _window, cx| view.claim_vip(cx))),
            );

        // 缓存容量预设
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
        preset_row = preset_row.child(badge(
            format!("当前: {}", format_bytes(self.max_cache_bytes)),
            ACCENT_GREEN_LIGHT,
            ACCENT_GREEN_HOVER,
        ));

        // 缓存路径
        let cache_row = h_flex()
            .gap(px(10.0))
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .px_3()
                    .py(px(4.0))
                    .rounded(px(10.0))
                    .bg(hex_rgb(0xf2f4f7))
                    .border_1()
                    .border_color(CARD_BORDER)
                    .gap(px(8.0))
                    .items_center()
                    .child(Icon::new(IconName::HardDrive).size(px(16.0)).text_color(ACCENT_GREEN))
                    .child(div().flex_1().child(Input::new(&self.cache_dir))),
            )
            .child(
                Button::new("save_cache_dir")
                    .primary()
                    .small()
                    .icon(IconName::Check)
                    .label("保存路径")
                    .on_click(cx.listener(|view, _event, _window, cx| view.save_cache_dir(cx))),
            );

        // 账号与维护
        let account_row = h_flex()
            .gap(px(10.0))
            .items_center()
            .child(
                Button::new("clear_cache")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label("清理内存接口缓存")
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

        // 操作状态反馈条
        let status = self.last_action.clone().map(|text| {
            h_flex()
                .w_full()
                .px_5()
                .py_2()
                .items_center()
                .gap(px(6.0))
                .border_t_1()
                .border_color(CARD_BORDER)
                .bg(hex_rgb(0xf9fafb))
                .child(Icon::new(IconName::Sparkles).size(px(14.0)).text_color(ACCENT_GREEN))
                .child(div().text_xs().text_color(TEXT_MUTED).child(text))
                .into_any_element()
        });

        v_flex()
            .size_full()
            .child(
                div()
                    .w_full()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(CARD_BORDER)
                    .child(title("设置", cx)),
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
                    ),
            )
            .children(status)
    }
}
