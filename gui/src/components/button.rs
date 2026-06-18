use gpui::{div, prelude::*, rgb, App, ClickEvent, SharedString, Window};

use crate::theme;

pub(crate) fn button(
    label: &str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(label.to_string()))
        .px_3()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .bg(rgb(theme::ACCENT))
        .child(label.to_string())
        .on_click(on_click)
}
