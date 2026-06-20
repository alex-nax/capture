//! The grouped-content card (design `.card`): PANEL fill, 1px CARD_BORDER outline,
//! RADIUS_LG corners, 20px padding. Wraps each Settings section's controls.
#![allow(dead_code)]

use gpui::{div, prelude::*, px, rgb};

use crate::theme;

/// A content card: PANEL bg, 1px CARD_BORDER, radius 8 (RADIUS_LG), padding 20.
pub(crate) fn card(content: impl IntoElement) -> impl IntoElement {
    div()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(rgb(theme::CARD_BORDER))
        .rounded(px(theme::RADIUS_LG))
        .p(px(20.0))
        .child(content)
}
