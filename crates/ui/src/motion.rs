//! 动效与过渡系统：基于 GPUI 原生动画引擎与 gpui-base motion 原语
use std::time::Duration;

use gpui_kit::base::animation::ease_out_cubic;
use gpui_kit::component::Icon;
use gpui_kit::{
    Animation, AnimationElement, AnimationExt as _, AnyElement, ElementId, IntoElement, Styled,
    Transformation, linear, percentage, pulsating_between, px,
};

/// 歌单胶囊指示器滑动切换动画时长（200ms）
pub(crate) const CAPSULE_TRANSITION_DURATION: Duration = Duration::from_millis(200);

/// 浮动轻提示入场动画时长（240ms，优雅下落）
pub(crate) const TOAST_TRANSITION_DURATION: Duration = Duration::from_millis(240);

/// 抽屉滑入动画时长（220ms，自右边缘平滑切入）
pub(crate) const DRAWER_TRANSITION_DURATION: Duration = Duration::from_millis(220);

/// 歌词大屏展开/收起动画时长（240ms）
pub(crate) const LYRICS_TRANSITION_DURATION: Duration = Duration::from_millis(240);

/// 图标旋转动画时长（1000ms 每圈）
pub(crate) const SPINNER_ROTATION_DURATION: Duration = Duration::from_millis(1000);

/// 播放脉冲动画时长（1600ms 呼吸周期）
pub(crate) const PULSE_DURATION: Duration = Duration::from_millis(1600);

/// 为侧边栏歌单胶囊切换器（自建歌单 / 收藏歌单）提供平滑滑动的背景指示块动效
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

/// 为歌词全屏视图提供沉浸式向上滑入展开动效（下->上位移 30px + 透明度渐入）
pub(crate) fn lyrics_open_entrance<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(LYRICS_TRANSITION_DURATION).with_easing(ease_out_cubic),
        |this, delta| {
            let offset = px(30.0 * (1.0 - delta));
            this.top(offset).opacity(delta)
        },
    )
}

/// 为歌词全屏视图提供平滑向下滑出退出动效（上->下位移 30px + 透明度渐出）
pub(crate) fn lyrics_close_exit<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(LYRICS_TRANSITION_DURATION).with_easing(ease_out_cubic),
        |this, delta| {
            let offset = px(30.0 * delta);
            this.top(offset).opacity(1.0 - delta)
        },
    )
}

/// 为浮动 Toast 提示提供轻缓下落与透明度渐入动效（从 top: 4px 下移至 16px）
pub(crate) fn toast_entrance<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    element.with_animation(
        id,
        Animation::new(TOAST_TRANSITION_DURATION).with_easing(ease_out_cubic),
        |this, delta| {
            let offset = px(12.0 * (1.0 - delta));
            this.top(px(16.0) - offset).opacity(delta)
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
            let offset = px(24.0 * (1.0 - delta));
            this.right(offset).opacity(delta)
        },
    )
}

/// 为图标附加平滑循环 360 度旋转动画
pub(crate) fn rotating_icon(icon: Icon, id: impl Into<ElementId>) -> AnyElement {
    icon.with_animation(
        id,
        Animation::new(SPINNER_ROTATION_DURATION)
            .repeat()
            .with_easing(linear),
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
        |icon, alpha| icon.opacity(alpha),
    )
    .into_any_element()
}
