use gpui::{prelude::*, px, rgb, svg};

/// A monochrome SVG icon from the embedded asset source (`gui/assets/icons/`),
/// tinted `color` and sized `sz`×`sz` px. gpui rasterizes the SVG to an alpha mask
/// and fills it with the element's `text_color`.
pub(crate) fn icon(name: &str, sz: f32, color: u32) -> impl IntoElement {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(sz))
        .flex_none()
        .text_color(rgb(color))
}
