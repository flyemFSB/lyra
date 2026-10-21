//! 动效与过渡系统：基于 GPUI 原生动画引擎与 gpui-base motion 原语
use std::time::Duration;

use gpui_kit::base::animation::{cubic_bezier, ease_out_cubic};
use gpui_kit::component::Icon;
use gpui_kit::{
    Animation, AnimationElement, AnimationExt as _, AnyElement, ElementId, IntoElement, Styled, Transformation, linear,
    percentage, pulsating_between, px,
};

// -----------------------------------------------------------------------------
// 一、全局缓动参数规范（Motion Tokens）
// -----------------------------------------------------------------------------

/// 微交互（Hover、点击反馈，150ms）
#[allow(dead_code)]
pub(crate) const DURATION_MICRO: Duration = Duration::from_millis(150);

/// 状态展开/折叠（抽屉、卡片尺寸变动、Accordion 折叠面板，300ms 柔和阻尼）
#[allow(dead_code)]
pub(crate) const DURATION_EXPAND: Duration = Duration::from_millis(300);

/// 封面平滑交叉淡化（Crossfade，350ms）
#[allow(dead_code)]
pub(crate) const DURATION_CROSSFADE: Duration = Duration::from_millis(350);

/// 歌词流体滚动与切歌过渡（500ms）
#[allow(dead_code)]
pub(crate) const DURATION_LYRICS_FLUID: Duration = Duration::from_millis(500);

/// 苹果设计语言柔和阻尼物理弹簧曲线：cubic-bezier(0.16, 1.0, 0.3, 1.0)
pub(crate) fn apple_damping_spring() -> impl Fn(f32) -> f32 {
    cubic_bezier(0.16, 1.0, 0.3, 1.0)
}

/// 歌词流体自然滚动阻尼曲线：cubic-bezier(0.25, 1.0, 0.5, 1.0)
#[allow(dead_code)]
pub(crate) fn fluid_scroll_spring() -> impl Fn(f32) -> f32 {
    cubic_bezier(0.25, 1.0, 0.5, 1.0)
}

/// 歌单胶囊指示器滑动切换动画时长（200ms）
#[allow(dead_code)]
pub(crate) const CAPSULE_TRANSITION_DURATION: Duration = Duration::from_millis(200);

/// 页面视图切换平滑过渡动画时长（180ms，轻盈丝滑）
pub(crate) const PAGE_TRANSITION_DURATION: Duration = Duration::from_millis(180);

/// 浮动轻提示入场动画时长（240ms，优雅下落）
pub(crate) const TOAST_TRANSITION_DURATION: Duration = Duration::from_millis(240);

/// 抽屉滑入动画时长（280ms，自右边缘平滑切入）
pub(crate) const DRAWER_TRANSITION_DURATION: Duration = Duration::from_millis(280);

/// 歌词大屏展开/收起动画时长（300ms）
pub(crate) const LYRICS_TRANSITION_DURATION: Duration = Duration::from_millis(300);

/// 图标旋转动画时长（1000ms 每圈）
pub(crate) const SPINNER_ROTATION_DURATION: Duration = Duration::from_millis(1000);

/// 播放脉冲动画时长（1600ms 呼吸周期）
pub(crate) const PULSE_DURATION: Duration = Duration::from_millis(1600);

/// 为主内容区各视图提供轻缓上浮淡入过渡动效（微位移 8px + 透明度渐入）
pub(crate) fn page_entrance<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(PAGE_TRANSITION_DURATION).with_easing(ease_out_cubic),
        |this, delta| {
            let offset = px(8.0 * (1.0 - delta));
            this.relative().top(offset).opacity(0.40 + 0.60 * delta)
        },
    )
}

/// 为侧边栏歌单胶囊切换器（自建歌单 / 收藏歌单）提供平滑滑动的背景指示块动效
#[allow(dead_code)]
pub(crate) fn capsule_pill_sliding<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    from_left: f32,
    to_left: f32,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(CAPSULE_TRANSITION_DURATION).with_easing(ease_out_cubic),
        move |this, delta| {
            let left = px(from_left + (to_left - from_left) * delta);
            this.left(left)
        },
    )
}

/// 为歌词全屏视图提供沉浸式向上滑入展开动效（苹果柔和阻尼曲线，位移与透明度渐入）
pub(crate) fn lyrics_open_entrance<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(LYRICS_TRANSITION_DURATION).with_easing(apple_damping_spring()),
        |this, delta| {
            let offset = px(24.0 * (1.0 - delta));
            this.relative().top(offset).opacity(delta.clamp(0.0, 1.0))
        },
    )
}

/// 为歌词全屏视图提供平滑向下滑出退出动效（苹果柔和阻尼曲线，位移与透明度渐出）
pub(crate) fn lyrics_close_exit<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(LYRICS_TRANSITION_DURATION).with_easing(apple_damping_spring()),
        |this, delta| {
            let offset = px(24.0 * delta);
            this.relative().top(offset).opacity((1.0 - delta).clamp(0.0, 1.0))
        },
    )
}

/// 为顶部浮动 Toast 弹窗提供轻缓自顶部滑落与透明度渐入动效（从 top: 2px 下移至 14px）
pub(crate) fn toast_entrance<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(TOAST_TRANSITION_DURATION).with_easing(ease_out_cubic),
        |this, delta| {
            let offset = px(12.0 * (1.0 - delta));
            this.top(px(14.0) - offset).opacity(delta)
        },
    )
}

/// 为右侧抽屉面板提供自右侧平滑滑入动效（位移 24px + 透明度渐入）
pub(crate) fn drawer_entrance<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(DRAWER_TRANSITION_DURATION).with_easing(ease_out_cubic),
        |this, delta| {
            let offset = px(-24.0 * (1.0 - delta));
            this.relative().right(offset).opacity(delta)
        },
    )
}

/// 为图标附加平滑循环 360 度旋转动画
pub(crate) fn rotating_icon(icon: Icon, id: impl Into<ElementId>) -> AnyElement {
    icon.with_animation(
        id,
        Animation::new(SPINNER_ROTATION_DURATION).repeat().with_easing(linear),
        |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
    )
    .into_any_element()
}

/// 为正在播放的声音指示图标附加自然的呼吸韵律动效
pub(crate) fn pulsating_icon(icon: Icon, id: impl Into<ElementId>) -> AnyElement {
    icon.with_animation(
        id,
        Animation::new(PULSE_DURATION)
            .repeat()
            .with_easing(pulsating_between(0.50, 1.0)),
        gpui_kit::Styled::opacity,
    )
    .into_any_element()
}
