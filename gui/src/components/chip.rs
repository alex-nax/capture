use gpui::{div, prelude::*, rgb, App, ClickEvent, SharedString, Window};

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
        .bg(if selected { rgb(0x2d4f67) } else { rgb(0x2a2a2a) })
        .text_color(if selected { rgb(0xe0e0e0) } else { rgb(0x9aa0a6) })
        .child(label.to_string())
        .on_click(on_click)
}
