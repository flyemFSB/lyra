//! 短信验证码登录视图组件。

use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use crate::bridge::{Backend, Msg};
use crate::theme::title;

/// 发送验证码倒计时时长（秒）。
const RESEND_SECONDS: u32 = 60;

/// 登录表单视图结构。
pub(crate) struct LoginView {
    backend: Backend,
    mobile: Entity<InputState>,
    code: Entity<InputState>,
    remaining: u32,
    sending: bool,
    status: Option<SharedString>,
    /// 倒计时异步任务句柄。
    #[allow(dead_code)]
    countdown: Task<()>,
    _subs: Vec<Subscription>,
}

impl LoginView {
    /// 创建登录视图。
    pub(crate) fn new(backend: Backend, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mobile = cx.new(|cx| InputState::new(window, cx).placeholder("手机号"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("验证码"));
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

    /// 发送短信验证码请求。
    fn send_code(&mut self, cx: &mut Context<Self>) {
        if self.sending || self.remaining > 0 {
            return;
        }
        let mobile = self.mobile.read(cx).value().to_string();
        if !lyra_app::is_mobile(&mobile) {
            self.status = Some(SharedString::new_static("请输入 11 位手机号"));
            cx.notify();
            return;
        }
        self.sending = true;
        self.status = Some(SharedString::new_static("正在下发验证码…"));
        cx.notify();
        let app = self.backend.app();
        self.backend
            .spawn(async move { Msg::AuthCode(app.send_code(&mobile).await.map_err(|error| error.to_string())) });
    }

    /// 提交手机号与验证码执行登录。
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let mobile = self.mobile.read(cx).value().to_string();
        let code = self.code.read(cx).value().to_string();
        if !lyra_app::is_mobile(&mobile) {
            self.status = Some(SharedString::new_static("请输入 11 位手机号"));
            cx.notify();
            return;
        }
        if code.trim().is_empty() {
            self.status = Some(SharedString::new_static("请输入验证码"));
            cx.notify();
            return;
        }
        self.sending = true;
        self.status = Some(SharedString::new_static("登录中…"));
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

    /// 处理验证码下发结果响应。
    pub(crate) fn on_auth_code(&mut self, result: Result<u64, String>, cx: &mut Context<Self>) {
        self.sending = false;
        match result {
            Ok(count) => {
                self.status = Some(SharedString::from(format!("验证码已下发（剩余 {count} 次）")));
                self.begin_countdown(cx);
            }
            Err(error) => self.status = Some(SharedString::from(format!("下发失败: {error}"))),
        }
        cx.notify();
    }

    /// 处理登录接口响应结果。
    pub(crate) fn on_login(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.sending = false;
        self.status = result
            .err()
            .map(|error| SharedString::from(format!("登录失败: {error}")));
        cx.notify();
    }

    /// 重置登录表单输入状态。
    pub(crate) fn reset(&mut self, cx: &mut Context<Self>) {
        self.remaining = 0;
        self.sending = false;
        self.status = None;
        self.countdown = Task::ready(());
        cx.notify();
    }

    /// 启动验证码重发每秒递减倒计时任务。
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
        let theme = cx.theme();
        let can_send = !self.sending && self.remaining == 0;
        let send_label = if self.remaining > 0 {
            SharedString::from(format!("{}s 后重发", self.remaining))
        } else {
            SharedString::new_static("发送验证码")
        };

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .child(
                v_flex()
                    .w(px(360.0))
                    .gap(px(12.0))
                    .p_6()
                    .rounded(px(12.0))
                    .bg(theme.popover)
                    .border_1()
                    .border_color(theme.border)
                    .child(title("登录", cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("概念版短信登录；验证码延迟时请稍候再试"),
                    )
                    .child(Input::new(&self.mobile).w_full())
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .child(div().flex_1().child(Input::new(&self.code)))
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
                            .label("登录")
                            .on_click(cx.listener(|this, _event, _window, cx| this.submit(cx))),
                    )
                    .children(
                        self.status
                            .clone()
                            .map(|text| div().text_xs().text_color(theme.muted_foreground).child(text)),
                    ),
            )
    }
}
