//! Selectable list row + checkbox (design/CAPTURE-HANDOFF.md §4). Used by the
//! Dashboard sessions/windows lists in #71–#76; unused until then.
#![allow(dead_code)]

use gpui::{div, prelude::*, px, rgb, App, ClickEvent, SharedString, Window};

use crate::components::icon::icon;
use crate::theme;

/// A selectable list row. Padding 10×12, radius 6, gap 12. The row's content
/// (id label, meta, action icons, checkbox) is composed by the caller and
/// passed in. Selected rows gain ACCENT_SUBTLE fill + 1px ACCENT_BORDER + a 2px
/// ACCENT left bar.
///
/// GPUI exposes only a single `border_color` for all sides, so the distinctly
/// coloured 2px left bar from §4 can't be a real per-side border. We render it
/// as an absolutely-positioned overlay on the left edge instead. The 2px bar is
/// always present (transparent when unselected) so selection doesn't shift the
/// content.
pub(crate) fn list_row(
    id: &str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    content: impl IntoElement,
) -> impl IntoElement {
    let left_bar = div()
        .absolute()
        .top_0()
        .left_0()
        .h_full()
        .w(px(2.0))
        .bg(if selected { rgb(theme::ACCENT) } else { rgb(theme::TRANSPARENT) });

    div()
        .id(SharedString::from(id.to_string()))
        .relative()
        .flex()
        .items_center()
        .gap(px(theme::SP_3))
        .py(px(10.0))
        .px(px(theme::SP_3))
        .rounded(px(theme::RADIUS_MD))
        .cursor_pointer()
        .border_1()
        .border_color(if selected { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
        .bg(if selected { rgb(theme::ACCENT_SUBTLE) } else { rgb(theme::PANEL) })
        .when(!selected, |d| d.hover(|s| s.bg(rgb(theme::ELEVATED))))
        .child(left_bar)
        .child(content)
        .on_click(on_click)
}

/// A 14px checkbox. Checked = ACCENT fill + a white `check` icon; unchecked =
/// CHIP_IDLE fill + 1px BORDER.
pub(crate) fn checkbox(checked: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .size(px(14.0))
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(if checked { rgb(theme::ACCENT) } else { rgb(theme::BORDER) })
        .bg(if checked { rgb(theme::ACCENT) } else { rgb(theme::CHIP_IDLE) })
        .when(checked, |d| d.child(icon("check", 10.0, theme::ON_ACCENT)))
}
