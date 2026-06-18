//! Section headers + status pill (design/CAPTURE-HANDOFF.md §4). Used by the
//! screen layouts in #71–#76; unused until then.
#![allow(dead_code)]

use gpui::{div, prelude::*, px, rgb};

use crate::theme;

/// A section eyebrow: 11/600 UPPERCASE TEXT_MUTED. (gpui has no letter-spacing
/// control, so the +0.06em tracking from §4 is dropped.)
pub(crate) fn eyebrow(text: &str) -> impl IntoElement {
    div()
        .text_size(px(theme::TS_EYEBROW))
        .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
        .text_color(rgb(theme::TEXT_MUTED))
        .child(text.to_uppercase())
}

/// A status pill: 12px text, padding 3×9, radius 5, with caller-supplied text
/// `color` on a `bg` fill (e.g. SUCCESS on SUCCESS_SUBTLE).
pub(crate) fn status_pill(text: &str, color: u32, bg: u32) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .py(px(3.0))
        .px(px(9.0))
        .rounded(px(theme::RADIUS_SM))
        .text_size(px(theme::TS_SMALL))
        .bg(rgb(bg))
        .text_color(rgb(color))
        .child(text.to_string())
}

/// A column header: 15/600 TEXT_PRIMARY title + an optional 12px TEXT_MUTED count.
pub(crate) fn column_header(title: &str, count: Option<usize>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(theme::SP_2))
        .child(
            div()
                .text_size(px(theme::TS_HEADING))
                .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                .text_color(rgb(theme::TEXT_PRIMARY))
                .child(title.to_string()),
        )
        .when_some(count, |d, n| {
            d.child(
                div()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(n.to_string()),
            )
        })
}
