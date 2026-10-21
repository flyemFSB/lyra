//! 界面主题系统与常用视觉组件

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, h_flex};
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

/// 窗口画布背景色（Level 0 基准冷浅灰）
pub(crate) const CANVAS_BG: Rgba = hex_rgb(0xf4f5f7);

/// 工作区主卡片背景色（Level 1 纯白）
pub(crate) const CARD_BG: Rgba = hex_rgb(0xffffff);

/// 卡片边框颜色（微弱分隔）
pub(crate) const CARD_BORDER: Rgba = hex_rgb(0xe5e7eb);

/// 悬停高亮背景色（Level 2 填充色微变）
pub(crate) const SURFACE_HOVER: Rgba = hex_rgb(0xf0f2f5);

/// 翡翠绿品牌主色（仅用于核心交互点）
pub(crate) const ACCENT_GREEN: Rgba = hex_rgb(0x10b981);

/// 翡翠绿悬停加深色
pub(crate) const ACCENT_GREEN_HOVER: Rgba = hex_rgb(0x059669);

/// 翡翠绿极浅背景（用于声波指示或激活点缀）
pub(crate) const ACCENT_GREEN_LIGHT: Rgba = hex_rgb(0xecfdf5);

/// 翡翠绿轻描边色
pub(crate) const ACCENT_GREEN_BORDER: Rgba = hex_rgb(0xa7f3d0);

// 基础文字颜色层级
pub(crate) const TEXT_PRIMARY: Rgba = hex_rgb(0x111827);
pub(crate) const TEXT_MUTED: Rgba = hex_rgb(0x6b7280);
pub(crate) const TEXT_FAINT: Rgba = hex_rgb(0x9ca3af);

/// VIP 会员卡片温润浅金微渐变与品质调和色（降低对比度，消除黑块割裂感）
pub(crate) const VIP_GOLD_BG: Rgba = hex_rgb(0xfdfbf7);
#[allow(dead_code)]
pub(crate) const VIP_GOLD_SURFACE: Rgba = hex_rgb(0xf8f4ec);
pub(crate) const VIP_GOLD_BORDER: Rgba = hex_rgb(0xf0e6d2);
pub(crate) const VIP_GOLD_TEXT: Rgba = hex_rgb(0x785924);
pub(crate) const VIP_GOLD_MUTED: Rgba = hex_rgb(0x9c7842);
pub(crate) const VIP_GOLD_ICON_BG: Rgba = hex_rgb(0xfef3c7);
pub(crate) const VIP_GOLD_ICON_FG: Rgba = hex_rgb(0xd97706);

/// 曲目列表单行高度（桌面端单行表格规范）
pub(crate) const ROW_HEIGHT: f32 = 48.0;

/// 生成等高虚拟列表行高集合
pub(crate) fn row_sizes(n: usize) -> std::rc::Rc<Vec<Size<Pixels>>> {
    std::rc::Rc::new((0..n).map(|_| size(px(0.0), px(ROW_HEIGHT))).collect())
}

/// 构造主内容圆角卡片容器（Level 1：无生硬外边框，轻微漫反射投影）
pub(crate) fn main_card() -> Div {
    div()
        .bg(CARD_BG)
        .rounded(px(12.0))
        .border_1()
        .border_color(hex_rgb(0xebedf1))
        .shadow_sm()
}

/// 构造通用胶囊徽标组件
pub(crate) fn badge(text: impl Into<SharedString>, bg: impl Into<Hsla>, fg: impl Into<Hsla>) -> impl IntoElement {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(999.0))
        .bg(bg.into())
        .text_color(fg.into())
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

/// 构造 SQ 无损音质微标（极简低对比度中性灰底微标，参考 Apple Music）
pub(crate) fn sq_badge() -> impl IntoElement {
    div()
        .px(px(4.0))
        .py(px(0.5))
        .rounded(px(3.0))
        .bg(hex_rgb(0xf3f4f6))
        .border_1()
        .border_color(hex_rgb(0xe5e7eb))
        .text_color(hex_rgb(0x6b7280))
        .text_size(px(9.0))
        .font_weight(FontWeight::SEMIBOLD)
        .child("SQ")
}

/// 构造骨架屏微光条纹占位组件（用于时长等未就绪数据，消除 0:00 悬空感）
pub(crate) fn skeleton_shimmer(width: f32, height: f32, id: impl Into<ElementId>) -> AnyElement {
    div()
        .w(px(width))
        .h(px(height))
        .rounded(px(height / 2.0))
        .bg(hex_rgb(0xe5e7eb))
        .with_animation(
            id,
            Animation::new(Duration::from_millis(1400))
                .repeat()
                .with_easing(pulsating_between(0.35, 0.85)),
            gpui_kit::Styled::opacity,
        )
        .into_any_element()
}

/// 构造 3 柱实时动态声波均衡器图标组件（跳动的柱形频谱，暂停时平滑回落平齐）
pub(crate) fn audio_equalizer(is_playing: bool, id: impl Into<ElementId>) -> AnyElement {
    let id_base: ElementId = id.into();
    if is_playing {
        h_flex()
            .gap(px(2.0))
            .items_end()
            .h(px(14.0))
            .child(
                div().w(px(2.5)).rounded(px(1.0)).bg(ACCENT_GREEN).with_animation(
                    (id_base.clone(), "eq_bar_1"),
                    Animation::new(Duration::from_millis(650))
                        .repeat()
                        .with_easing(pulsating_between(0.35, 1.0)),
                    |bar, delta| bar.h(px(4.0 + 10.0 * delta)),
                ),
            )
            .child(
                div().w(px(2.5)).rounded(px(1.0)).bg(ACCENT_GREEN).with_animation(
                    (id_base.clone(), "eq_bar_2"),
                    Animation::new(Duration::from_millis(500))
                        .repeat()
                        .with_easing(pulsating_between(0.20, 0.90)),
                    |bar, delta| bar.h(px(3.0 + 11.0 * delta)),
                ),
            )
            .child(
                div().w(px(2.5)).rounded(px(1.0)).bg(ACCENT_GREEN).with_animation(
                    (id_base, "eq_bar_3"),
                    Animation::new(Duration::from_millis(800))
                        .repeat()
                        .with_easing(pulsating_between(0.40, 1.0)),
                    |bar, delta| bar.h(px(5.0 + 9.0 * delta)),
                ),
            )
            .into_any_element()
    } else {
        h_flex()
            .gap(px(2.0))
            .items_end()
            .h(px(14.0))
            .child(div().w(px(2.5)).h(px(4.0)).rounded(px(1.0)).bg(ACCENT_GREEN))
            .child(div().w(px(2.5)).h(px(4.0)).rounded(px(1.0)).bg(ACCENT_GREEN))
            .child(div().w(px(2.5)).h(px(4.0)).rounded(px(1.0)).bg(ACCENT_GREEN))
            .into_any_element()
    }
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
        .size(px(24.0))
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
        .gap(px(10.0))
        .text_color(TEXT_MUTED)
        .child(div().p_3().rounded_full().bg(SURFACE_HOVER).child(icon_element))
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(text.into()))
        .into_any_element()
}

/// 构造居中状态提示组件
pub(crate) fn notice(text: impl Into<SharedString>) -> AnyElement {
    notice_with_icon(IconName::Music, text)
}

/// 黑胶与抽象音符矢量 SVG 源码（高品质专业音乐占位图，零内存分配）
const VINYL_ART_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" fill="none">
  <circle cx="50" cy="50" r="42" stroke="currentColor" stroke-width="1.8" opacity="0.16" />
  <circle cx="50" cy="50" r="33" stroke="currentColor" stroke-width="1.2" opacity="0.22" />
  <circle cx="50" cy="50" r="24" stroke="currentColor" stroke-width="1.2" opacity="0.16" />
  <circle cx="50" cy="50" r="16" fill="currentColor" opacity="0.26" />
  <path d="M 48 37 L 48 55 C 46.5 54 44.5 53.2 42 53.2 C 38 53.2 35 55.6 35 59 C 35 62.4 38 64.8 42 64.8 C 46 64.8 49 62.4 49 59 L 49 42 L 61 38.5 L 61 51 C 59.5 50 57.5 49.2 55 49.2 C 51 49.2 48 51.6 48 55 C 48 58.4 51 60.8 55 60.8 C 59 60.8 62 58.4 62 55 L 62 33 Z" fill="currentColor" opacity="0.88" />
</svg>"#;

/// 构造专辑封面占位组件（无封面或离线时优雅降级：列表呈精致中性冷银卡片，大图呈唱片黑胶质感）
pub(crate) fn cover_box(_label: &str, edge: f32) -> AnyElement {
    let radius = (edge * 0.15).clamp(4.0, 16.0);
    let icon_edge = edge * 0.72;

    if edge >= 100.0 {
        // 大图场景（全屏歌词 320px 等）：黑胶唱片质感
        div()
            .w(px(edge))
            .h(px(edge))
            .flex_shrink_0()
            .rounded(px(radius))
            .bg(hex_rgb(0x181a20))
            .border_1()
            .border_color(hex_rgb(0x2d323b))
            .text_color(hex_rgb(0x9ca3af))
            .shadow_md()
            .flex()
            .items_center()
            .justify_center()
            .child(svg().data(VINYL_ART_SVG.as_bytes()).size(px(icon_edge)))
            .into_any_element()
    } else {
        // 紧凑列表场景（40px / 44px / 28px）：与全局浅色层级一致的冷银音符卡片，绝非突兀黑块
        div()
            .w(px(edge))
            .h(px(edge))
            .flex_shrink_0()
            .rounded(px(radius))
            .bg(hex_rgb(0xf1f3f5))
            .border_1()
            .border_color(hex_rgb(0xe2e4e8))
            .text_color(hex_rgb(0x94a3b8))
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(IconName::Music).size(px((edge * 0.45).clamp(12.0, 24.0))))
            .into_any_element()
    }
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

/// 构造歌曲封面组件（有网络封面图时展示高清圆角封面，无封面或加载失败时优雅降级）
pub(crate) fn song_cover(cover_url: &str, title: &str, edge: f32) -> AnyElement {
    let radius = (edge * 0.15).clamp(4.0, 16.0);
    let title_owned = title.to_owned();
    let trimmed = cover_url.trim();
    if trimmed.is_empty() {
        return cover_box(title, edge);
    }

    img(trimmed.to_owned())
        .w(px(edge))
        .h(px(edge))
        .flex_shrink_0()
        .rounded(px(radius))
        .object_fit(ObjectFit::Cover)
        .with_fallback(move || cover_box(&title_owned, edge))
        .into_any_element()
}

/// 拦截底层 raw 报错，转换为对用户友好的提示文案
pub(crate) fn friendly_error_message(error: &str) -> String {
    let lower = error.to_lowercase();
    if error.contains("297002") {
        "今日畅听 VIP 已经领取过啦".to_owned()
    } else if error.contains("297000") {
        "今日领取条件未达成（概念版需先试听歌曲几秒后再试）".to_owned()
    } else if lower.contains("error_code") || lower.contains("upstream") {
        "服务响应异常，请稍后重试".to_owned()
    } else if lower.contains("network")
        || lower.contains("timeout")
        || lower.contains("connection")
        || lower.contains("dns")
    {
        "网络连接不稳定，请检查网络".to_owned()
    } else if lower.contains("login") || lower.contains("auth") || lower.contains("token") {
        "账号登录态已过期，请重新登录".to_owned()
    } else if lower.contains("decode") || lower.contains("format") || lower.contains("unsupported") {
        "音频解码暂不支持或资源受损".to_owned()
    } else if lower.contains("vip") || lower.contains("pay") || lower.contains("copyright") {
        "该曲目需要会员权益或暂无版权".to_owned()
    } else if lower.contains("device") {
        "未找到可用音频输出设备".to_owned()
    } else {
        "操作未成功，请稍后重试".to_owned()
    }
}
