//! Overlay renders: the confirmation modal (delete / destructive prune / update) and the
//! start-capture preset picker. Relocated verbatim from `app.rs` `render()` (#68). Returned
//! as an ordered list and appended by the shell exactly as before (confirm, then picker).

use gpui::{div, prelude::*, px, rgb, rgba, Context, SharedString};

use crate::app::CaptureApp;
use crate::components::{button, icon};
use crate::state::{short_id, CAPTURE_PRESETS, ConfirmKind};
use crate::theme;

impl CaptureApp {
    /// Build the overlay children in render order: the confirmation modal (if a destructive
    /// action is pending) then the preset picker (if open). Mirrors the prior two
    /// `.children(self.confirm…)` / `.children(self.show_preset_picker.then(…))` calls.
    pub(crate) fn render_overlays(&mut self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut out: Vec<gpui::AnyElement> = Vec::new();

        // Confirmation modal (delete / destructive prune) — occluding backdrop + card.
        if let Some(kind) = self.confirm.clone() {
            let (title, body, label): (&str, String, &str) = match &kind {
                ConfirmKind::DeleteSession(sid) => (
                    "Delete this capture?",
                    format!("{} — removes the folder and its record. This can't be undone.", short_id(sid)),
                    "Delete",
                ),
                ConfirmKind::Prune(_, _, body) => ("Prune this capture?", body.clone(), "Remove"),
                ConfirmKind::Update(info) => (
                    "Update Capture?",
                    format!(
                        "Download v{} and install it. The app will quit and relaunch (stop any running captures first).",
                        info.version
                    ),
                    "Update",
                ),
            };
            out.push(
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
                            .gap_3()
                            .w(px(340.0))
                            .p_4()
                            .rounded_lg()
                            .bg(rgb(theme::PANEL))
                            .child(div().text_lg().child(title))
                            .child(div().text_color(rgb(theme::TEXT_SECONDARY)).child(body))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(button(
                                        "Cancel",
                                        cx.listener(|this, _, _, cx| {
                                            this.confirm = None;
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        div()
                                            .id("confirm-go")
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .px_3()
                                            .py_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .bg(rgb(theme::ERROR_SUBTLE))
                                            .child(icon("trash", 14.0, theme::ERROR))
                                            .child(label)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.confirm = None;
                                                match kind.clone() {
                                                    ConfirmKind::DeleteSession(sid) => this.delete_session(sid, cx),
                                                    ConfirmKind::Prune(sid, parts, _) => this.prune(sid, parts, cx),
                                                    ConfirmKind::Update(info) => this.start_update(info, cx),
                                                }
                                            })),
                                    ),
                            ),
                    )
                    .into_any_element(),
            );
        }

        // Start-capture preset picker — occluding backdrop + a card listing the 6
        // presets (label + one-line hint). Picking one applies its toggles + starts.
        if self.show_preset_picker {
            let mut card = div()
                .id("preset-card")
                .track_scroll(&self.preset_scroll)
                .flex()
                .flex_col()
                .gap_2()
                .w(px(400.0))
                .max_h_full()
                .overflow_y_scroll() // cap to the viewport (minus the overlay padding) + scroll
                .p_4()
                .rounded_lg()
                .bg(rgb(theme::PANEL))
                .child(div().text_lg().child("Start capture"))
                .child(
                    div()
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child("Pick a preset — it sets the mic/screenshots and how the index reads the screen."),
                );
            for (id, label, hint) in CAPTURE_PRESETS {
                let pid = id.to_string();
                card = card.child(
                    div()
                        .id(SharedString::from(format!("preset-{id}")))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(rgb(theme::ELEVATED))
                        .hover(|s| s.bg(rgb(theme::ACCENT_SUBTLE)))
                        .child(div().text_color(rgb(theme::TEXT_PRIMARY)).child(*label))
                        .child(div().text_sm().text_color(rgb(theme::TEXT_MUTED)).child(*hint))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start_with_preset(&pid, cx);
                        })),
                );
            }
            card = card.child(
                div().flex().justify_end().child(button(
                    "Cancel",
                    cx.listener(|this, _, _, cx| {
                        this.show_preset_picker = false;
                        cx.notify();
                    }),
                )),
            );
            out.push(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .p_6() // margins so the card never touches the window edges
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(theme::BACKDROP))
                    .occlude()
                    .child(card)
                    .into_any_element(),
            );
        }

        out
    }
}
