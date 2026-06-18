use gpui::{div, prelude::*, rgb, App, ClickEvent, SharedString, Window};

use crate::theme;

/// A selectable "chip" for Settings toggles (highlighted when `selected`).
pub(crate) fn chip(
    id: &str,
    label: &str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .bg(if selected { rgb(theme::ACCENT_SUBTLE) } else { rgb(theme::CHIP_IDLE) })
        .text_color(if selected { rgb(theme::ACCENT_TEXT_STRONG) } else { rgb(theme::TEXT_MUTED) })
        .child(label.to_string())
        .on_click(on_click)
}
