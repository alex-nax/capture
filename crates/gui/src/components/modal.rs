//! Modal / card (design/CAPTURE-HANDOFF.md §4). Used by the preset picker +
//! confirm dialogs in #76; unused until then.
#![allow(dead_code)]

use gpui::{div, prelude::*, px, rgb, rgba};

use crate::theme;

/// A centered modal card on a full-screen dimmed backdrop. Card: width 380,
/// PANEL fill, 1px BORDER, radius 8, padding 20. Title 15/600; `body` and
/// `actions` (right-aligned, gap 10) are composed by the caller. The backdrop
/// `.occlude()`s clicks to the screen behind it.
pub(crate) fn modal(
    title: &str,
    body: impl IntoElement,
    actions: impl IntoElement,
) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(theme::BACKDROP))
        .occlude()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SP_4))
                .w(px(380.0))
                .p(px(theme::SP_5))
                .rounded(px(theme::RADIUS_LG))
                .bg(rgb(theme::PANEL))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .child(
                    div()
                        .text_size(px(theme::TS_HEADING))
                        .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(title.to_string()),
                )
                .child(div().text_size(px(theme::TS_BODY)).text_color(rgb(theme::TEXT_SECONDARY)).child(body))
                .child(div().flex().justify_end().gap(px(10.0)).child(actions)),
        )
}
