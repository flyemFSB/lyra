//! 手机验证码登录视图。

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use crate::bridge::{Backend, Msg};
use crate::theme::{ACCENT_GREEN, CARD_BORDER, TEXT_MUTED, hex_rgb, main_card, title};

/// 验证码重发倒计时时长（秒）
const RESEND_SECONDS: u32 = 60;

/// 登录视图
pub(crate) struct LoginView {
    backend: Backend,
    mobile: Entity<InputState>,
    code: Entity<InputState>,
    remaining: u32,
    sending: bool,
    status: Option<SharedString>,
    /// 倒计时异步任务
    #[allow(dead_code)]
    countdown: Task<()>,
    _subs: Vec<Subscription>,
}

impl LoginView {
    /// 创建登录视图
    pub(crate) fn new(backend: Backend, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mobile = cx.new(|cx| InputState::new(window, cx).placeholder("输入 11 位手机号码"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("短信验证码"));
        let mobile_sub = cx.subscribe(&mobile, |this, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.send_code(cx);
            }
        });
        let code_sub = cx.subscribe(&code, |this, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit(cx);
            }
        });
        Self {
            backend,
            mobile,
            code,
            remaining: 0,
            sending: false,
            status: None,
            countdown: Task::ready(()),
            _subs: vec![mobile_sub, code_sub],
        }
    }

    /// 发送短信验证码
    fn send_code(&mut self, cx: &mut Context<Self>) {
        if self.sending || self.remaining > 0 {
            return;
        }
        let mobile = self.mobile.read(cx).value().to_string();
        if !lyra_app::is_mobile(&mobile) {
            self.status = Some(SharedString::new_static("请输入有效 11 位手机号码"));
            cx.notify();
            return;
        }
        self.sending = true;
        self.status = Some(SharedString::new_static("正在发送短信验证码…"));
        cx.notify();
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::AuthCode(app.send_code(&mobile).await.map_err(|error| error.to_string())) });
    }

    /// 提交登录请求
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let mobile = self.mobile.read(cx).value().to_string();
        let code = self.code.read(cx).value().to_string();
        if !lyra_app::is_mobile(&mobile) {
            self.status = Some(SharedString::new_static("请输入有效 11 位手机号码"));
            cx.notify();
            return;
        }
        if code.trim().is_empty() {
            self.status = Some(SharedString::new_static("请输入短信验证码"));
            cx.notify();
            return;
        }
        self.sending = true;
        self.status = Some(SharedString::new_static("正在登录验证…"));
        cx.notify();
        let app = self.backend.app();
        self.backend.spawn(async move {
            Msg::Login(
                app.login(&mobile, &code)
                    .await
                    .map(|_account| ())
                    .map_err(|error| error.to_string()),
            )
        });
    }

    /// 处理验证码发送响应
    pub(crate) fn on_auth_code(&mut self, result: Result<u64, String>, cx: &mut Context<Self>) {
        self.sending = false;
        match result {
            Ok(count) => {
                self.status = Some(SharedString::from(format!("验证码已下发（今日剩余 {count} 次）")));
                self.begin_countdown(cx);
            }
            Err(error) => self.status = Some(SharedString::from(format!("下发失败: {error}"))),
        }
        cx.notify();
    }

    /// 处理登录响应
    pub(crate) fn on_login(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.sending = false;
        self.status = result
            .err()
            .map(|error| SharedString::from(format!("登录失败: {error}")));
        cx.notify();
    }

    /// 重置表单状态
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.remaining = 0;
        self.sending = false;
        self.status = None;
        self.countdown = Task::ready(());
        cx.notify();
    }

    /// 启动重发倒计时
    fn begin_countdown(&mut self, cx: &mut Context<Self>) {
        self.remaining = RESEND_SECONDS;
        self.countdown = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let done = this
                    .update(cx, |view, cx| {
                        view.remaining = view.remaining.saturating_sub(1);
                        cx.notify();
                        view.remaining == 0
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        });
    }
}

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_send = !self.sending && self.remaining == 0;
        let send_label = if self.remaining > 0 {
            SharedString::from(format!("{}s", self.remaining))
        } else {
            SharedString::new_static("获取验证码")
        };

        div().size_full().flex().items_center().justify_center().p_6().child(
            main_card()
                    .w(px(380.0))
                    .gap(px(16.0))
                    .p_7()
                    .shadow_xl()
                    // 品牌标识
                    .child(
                        v_flex()
                            .items_center()
                            .justify_center()
                            .gap(px(8.0))
                            .mb_2()
                            .child(crate::logo::lyra_logo_icon(56.0))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .child(title("登录到 Lyra音乐", cx)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(TEXT_MUTED)
                                    .child("输入手机号即可同步个人歌单与每日推荐"),
                            ),
                    )
                    .child(
                        h_flex()
                            .px_3()
                            .py(px(4.0))
                            .rounded(px(10.0))
                            .bg(hex_rgb(0xf2f4f7))
                            .border_1()
                            .border_color(CARD_BORDER)
                            .gap(px(8.0))
                            .items_center()
                            .child(Icon::new(IconName::Smartphone).size(px(16.0)).text_color(ACCENT_GREEN))
                            .child(div().flex_1().child(Input::new(&self.mobile).w_full())),
                    )
                    .child(
                        h_flex()
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
                                    .child(Icon::new(IconName::KeyRound).size(px(16.0)).text_color(ACCENT_GREEN))
                                    .child(div().flex_1().child(Input::new(&self.code).w_full())),
                            )
                            .child(
                                Button::new("send_code")
                                    .small()
                                    .label(send_label)
                                    .disabled(!can_send)
                                    .on_click(cx.listener(|this, _event, _window, cx| this.send_code(cx))),
                            ),
                    )
                    .child(
                        Button::new("login")
                            .primary()
                            .icon(IconName::LogIn)
                            .label("立即安全登录")
                            .on_click(cx.listener(|this, _event, _window, cx| this.submit(cx))),
                    )
                    .children(
                        self.status.clone().map(|text| {
                            h_flex()
                                .justify_center()
                                .gap(px(6.0))
                                .items_center()
                                .child(Icon::new(IconName::Sparkles).size(px(12.0)).text_color(ACCENT_GREEN))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(TEXT_MUTED)
                                        .child(text),
                                )
                        }),
                    ),
        )
    }
}
