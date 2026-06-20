//! Progress bar (design/CAPTURE-HANDOFF.md §4). Used by App-updates / import in
//! #71–#76; unused until then.
#![allow(dead_code)]

use gpui::{div, prelude::*, px, relative, rgb};

use crate::theme;

/// A progress track (height 6, radius 3, ELEVATED) with an inner fill whose
/// width is `fraction` of the track. Fill is ACCENT, or SUCCESS when `complete`.
pub(crate) fn progress_bar(fraction: f32, complete: bool) -> impl IntoElement {
    div()
        .w_full()
        .h(px(6.0))
        .rounded(px(3.0))
        .bg(rgb(theme::ELEVATED))
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0.0, 1.0)))
                .rounded(px(3.0))
                .bg(if complete { rgb(theme::SUCCESS) } else { rgb(theme::ACCENT) }),
        )
}
