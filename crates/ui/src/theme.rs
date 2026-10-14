//! 界面主题与共用基础视觉组件。

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

/// 曲目虚拟列表单行高度。
pub(crate) const ROW_HEIGHT: f32 = 54.0;

/// 生成指定行数的等高虚拟列表尺寸集合。
pub(crate) fn row_sizes(n: usize) -> std::rc::Rc<Vec<Size<Pixels>>> {
    std::rc::Rc::new((0..n).map(|_| size(px(0.0), px(ROW_HEIGHT))).collect())
}

/// 构造统一样式的区块标题组件。
pub(crate) fn title(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().foreground)
        .child(text.into())
}

/// 构造居中展示的状态提示或空态组件。
pub(crate) fn notice(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(cx.theme().muted_foreground)
        .child(div().text_sm().child(text.into()))
        .into_any_element()
}

/// 构造封面占位图像组件。
pub(crate) fn cover_box(label: &str, edge: f32, cx: &App) -> AnyElement {
    let initial = label.chars().next().map_or_else(|| "♪".to_owned(), |c| c.to_string());
    div()
        .w(px(edge))
        .h(px(edge))
        .flex_shrink_0()
        .rounded(px(6.0))
        .bg(cx.theme().muted)
        .flex()
        .items_center()
        .justify_center()
        .text_color(cx.theme().muted_foreground)
        .child(div().text_lg().child(initial))
        .into_any_element()
}
