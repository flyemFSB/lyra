//! 界面主题系统与视觉组件：仿照现代主流音乐桌面端（图 2）打造的简约淡雅设计系统。

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// 常量 RGB 构造辅助函数。
pub(crate) const fn hex_rgb(hex: u32) -> Rgba {
    let [_, r, g, b] = hex.to_be_bytes();
    Rgba {
        r: (r as f32) / 255.0,
        g: (g as f32) / 255.0,
        b: (b as f32) / 255.0,
        a: 1.0,
    }
}

/// 窗口底色画布：极其轻盈温和的纸白雾灰底色（仿图 2 简约淡雅）。
pub(crate) const CANVAS_BG: Rgba = hex_rgb(0xf2f4f7);

/// 主工作区卡片底色：纯白无暇的悬浮主岛屿。
pub(crate) const CARD_BG: Rgba = hex_rgb(0xffffff);

/// 极细微卡片边框：温润淡灰。
pub(crate) const CARD_BORDER: Rgba = hex_rgb(0xe5e7eb);

/// 悬浮交互浅色高亮。
pub(crate) const SURFACE_HOVER: Rgba = hex_rgb(0xe9edf2);

/// 品牌生机翡翠绿主色（仿图 2 主播放键与高光标识）。
pub(crate) const ACCENT_GREEN: Rgba = hex_rgb(0x10b981);

/// 翡翠绿悬浮加深色。
pub(crate) const ACCENT_GREEN_HOVER: Rgba = hex_rgb(0x059669);

/// 翡翠绿浅色气泡背景（高亮行与小徽标）。
pub(crate) const ACCENT_GREEN_LIGHT: Rgba = hex_rgb(0xecfdf5);

/// 翡翠绿边框。
pub(crate) const ACCENT_GREEN_BORDER: Rgba = hex_rgb(0xa7f3d0);

/// VIP 尊享鎏金色。
pub(crate) const VIP_GOLD: Rgba = hex_rgb(0xf59e0b);

/// VIP 浅金底色。
#[allow(dead_code)]
pub(crate) const VIP_GOLD_LIGHT: Rgba = hex_rgb(0xfef3c7);

/// 基础文字层级：高对比度、清晰舒适的排版规范。
pub(crate) const TEXT_PRIMARY: Rgba = hex_rgb(0x111827);
pub(crate) const TEXT_MUTED: Rgba = hex_rgb(0x4b5563);
pub(crate) const TEXT_FAINT: Rgba = hex_rgb(0x9ca3af);

/// 曲目列表单行高度。
pub(crate) const ROW_HEIGHT: f32 = 52.0;

/// 生成指定行数的等高虚拟列表尺寸集合。
pub(crate) fn row_sizes(n: usize) -> std::rc::Rc<Vec<Size<Pixels>>> {
    std::rc::Rc::new((0..n).map(|_| size(px(0.0), px(ROW_HEIGHT))).collect())
}

/// 构造仿图 2 的纯白大圆角主内容悬浮卡片。
pub(crate) fn main_card() -> Div {
    div()
        .bg(CARD_BG)
        .rounded(px(16.0))
        .border_1()
        .border_color(CARD_BORDER)
        .shadow_sm()
}

/// 构造精巧的高品质胶囊徽标（如 SQ、VIP、无损等）。
pub(crate) fn badge(text: impl Into<SharedString>, bg: impl Into<Hsla>, fg: impl Into<Hsla>) -> impl IntoElement {
    div()
        .px_2()
        .py(px(1.5))
        .rounded(px(999.0))
        .bg(bg.into())
        .text_color(fg.into())
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(text.into())
}

/// 构造统一样式的区块标题组件。
pub(crate) fn title(text: impl Into<SharedString>, _cx: &App) -> impl IntoElement {
    div()
        .text_base()
        .font_weight(FontWeight::BOLD)
        .text_color(TEXT_PRIMARY)
        .child(text.into())
}

/// 构造居中展示的状态提示或空态组件（带图标支持）。
pub(crate) fn notice_with_icon(icon: IconName, text: impl Into<SharedString>, _cx: &App) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .text_color(TEXT_MUTED)
        .child(
            div()
                .p_4()
                .rounded_full()
                .bg(SURFACE_HOVER)
                .border_1()
                .border_color(CARD_BORDER)
                .child(Icon::new(icon).size(px(26.0)).text_color(TEXT_MUTED)),
        )
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(text.into()))
        .into_any_element()
}

/// 构造居中展示的状态提示或空态组件。
pub(crate) fn notice(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    notice_with_icon(IconName::Music, text, cx)
}

/// 构造淡雅现代的专辑封面占位组件（具有柔和淡彩底色与精致排版）。
pub(crate) fn cover_box(label: &str, edge: f32, _cx: &App) -> AnyElement {
    let initial = label.chars().next().map_or_else(|| "♪".to_owned(), |c| c.to_string());

    // 根据歌名生成确定性的淡雅柔和渐变底色
    let hash = label.bytes().fold(0u32, |acc, b| acc.wrapping_add(b as u32));
    let hues = [158.0, 200.0, 240.0, 280.0, 330.0, 30.0];
    let hue = hues[(hash as usize) % hues.len()];

    let bg_color = hsla(hue / 360.0, 0.45, 0.92, 1.0);
    let accent_color = hsla(hue / 360.0, 0.65, 0.40, 1.0);

    div()
        .w(px(edge))
        .h(px(edge))
        .flex_shrink_0()
        .rounded(px(8.0))
        .bg(bg_color)
        .border_1()
        .border_color(CARD_BORDER)
        .shadow_sm()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_color(accent_color)
                .font_weight(FontWeight::BOLD)
                .text_xs()
                .child(initial),
        )
        .into_any_element()
}

/// 构造可点击的交互标签胶囊（用于搜索推荐词、分类等）。
pub(crate) fn chip(label: impl Into<SharedString>, active: bool, icon: Option<IconName>) -> Div {
    div()
        .cursor_pointer()
        .px_3()
        .py_1()
        .rounded(px(999.0))
        .text_xs()
        .font_weight(if active { FontWeight::BOLD } else { FontWeight::NORMAL })
        .flex()
        .items_center()
        .gap(px(5.0))
        .when(active, |this| {
            this.bg(ACCENT_GREEN_LIGHT)
                .border_1()
                .border_color(ACCENT_GREEN_BORDER)
                .text_color(ACCENT_GREEN_HOVER)
        })
        .when(!active, |this| {
            this.bg(CARD_BG)
                .border_1()
                .border_color(CARD_BORDER)
                .text_color(TEXT_MUTED)
                .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
        })
        .when_some(icon, |this, icon| this.child(Icon::new(icon).size(px(13.0))))
        .child(label.into())
}
