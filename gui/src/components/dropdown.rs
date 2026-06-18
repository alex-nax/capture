//! Dropdown field + menu (design/CAPTURE-HANDOFF.md §4). Used by Settings/Playback
//! selectors in #71–#76; unused until then.
#![allow(dead_code)]

use gpui::{div, prelude::*, px, rgb, rgba, App, ClickEvent, SharedString, Window};

use crate::components::icon::icon;
use crate::theme;

/// A closed/open dropdown field: PANEL fill, 1px BORDER (hover BORDER_STRONG,
/// open ACCENT_BORDER), padding 8×12, radius 6, with the current `value` and a
/// trailing `chevron-down` caret (15px TEXT_MUTED).
pub(crate) fn dropdown_field(
    id: &str,
    value: &str,
    open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(theme::SP_2))
        .py(px(theme::SP_2))
        .px(px(theme::SP_3))
        .rounded(px(theme::RADIUS_MD))
        .text_size(px(theme::TS_BODY))
        .cursor_pointer()
        .bg(rgb(theme::PANEL))
        .border_1()
        .border_color(if open { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
        .text_color(rgb(theme::TEXT_PRIMARY))
        .when(!open, |d| d.hover(|s| s.border_color(rgb(theme::BORDER_STRONG))))
        .child(value.to_string())
        .child(icon("chevron-down", 15.0, theme::TEXT_MUTED))
        .on_click(on_click)
}

/// The open menu surface: ELEVATED fill, 1px BORDER, radius 6, pad 4. Caller
/// supplies the `dropdown_menu_item` children.
pub(crate) fn dropdown_menu(children: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .p(px(theme::SP_1))
        .rounded(px(theme::RADIUS_MD))
        .bg(rgb(theme::ELEVATED))
        .border_1()
        .border_color(rgb(theme::BORDER))
        .child(children)
}

/// A menu row: padding 7×10, radius 4, hover GHOST_HOVER, selected
/// ACCENT_SUBTLE fill + ACCENT_TEXT.
pub(crate) fn dropdown_menu_item(
    id: &str,
    label: &str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .py(px(7.0))
        .px(px(10.0))
        .rounded(px(4.0))
        .text_size(px(theme::TS_BODY))
        .cursor_pointer()
        .when(selected, |d| {
            d.bg(rgb(theme::ACCENT_SUBTLE)).text_color(rgb(theme::ACCENT_TEXT))
        })
        .when(!selected, |d| {
            d.text_color(rgb(theme::TEXT_SECONDARY))
                .hover(|s| s.bg(rgba(theme::GHOST_HOVER)))
        })
        .child(label.to_string())
        .on_click(on_click)
}
