//! Lyra音乐品牌标识与图标组件

use gpui_kit::component::h_flex;
use gpui_kit::*;

use crate::theme::{ACCENT_GREEN, TEXT_PRIMARY};

/// Lyra 天琴座标志纯矢量 SVG 源码（支持 fill="currentColor" 自由着色）
pub(crate) const LYRA_MARK_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" fill="none">
  <path d="M 236 384 C 180 376 136 328 136 248 C 136 172 176 138 206 138 C 220 138 226 148 218 160 C 206 178 174 200 174 250 C 174 300 204 336 244 348 Z" fill="currentColor" />
  <path d="M 276 384 C 332 376 376 328 376 248 C 376 172 336 138 306 138 C 292 138 286 148 294 160 C 306 178 338 200 338 250 C 338 300 308 336 268 348 Z" fill="currentColor" />
  <path d="M 200 368 C 220 404 292 404 312 368 C 286 382 226 382 200 368 Z" fill="currentColor" />
  <rect x="210" y="220" width="14" height="136" rx="7" fill="currentColor" />
  <rect x="238" y="176" width="14" height="186" rx="7" fill="currentColor" />
  <rect x="266" y="176" width="14" height="186" rx="7" fill="currentColor" />
  <rect x="294" y="220" width="14" height="136" rx="7" fill="currentColor" />
  <circle cx="259" cy="270" r="10" fill="currentColor" />
  <path d="M 372 100 Q 372 128 396 128 Q 372 128 372 156 Q 372 128 348 128 Q 372 128 372 100 Z" fill="currentColor" />
  <circle cx="372" cy="128" r="4" fill="currentColor" />
</svg>"#;

/// 构造指定尺寸的 Lyra音乐 品牌 Logo 图标
pub(crate) fn lyra_logo_icon(edge: f32) -> Div {
    let radius = edge * 0.25;
    let mark_size = edge * 0.62;
    div()
        .w(px(edge))
        .h(px(edge))
        .flex_shrink_0()
        .rounded(px(radius))
        .bg(ACCENT_GREEN)
        .shadow_md()
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .data(LYRA_MARK_SVG.as_bytes())
                .size(px(mark_size))
                .text_color(rgb(0xffffff)),
        )
}

/// 侧边栏顶部品牌标识栏组件（精简现代风格，与左侧栏无缝贴合）
pub(crate) fn lyra_brand_header() -> impl IntoElement {
    h_flex()
        .id("lyra_brand_header")
        .w_full()
        .px_2()
        .py(px(6.0))
        .items_center()
        .gap(px(8.0))
        .child(lyra_logo_icon(26.0))
        .child(
            h_flex()
                .gap(px(6.0))
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(TEXT_PRIMARY)
                        .child("Lyra音乐"),
                )
                .child(
                    div()
                        .px(px(4.0))
                        .py(px(0.5))
                        .rounded(px(3.0))
                        .bg(crate::theme::hex_rgb(0xe5e7eb))
                        .text_color(crate::theme::hex_rgb(0x6b7280))
                        .text_size(px(9.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Lite"),
                ),
        )
}
