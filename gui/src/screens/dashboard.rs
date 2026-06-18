//! The dashboard screen render branch. Relocated verbatim from `app.rs` `render()` (#68).
//! Returns the ordered list of dashboard children; the shell in `app.rs` appends them
//! via `.children(...)` exactly as before.

use gpui::{div, prelude::*, px, rgb, Context, SharedString, Window};

use crate::app::CaptureApp;
use crate::components::{button, chip, icon};
use crate::daemon::WindowInfo;
use crate::state::{short_id, truncate, ConfirmKind};

impl CaptureApp {
    /// Build the dashboard screen's children, in render order: window/session prep, the
    /// Refresh/Start buttons, the mic selector, the launch field, the import row, and the
    /// Windows + Sessions two-column panel. Mirrors the prior `dash.then(...)` chain.
    pub(crate) fn render_dashboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let cmd_focused = self.cmd_focus.is_focused(window);

        // Group windows by app (first-seen order). Each group is a header (app name +
        // window count + a 🎤 radio that assigns the mic to THIS app) followed by a
        // checkbox row per window. Multi-app, multi-window; "Start" spawns one session
        // per checked window (per-window screenshots), one app-audio per app.
        let mut groups: Vec<(String, Vec<&WindowInfo>)> = Vec::new();
        for w in &self.windows {
            if let Some(g) = groups.iter_mut().find(|(name, _)| name == &w.app_name) {
                g.1.push(w);
            } else {
                groups.push((w.app_name.clone(), vec![w]));
            }
        }
        let mut window_rows: Vec<gpui::AnyElement> = Vec::new();
        for (app, ws) in &groups {
            let is_mic_app = self.mic_app.as_deref() == Some(app.as_str());
            let an = app.clone();
            let header = div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .pt_1()
                .child(div().text_color(rgb(0x9aa0a6)).child(format!("{}  ({})", app, ws.len())))
                .child(
                    // 🎤 radio: mic attaches to exactly one app (only takes effect when a
                    // device is also chosen in the mic selector below).
                    div()
                        .id(SharedString::from(format!("micapp-{app}")))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py(px(2.0))
                        .rounded_md()
                        .cursor_pointer()
                        .bg(if is_mic_app { rgb(0x3a5f3a) } else { rgb(0x242424) })
                        .text_color(if is_mic_app { rgb(0xc8e6c8) } else { rgb(0x808080) })
                        .child(icon("mic", 12.0, if is_mic_app { 0xc8e6c8 } else { 0x808080 }))
                        .child(if is_mic_app { "mic ✓" } else { "mic" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.mic_app = if this.mic_app.as_deref() == Some(an.as_str()) {
                                None
                            } else {
                                Some(an.clone())
                            };
                            cx.notify();
                        })),
                );
            window_rows.push(header.into_any_element());
            for w in ws {
                let wid = w.window_id;
                let checked = self.checked.contains(&wid);
                let title = if w.title.trim().is_empty() {
                    "(untitled window)".to_string()
                } else {
                    truncate(&w.title, 44)
                };
                window_rows.push(
                    div()
                        .id(("win", wid as usize))
                        .flex()
                        .items_center()
                        .gap_2()
                        .pl_4()
                        .pr_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(if checked { rgb(0x2d4f67) } else { rgb(0x1e1e1e) })
                        .child(div().child(if checked { "☑" } else { "☐" }))
                        .child(div().flex_1().child(title))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.checked.remove(&wid) {
                                this.checked.insert(wid);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }

        let mut session_rows: Vec<_> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(ix, s)| {
                let running = s.state == "running";
                let open = self.selected_session.as_deref() == Some(s.session_id.as_str());
                let id = s.session_id.clone();
                let dir = s.dir.clone();
                let line = format!(
                    "{} · {} · {}s · {}seg",
                    short_id(&s.session_id),
                    s.state,
                    s.screenshots,
                    s.transcript_segments
                );
                let id_sel = id.clone();
                let mut row = div().flex().items_center().gap_1().child(
                    div()
                        .id(("sel", ix))
                        .flex_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(if open { rgb(0x24323b) } else { rgb(0x1a1a1a) })
                        .child(line)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_session(id_sel.clone(), cx);
                        })),
                );
                // Compact per-capture actions: open folder, copy a summary prompt,
                // and (for a finished capture) delete; running ones get Stop instead.
                let action = |id_str: &'static str, icon_name: &'static str, bg: u32, tint: u32| {
                    div()
                        .id((id_str, ix))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(28.0))
                        .h(px(24.0))
                        .rounded_md()
                        .cursor_pointer()
                        .bg(rgb(bg))
                        .child(icon(icon_name, 14.0, tint))
                };
                let d_folder = dir.clone();
                row = row.child(action("folder", "folder", 0x2a2a2a, 0xcfd3d6).on_click(
                    cx.listener(move |this, _, _, cx| this.open_folder(d_folder.clone(), cx)),
                ));
                let d_prompt = dir.clone();
                row = row.child(action("prompt", "clipboard", 0x2a2a2a, 0xcfd3d6).on_click(
                    cx.listener(move |this, _, _, cx| this.copy_summary_prompt(d_prompt.clone(), cx)),
                ));
                if running {
                    let id_stop = id.clone();
                    row = row.child(action("stop", "stop", 0x7a2d2d, 0xe6c0c0).on_click(
                        cx.listener(move |this, _, _, cx| this.stop_capture(id_stop.clone(), cx)),
                    ));
                } else {
                    // Delete asks first (modal); the icon opens the confirmation.
                    let id_del = id.clone();
                    row = row.child(action("del", "trash", 0x4a2a2a, 0xe6a0a0).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.confirm = Some(ConfirmKind::DeleteSession(id_del.clone()));
                            cx.notify();
                        }),
                    ));
                }
                row
            })
            .collect();
        session_rows.reverse();

        let mut out: Vec<gpui::AnyElement> = Vec::new();

        out.push(
            div()
                .flex()
                .gap_2()
                .child(button(
                    "Refresh windows",
                    cx.listener(|this, _, _, cx| this.refresh_windows(cx)),
                ))
                .child(button(
                    "Start capture",
                    cx.listener(|this, _, _, cx| this.open_preset_picker(cx)),
                ))
                .into_any_element(),
        );

        out.push({
            // Mic selector: pick ONE input device to add (None = no mic). It records
            // as a SEPARATE track on whichever app you tag with the 🎤 radio above.
            let mut row = div()
                .flex()
                .gap_2()
                .items_center()
                .flex_wrap()
                .child(div().min_w(px(60.0)).text_color(rgb(0x9aa0a6)).child("Mic:"))
                .child(chip(
                    "mic-none",
                    "No mic",
                    self.mic_device.is_none(),
                    cx.listener(|this, _, _, cx| {
                        this.mic_device = None;
                        this.save_settings();
                        cx.notify();
                    }),
                ));
            for dev in &self.mics {
                let id = dev.id.clone();
                let selected = self.mic_device.as_deref() == Some(dev.id.as_str());
                let label = format!("{}{}", dev.name, if dev.default { " (default)" } else { "" });
                row = row.child(chip(
                    &format!("mic-{}", dev.id),
                    &label,
                    selected,
                    cx.listener(move |this, _, _, cx| {
                        this.mic_device = Some(id.clone());
                        this.save_settings();
                        cx.notify();
                    }),
                ));
            }
            if self.mics.is_empty() {
                row = row.child(
                    div()
                        .text_color(rgb(0x6a6a6a))
                        .child("(no devices yet — Refresh windows)"),
                );
            }
            row.into_any_element()
        });

        out.push({
            // Launch-and-capture a new process or URL: a minimal single-line input
            // (click to focus, type, ⌘V to paste, Enter or the button to launch).
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(div().text_color(rgb(0x9aa0a6)).child("Launch:"))
                .child(
                    div()
                        .id("cmd-input")
                        .track_focus(&self.cmd_focus)
                        .key_context("cmd")
                        .on_key_down(cx.listener(Self::on_cmd_key))
                        .flex_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(if cmd_focused { rgb(0x3d6a87) } else { rgb(0x2a2a2a) })
                        .bg(rgb(0x1e1e1e))
                        .text_color(if self.cmd_input.is_empty() {
                            rgb(0x666b6f)
                        } else {
                            rgb(0xe0e0e0)
                        })
                        .child(if self.cmd_input.is_empty() {
                            #[cfg(target_os = "macos")]
                            { "command or URL — e.g. open https://…  (Enter to launch)".to_string() }
                            #[cfg(target_os = "windows")]
                            { "command or URL — e.g. cmd /c start https://…  (Enter to launch)".to_string() }
                            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                            { "command or URL — e.g. xdg-open https://…  (Enter to launch)".to_string() }
                        } else {
                            format!("{}▏", self.cmd_input)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            window.focus(&this.cmd_focus);
                            cx.notify();
                        })),
                )
                .child(button(
                    "Launch & Capture",
                    cx.listener(|this, _, _, cx| this.launch_command(cx)),
                ))
                .into_any_element()
        });

        out.push({
            // Import an existing audio/video file as a session (native file picker →
            // daemon extracts audio/frames + runs ASR; progress streams over SSE).
            let importing = self.live.lock().unwrap().import_progress.clone();
            let mut row = div()
                .flex()
                .gap_2()
                .items_center()
                .child(div().text_color(rgb(0x9aa0a6)).child("Import:"))
                .child(button(
                    "Import audio/video…",
                    cx.listener(|this, _, _, cx| this.import_file(cx)),
                ));
            if let Some((phase, frac)) = importing {
                row = row.child(
                    div()
                        .text_color(rgb(0x8ab4f8))
                        .child(format!("{} {}%", phase, (frac * 100.0) as i32)),
                );
            }
            row.into_any_element()
        });

        out.push(
            div()
                .flex()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_color(rgb(0x9aa0a6)).child("Windows"))
                        .children(window_rows),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_color(rgb(0x9aa0a6)).child("Sessions"))
                        .children(session_rows),
                )
                .into_any_element(),
        );

        out
    }
}
