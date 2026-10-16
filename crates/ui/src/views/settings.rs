//! 系统设置与账户管理视图组件。

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lyra_api::model::VipStatus;

use crate::bridge::{Backend, Msg};
use crate::format::{QUALITY_CHOICES, format_bytes, quality_label};
use crate::theme::{ROW_HEIGHT, title};

/// 缓存容量预设选项。
const CACHE_PRESETS: &[(&str, u64)] = &[
    ("512 MiB", 512 * 1024 * 1024),
    ("1 GiB", 1 << 30),
    ("2 GiB", 2 << 30),
    ("4 GiB", 4 << 30),
];

/// 默认最大缓存字节数（取预设中的 2 GiB，避免与 `CACHE_PRESETS` 重复维护）。
const DEFAULT_MAX_CACHE_BYTES: u64 = CACHE_PRESETS[2].1;

/// 设置视图状态结构。
pub(crate) struct SettingsView {
    backend: Backend,
    quality: String,
    cache_dir: Entity<InputState>,
    max_cache_bytes: u64,
    vip: Option<VipStatus>,
    loaded: bool,
    /// 最近一次操作的提示反馈信息。
    last_action: Option<SharedString>,
}

impl SettingsView {
    /// 创建设置视图实例。
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
                .placeholder("留空使用默认缓存目录")
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

    /// 首次进入视图时触发会员信息查询。
    pub(crate) fn ensure(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::Vip(app.vip().await.map_err(|error| error.to_string())) });
    }

    /// 响应处理会员状态数据。
    pub(crate) fn on_vip(&mut self, result: Result<VipStatus, String>, cx: &mut Context<Self>) {
        match result {
            Ok(vip) => self.vip = Some(vip),
            Err(error) => self.last_action = Some(error.into()),
        }
        cx.notify();
    }

    /// 响应处理领取 VIP 结果。
    pub(crate) fn on_claim(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        match result {
            Ok(()) => {
                self.last_action = Some(SharedString::new_static("已领取每日畅听 VIP"));
                self.loaded = false;
                self.ensure();
            }
            Err(error) => self.last_action = Some(SharedString::from(format!("领取失败: {error}"))),
        }
        cx.notify();
    }

    /// 响应处理设置保存结果。
    pub(crate) fn on_saved(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.last_action = Some(match result {
            Ok(()) => SharedString::new_static("设置已保存"),
            Err(error) => SharedString::from(format!("保存失败: {error}")),
        });
        cx.notify();
    }

    /// 修改首选播放音质并持久化。
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

    /// 保存自定义缓存存储路径。
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

    /// 设置最大本地缓存上限并保存。
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

    /// 触发领取每日免费畅听 VIP。
    fn claim_vip(&mut self, cx: &mut Context<Self>) {
        self.last_action = Some(SharedString::new_static("正在领取…"));
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

    /// 执行退出登录并清理凭据。
    fn logout(&mut self) {
        let app = self.backend.app();
        self.backend.spawn(async move {
            app.logout()
                .map_or_else(|error| Msg::Toast(format!("登出失败: {error}")), |()| Msg::LoggedOut)
        });
    }

    /// 清理内存中的接口请求缓存。
    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        self.backend.app().clear_cache();
        self.last_action = Some(SharedString::new_static("已清理接口缓存"));
        cx.notify();
    }

    /// 生成会员状态展示文案。
    fn vip_label(&self) -> &'static str {
        match &self.vip {
            Some(vip) if vip.is_concept_active() => "畅听 VIP 有效",
            Some(_) => "未开通畅听 VIP",
            None => "会员状态获取中…",
        }
    }
}

impl SettingsView {
    /// 渲染带有左侧标题的设置条目行。
    fn section(label: &str, body: AnyElement, cx: &App) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .w_full()
            .min_h(px(ROW_HEIGHT))
            .px_4()
            .py_2()
            .gap(px(16.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .w(px(140.0))
                    .flex_shrink_0()
                    .text_sm()
                    .child(SharedString::from(label.to_owned())),
            )
            .child(div().flex_1().min_w(px(0.0)).child(body))
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let mut quality_row = h_flex().gap(px(6.0)).flex_wrap();
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
        quality_row = quality_row.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!(
                    "当前: {}",
                    quality_label(self.quality.as_str())
                ))),
        );

        let mut preset_row = h_flex().gap(px(6.0)).flex_wrap();
        preset_row = preset_row.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!(
                    "当前上限: {}",
                    format_bytes(self.max_cache_bytes)
                ))),
        );
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

        let cache_row = h_flex()
            .gap(px(8.0))
            .child(div().flex_1().child(Input::new(&self.cache_dir)))
            .child(
                Button::new("save_cache_dir")
                    .small()
                    .label("保存")
                    .on_click(cx.listener(|view, _event, _window, cx| view.save_cache_dir(cx))),
            );

        let vip_row = h_flex()
            .gap(px(12.0))
            .child(div().text_sm().child(SharedString::from(self.vip_label())))
            .child(
                Button::new("claim_vip")
                    .small()
                    .label("领取每日畅听 VIP")
                    .on_click(cx.listener(|view, _event, _window, cx| view.claim_vip(cx))),
            );

        let account_row = h_flex()
            .gap(px(8.0))
            .child(
                Button::new("logout")
                    .small()
                    .danger()
                    .label("登出")
                    .on_click(cx.listener(|view, _event, _window, _cx| view.logout())),
            )
            .child(
                Button::new("clear_cache")
                    .small()
                    .label("清理接口缓存")
                    .on_click(cx.listener(|view, _event, _window, cx| view.clear_cache(cx))),
            );

        let status = self.last_action.clone().map(|text| {
            div()
                .px_4()
                .py_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element()
        });

        v_flex()
            .size_full()
            .child(
                div()
                    .w_full()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(title("设置", cx)),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(Self::section("音质", quality_row.into_any_element(), cx))
                    .child(Self::section(
                        "缓存目录（重启后生效）",
                        cache_row.into_any_element(),
                        cx,
                    ))
                    .child(Self::section("缓存上限", preset_row.into_any_element(), cx))
                    .child(Self::section("会员", vip_row.into_any_element(), cx))
                    .child(Self::section("账号与缓存", account_row.into_any_element(), cx)),
            )
            .children(status)
    }
}
