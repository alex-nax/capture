use gpui::{div, prelude::*, px, rgb, App, ClickEvent, SharedString, Window};

use crate::theme;

/// A selectable "chip" / toggle (design/CAPTURE-HANDOFF.md §4). Height 29,
/// padding 6×12, radius 5, weight 500. Selected = ACCENT_SUBTLE fill +
/// ACCENT_TEXT_STRONG text + 1px ACCENT_BORDER; idle = CHIP_IDLE/TEXT_MUTED
/// that hovers to ELEVATED/TEXT_SECONDARY.
pub(crate) fn chip(
    id: &str,
    label: &str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .h(px(29.0))
        .py(px(6.0))
        .px(px(12.0))
        .rounded(px(theme::RADIUS_SM))
        .text_size(px(theme::TS_BODY))
        .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
        .cursor_pointer()
        .border_1()
        .border_color(if selected { rgb(theme::ACCENT_BORDER) } else { rgb(theme::CHIP_IDLE) })
        .bg(if selected { rgb(theme::ACCENT_SUBTLE) } else { rgb(theme::CHIP_IDLE) })
        .text_color(if selected { rgb(theme::ACCENT_TEXT_STRONG) } else { rgb(theme::TEXT_MUTED) })
        .when(!selected, |d| {
            d.hover(|s| s.bg(rgb(theme::ELEVATED)).text_color(rgb(theme::TEXT_SECONDARY)))
        })
        .child(label.to_string())
        .on_click(on_click)
}

/// A non-interactive disabled chip (CHIP_DISABLED fill, TEXT_DISABLED, no hover/click).
#[allow(dead_code)] // consumed by a disabled-chip call site in #71–#76
pub(crate) fn chip_disabled(id: &str, label: &str) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .h(px(29.0))
        .py(px(6.0))
        .px(px(12.0))
        .rounded(px(theme::RADIUS_SM))
        .text_size(px(theme::TS_BODY))
        .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
        .border_1()
        .border_color(rgb(theme::CHIP_DISABLED))
        .bg(rgb(theme::CHIP_DISABLED))
        .text_color(rgb(theme::TEXT_DISABLED))
        .child(label.to_string())
}
