//! 界面主题系统与常用视觉组件

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::*;

/// 十六进制 RGB 颜色构造
pub(crate) const fn hex_rgb(hex: u32) -> Rgba {
    let [_, r, g, b] = hex.to_be_bytes();
    Rgba {
        r: (r as f32) / 255.0,
        g: (g as f32) / 255.0,
        b: (b as f32) / 255.0,
        a: 1.0,
    }
}

/// 窗口画布背景色
pub(crate) const CANVAS_BG: Rgba = hex_rgb(0xf2f4f7);

/// 工作区主卡片背景色
pub(crate) const CARD_BG: Rgba = hex_rgb(0xffffff);

/// 卡片边框颜色
pub(crate) const CARD_BORDER: Rgba = hex_rgb(0xe5e7eb);

/// 悬停高亮背景色
pub(crate) const SURFACE_HOVER: Rgba = hex_rgb(0xe9edf2);

/// 翡翠绿品牌主色
pub(crate) const ACCENT_GREEN: Rgba = hex_rgb(0x10b981);

/// 翡翠绿悬停加深色
pub(crate) const ACCENT_GREEN_HOVER: Rgba = hex_rgb(0x059669);

/// 翡翠绿浅色背景
pub(crate) const ACCENT_GREEN_LIGHT: Rgba = hex_rgb(0xecfdf5);

/// 翡翠绿边框色
pub(crate) const ACCENT_GREEN_BORDER: Rgba = hex_rgb(0xa7f3d0);

/// VIP 标识金色
pub(crate) const VIP_GOLD: Rgba = hex_rgb(0xf59e0b);

/// VIP 浅金背景色
pub(crate) const VIP_GOLD_LIGHT: Rgba = hex_rgb(0xfef3c7);

// 基础文字颜色层级
pub(crate) const TEXT_PRIMARY: Rgba = hex_rgb(0x111827);
pub(crate) const TEXT_MUTED: Rgba = hex_rgb(0x4b5563);
pub(crate) const TEXT_FAINT: Rgba = hex_rgb(0x9ca3af);

/// 曲目列表单行高度
pub(crate) const ROW_HEIGHT: f32 = 52.0;

/// 生成等高虚拟列表行高集合
pub(crate) fn row_sizes(n: usize) -> std::rc::Rc<Vec<Size<Pixels>>> {
    std::rc::Rc::new((0..n).map(|_| size(px(0.0), px(ROW_HEIGHT))).collect())
}

/// 构造主内容圆角卡片容器
pub(crate) fn main_card() -> Div {
    div()
        .bg(CARD_BG)
        .rounded(px(16.0))
        .border_1()
        .border_color(CARD_BORDER)
        .shadow_sm()
}

/// 构造胶囊徽标组件（如 SQ、VIP 等）
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

/// 构造区块标题组件
pub(crate) fn title(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_base()
        .font_weight(FontWeight::BOLD)
        .text_color(TEXT_PRIMARY)
        .child(text.into())
}

/// 构造居中状态提示组件（支持自定义图标与旋转动效）
pub(crate) fn notice_with_icon(icon: IconName, text: impl Into<SharedString>) -> AnyElement {
    let is_spinning = matches!(icon, IconName::RefreshCw | IconName::Loader);
    let icon_element = Icon::new(icon)
        .size(px(26.0))
        .text_color(if is_spinning { ACCENT_GREEN } else { TEXT_MUTED });
    let icon_element = if is_spinning {
        crate::motion::rotating_icon(icon_element, "notice_spinner")
    } else {
        icon_element.into_any_element()
    };

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
                .child(icon_element),
        )
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(text.into()))
        .into_any_element()
}

/// 构造居中状态提示组件
pub(crate) fn notice(text: impl Into<SharedString>) -> AnyElement {
    notice_with_icon(IconName::Music, text)
}

/// 构造专辑封面占位组件
pub(crate) fn cover_box(label: &str, edge: f32) -> AnyElement {
    // 封面占位会在每一行曲目里渲染，必须零分配：
    // 单字符走 SmolStr 的内联存储，不占堆。
    let initial = label
        .chars()
        .next()
        .map_or_else(|| SharedString::new_static("♪"), SharedString::from);

    // 根据歌名哈希计算稳定的柔和色相
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

/// 构造热门词胶囊组件
pub(crate) fn chip(label: impl Into<SharedString>, icon: IconName) -> Div {
    div()
        .cursor_pointer()
        .px_3()
        .py_1()
        .rounded(px(999.0))
        .text_xs()
        .font_weight(FontWeight::NORMAL)
        .flex()
        .items_center()
        .gap(px(5.0))
        .bg(CARD_BG)
        .border_1()
        .border_color(CARD_BORDER)
        .text_color(TEXT_MUTED)
        .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT_PRIMARY))
        .child(Icon::new(icon).size(px(13.0)))
        .child(label.into())
}
