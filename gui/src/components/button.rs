use gpui::{div, prelude::*, px, rgb, rgba, App, ClickEvent, SharedString, Window};

use crate::components::icon::icon;
use crate::theme;

/// Visual variants per design/CAPTURE-HANDOFF.md §4 (Button state matrix).
pub(crate) enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    // Wired up by a destructive call site in #71–#76.
    #[allow(dead_code)]
    Destructive,
}

/// Shared chrome for every button variant: height 32, radius 5 (RADIUS_SM),
/// horizontal padding ~14, body type, with per-variant fill/border/text/weight
/// + hover (and pressed for Primary). `id` keys the interactive element.
fn base(id: &str, variant: &ButtonVariant) -> gpui::Stateful<gpui::Div> {
    let b = div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .items_center()
        .justify_center()
        .h(px(32.0))
        .px(px(14.0))
        .rounded(px(theme::RADIUS_SM))
        .text_size(px(theme::TS_BODY))
        .cursor_pointer();
    match variant {
        ButtonVariant::Primary => b
            .bg(rgb(theme::ACCENT))
            .text_color(rgb(theme::ON_ACCENT))
            .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
            .hover(|s| s.bg(rgb(theme::ACCENT_HOVER)))
            .active(|s| s.bg(rgb(theme::ACCENT_ACTIVE))),
        ButtonVariant::Secondary => b
            .bg(rgb(theme::ELEVATED))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(theme::TEXT_PRIMARY))
            .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
            .hover(|s| s.border_color(rgb(theme::BORDER_STRONG))),
        ButtonVariant::Ghost => b
            .text_color(rgb(theme::TEXT_SECONDARY))
            .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
            .hover(|s| s.bg(rgba(theme::GHOST_HOVER))),
        ButtonVariant::Destructive => b
            .bg(rgb(theme::ERROR_SUBTLE))
            .border_1()
            .border_color(rgb(theme::ERROR_BORDER))
            .text_color(rgb(theme::ERROR))
            .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32)),
    }
}

/// A text button. `label` doubles as the element id.
pub(crate) fn button(
    label: &str,
    variant: ButtonVariant,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    base(label, &variant).child(label.to_string()).on_click(on_click)
}

/// A button with a leading icon (gap 7px) sized to the label. The icon is tinted
/// to the variant's text color via the element's inherited `text_color`.
#[allow(dead_code)] // consumed by an icon-button call site in #71–#76
pub(crate) fn icon_button(
    icon_name: &str,
    label: &str,
    variant: ButtonVariant,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let tint = match variant {
        ButtonVariant::Primary => theme::ON_ACCENT,
        ButtonVariant::Secondary => theme::TEXT_PRIMARY,
        ButtonVariant::Ghost => theme::TEXT_SECONDARY,
        ButtonVariant::Destructive => theme::ERROR,
    };
    base(label, &variant)
        .gap(px(7.0))
        .child(icon(icon_name, 15.0, tint))
        .child(label.to_string())
        .on_click(on_click)
}

/// A non-interactive disabled button (no hover, no pointer cursor).
#[allow(dead_code)] // consumed by a disabled-button call site in #71–#76
pub(crate) fn button_disabled(label: &str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(px(32.0))
        .px(px(14.0))
        .rounded(px(theme::RADIUS_SM))
        .text_size(px(theme::TS_BODY))
        .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
        .bg(rgb(theme::ELEVATED))
        .text_color(rgb(theme::TEXT_DISABLED))
        .child(label.to_string())
}
