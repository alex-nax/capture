//! The dashboard screen render branch. Relocated verbatim from `app.rs` `render()` (#68).
//! Returns the ordered list of dashboard children; the shell in `app.rs` appends them
//! via `.children(...)` exactly as before.

use gpui::{div, prelude::*, px, relative, rgb, rgba, Context, SharedString, Window};

use crate::app::CaptureApp;
use crate::components::{button, card, checkbox, chip, column_header, icon, ButtonVariant};
use crate::daemon::WindowInfo;
use crate::state::{short_id, truncate, ConfirmKind};
use crate::theme;

impl CaptureApp {
    /// Build the dashboard body (the shell in `app.rs` renders the page header above it).
    /// Returns three children in order: the Mic device row, the Launch input row, and the
    /// two-column grid (Windows card + Sessions card). The Windows card groups windows by
    /// app with a per-app "Start capture" button (enabled once a window of that app is
    /// checked); the Sessions card lists captures newest-first with per-capture actions.
    pub(crate) fn render_dashboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let cmd_focused = self.cmd_focus.is_focused(window);

        // ── Mic row ──────────────────────────────────────────────────────────────
        // Pick ONE input device to add (None = no mic). Records as a separate track on
        // the captured app's audio. The leading label aligns with the Launch label.
        let mic_row = {
            let mut chips = div().flex().items_center().gap(px(theme::SP_2)).flex_wrap().child(chip(
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
                chips = chips.child(chip(
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
            div()
                .flex()
                .items_center()
                .gap(px(theme::SP_2))
                .child(
                    div()
                        .flex_none()
                        .w(px(54.0))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child("Mic"),
                )
                .child(chips)
                .into_any_element()
        };

        // ── Launch row ───────────────────────────────────────────────────────────
        // A minimal single-line input (click to focus, type, ⌘V to paste, Enter or the
        // button to launch) plus the primary launch button.
        let launch_row = div()
            .flex()
            .items_center()
            .gap(px(theme::SP_2))
            .child(
                div()
                    .flex_none()
                    .w(px(54.0))
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .child("Launch"),
            )
            .child(
                div()
                    .id("cmd-input")
                    .track_focus(&self.cmd_focus)
                    .key_context("cmd")
                    .on_key_down(cx.listener(Self::on_cmd_key))
                    .flex_1()
                    .px(px(theme::SP_3))
                    .py(px(theme::SP_2))
                    .rounded(px(theme::RADIUS_SM))
                    .border_1()
                    .border_color(if cmd_focused { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
                    .bg(rgb(theme::BG))
                    .text_color(if self.cmd_input.is_empty() {
                        rgb(theme::TEXT_MUTED)
                    } else {
                        rgb(theme::TEXT_PRIMARY)
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
                ButtonVariant::Primary,
                cx.listener(|this, _, _, cx| this.launch_command(cx)),
            ))
            .into_any_element();

        // ── Windows card ─────────────────────────────────────────────────────────
        // Group windows by app (first-seen order). Each group is a header (app name +
        // window count + a per-app "Start capture" button) followed by a checkbox row
        // per window. The Start button is enabled once at least one window of THAT app
        // is checked; clicking it opens the preset picker. "Start" then spawns one
        // session per checked window (per-window screenshots), one app-audio per app.
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
            let any_checked = ws.iter().any(|w| self.checked.contains(&w.window_id));
            let group_header = div()
                .flex()
                .items_center()
                .gap(px(theme::SP_2))
                .py(px(7.0))
                .px(px(theme::SP_2))
                .overflow_hidden()
                .child(
                    div()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_size(px(theme::TS_BODY))
                        .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(app.clone()),
                )
                .child(
                    div()
                        .text_size(px(theme::TS_SMALL))
                        .text_color(rgb(theme::TEXT_DISABLED))
                        .child(ws.len().to_string()),
                )
                .child(div().flex_1())
                .child({
                    // Compact per-app Start button with a leading status dot (design §dashboard):
                    // indigo + white dot when a window of this app is checked, else flat + grey dot.
                    let dot = |c: u32| div().flex_none().size(px(6.0)).rounded_full().bg(rgb(c));
                    let base = div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(6.0))
                        .py(px(4.0))
                        .px(px(10.0))
                        .rounded(px(theme::RADIUS_SM))
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32));
                    if any_checked {
                        base.id(SharedString::from(format!("start-{app}")))
                            .cursor_pointer()
                            .bg(rgb(theme::ACCENT))
                            .text_color(rgb(theme::ON_ACCENT))
                            .hover(|s| s.bg(rgb(theme::ACCENT_HOVER)))
                            .child(dot(theme::ON_ACCENT))
                            .child("Start capture")
                            .on_click(cx.listener(|this, _, _, cx| this.open_preset_picker(cx)))
                            .into_any_element()
                    } else {
                        base.bg(rgb(theme::CHIP_DISABLED))
                            .text_color(rgb(theme::TEXT_DISABLED))
                            .child(dot(theme::TEXT_DISABLED))
                            .child("Start capture")
                            .into_any_element()
                    }
                });
            window_rows.push(group_header.into_any_element());
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
                        .gap(px(theme::SP_2))
                        .pl(px(theme::SP_5))
                        .pr(px(theme::SP_2))
                        .py(px(theme::SP_2))
                        .rounded(px(theme::RADIUS_MD))
                        .overflow_hidden()
                        .cursor_pointer()
                        // Unselected = flat (the design boxes ONLY the selected row). A 1px
                        // transparent border keeps the row height stable when selection toggles.
                        .border_1()
                        .border_color(if checked { rgb(theme::ACCENT_BORDER) } else { rgba(theme::TRANSPARENT) })
                        .when(checked, |d| d.bg(rgb(theme::ACCENT_SUBTLE)))
                        .when(!checked, |d| d.hover(|s| s.bg(rgb(theme::ELEVATED))))
                        .child(checkbox(checked))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_size(px(theme::TS_BODY))
                                .text_color(if checked {
                                    rgb(theme::TEXT_PRIMARY)
                                } else {
                                    rgb(theme::TEXT_SECONDARY)
                                })
                                .child(title),
                        )
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
        let windows_header = div()
            .flex()
            .items_center()
            .gap(px(theme::SP_2))
            .mb(px(theme::SP_3))
            .child(column_header("Windows", Some(self.windows.len())))
            .child(div().flex_1())
            .child(
                div()
                    .id("refresh-windows")
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .py(px(4.0))
                    .px(px(9.0))
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .text_size(px(theme::TS_SMALL))
                    .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .hover(|s| s.border_color(rgb(theme::BORDER_STRONG)))
                    .child(icon("refresh", 14.0, theme::TEXT_SECONDARY))
                    .child("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_windows(cx))),
            );
        let windows_card = card(
            div()
                .flex()
                .flex_col()
                .child(windows_header)
                .child(div().flex().flex_col().gap(px(2.0)).children(window_rows)),
        );

        // ── Sessions card ────────────────────────────────────────────────────────
        // Captures, newest first. Each row is a selectable container with a 2px accent
        // left-bar (selection marker) holding a clickable SELECT area (id + meta) and,
        // as SIBLINGS so a click doesn't also select, per-capture action icons: open
        // folder, copy summary prompt, and stop (running) / delete (finished).
        // No persistent "selected" treatment — clicking a row opens Playback, so the row chrome
        // reflects its REAL state instead (design/TASK-session-row-states): live (red dot + Stop),
        // indexing (status + a bottom progress fill), or stopped (plain folder/copy/trash).
        let index_progress = self.live.lock().unwrap().index_progress.clone();
        let mut session_rows: Vec<gpui::AnyElement> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(ix, s)| {
                let running = s.state == "running";
                let indexing = index_progress.get(&s.session_id).map(|(p, f)| (p.clone(), *f));
                let id = s.session_id.clone();
                let dir = s.dir.clone();
                let sid = short_id(&s.session_id).to_string();

                // The status line: a leading dot (live) / refresh glyph (indexing) + text, per state.
                let status = if running {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(div().flex_none().size(px(6.0)).rounded_full().bg(rgb(theme::LIVE)))
                        .child(
                            div().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::LIVE)).child(
                                format!("live · {}s · {} seg", s.screenshots, s.transcript_segments),
                            ),
                        )
                } else if let Some((phase, frac)) = &indexing {
                    // Live (auto) indexing is open-ended → "indexing…"; an on-demand build → "· NN%".
                    let label = if phase == "live" {
                        "indexing…".to_string()
                    } else {
                        format!("indexing · {}%", (*frac * 100.0).round() as i32)
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(icon("refresh", 12.0, theme::ACCENT_TEXT))
                        .child(
                            div()
                                .text_size(px(theme::TS_SMALL))
                                .text_color(rgb(theme::ACCENT_TEXT))
                                .child(label),
                        )
                } else {
                    div().min_w(px(0.0)).overflow_hidden().child(
                        div()
                            .text_size(px(theme::TS_SMALL))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(format!("{} · {}s · {} seg", s.state, s.screenshots, s.transcript_segments)),
                    )
                };

                let id_sel = id.clone();
                let select_area = div()
                    .id(("sel", ix))
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(theme::SP_2))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(theme::TS_BODY))
                            .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .child(sid),
                    )
                    .child(status)
                    .on_click(cx.listener(move |this, _, _, cx| this.select_session(id_sel.clone(), cx)));

                let action = |id_str: &'static str, icon_name: &'static str, tint: u32| {
                    div()
                        .id((id_str, ix))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .cursor_pointer()
                        .child(icon(icon_name, 14.0, tint))
                };
                let d_folder = dir.clone();
                let d_prompt = dir.clone();
                let mut row = div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(theme::SP_3))
                    .py(px(6.0))
                    .px(px(theme::SP_3))
                    .rounded(px(theme::RADIUS_MD))
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(theme::CARD_BORDER))
                    .bg(rgb(theme::PANEL))
                    .child(select_area)
                    .child(action("folder", "folder", theme::TEXT_MUTED).on_click(
                        cx.listener(move |this, _, _, cx| this.open_folder(d_folder.clone(), cx)),
                    ))
                    .child(action("prompt", "clipboard", theme::TEXT_MUTED).on_click(
                        cx.listener(move |this, _, _, cx| this.copy_summary_prompt(d_prompt.clone(), cx)),
                    ));
                if running {
                    // Live: a red Stop in place of trash (ends the capture; does NOT delete).
                    let id_stop = id.clone();
                    row = row.child(action("stop", "stop", theme::LIVE).on_click(cx.listener(
                        move |this, _, _, cx| this.stop_capture(id_stop.clone(), cx),
                    )));
                } else {
                    // Trash (delete) — same reliable icon() fill as folder/copy.
                    let id_del = id.clone();
                    row = row.child(action("del", "trash", theme::TEXT_MUTED).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.confirm = Some(ConfirmKind::DeleteSession(id_del.clone()));
                            cx.notify();
                        },
                    )));
                }
                // Indexing: a 2px ACCENT fill along the bottom edge, width = percent.
                if let Some((_, frac)) = &indexing {
                    row = row.child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .h(px(2.0))
                            .w(relative(frac.clamp(0.0, 1.0)))
                            .bg(rgb(theme::ACCENT)),
                    );
                }
                row.into_any_element()
            })
            .collect();
        session_rows.reverse();

        // Import an existing audio/video file as a session (native file picker → daemon
        // extracts audio/frames + runs ASR; progress streams over SSE and shows inline).
        let importing = self.live.lock().unwrap().import_progress.clone();
        let mut sessions_header = div()
            .flex()
            .items_center()
            .gap(px(theme::SP_2))
            .mb(px(theme::SP_3))
            .child(column_header("Sessions", Some(self.sessions.len())))
            .child(div().flex_1());
        if let Some((phase, frac)) = importing {
            sessions_header = sessions_header.child(
                div()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::ACCENT_TEXT))
                    .child(format!("{} {}%", phase, (frac * 100.0) as i32)),
            );
        }
        sessions_header = sessions_header.child(
            div()
                .id("import-file")
                .flex()
                .items_center()
                .gap(px(6.0))
                .py(px(4.0))
                .px(px(9.0))
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .border_1()
                .border_color(rgb(theme::BORDER))
                .text_size(px(theme::TS_SMALL))
                .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                .text_color(rgb(theme::TEXT_SECONDARY))
                .hover(|s| s.border_color(rgb(theme::BORDER_STRONG)))
                .child(icon("folder", 14.0, theme::TEXT_SECONDARY))
                .child("Import…")
                .on_click(cx.listener(|this, _, _, cx| this.import_file(cx))),
        );
        let sessions_card = card(
            div()
                .flex()
                .flex_col()
                .child(sessions_header)
                .child(div().flex().flex_col().gap(px(theme::SP_1)).children(session_rows)),
        );

        // ── Two-column grid ──────────────────────────────────────────────────────
        // Responsive: two equal columns on a wide window, STACKED on a narrow one (so it
        // never clips). min_w(0) lets a flex column shrink below its content width, so long
        // titles/meta clip (via the rows' overflow_hidden) instead of overflowing the window.
        let narrow = window.viewport_size().width < px(720.0);
        let grid = if narrow {
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SP_5))
                .child(div().w_full().min_w(px(0.0)).child(windows_card))
                .child(div().w_full().min_w(px(0.0)).child(sessions_card))
        } else {
            div()
                .flex()
                .items_start()
                .gap(px(theme::SP_5))
                .child(div().flex_1().min_w(px(0.0)).child(windows_card))
                .child(div().flex_1().min_w(px(0.0)).child(sessions_card))
        }
        .into_any_element();

        // The transcription-setup CTA (#83) sits above the mic row — hero / partial / minimised pill
        // when ASR isn't ready, or a compact "Transcription is on" confirmation when it is.
        let mut out: Vec<gpui::AnyElement> = Vec::new();
        if let Some(cta) = self.render_asr_cta(cx) {
            out.push(cta);
        }
        out.push(mic_row);
        out.push(launch_row);
        out.push(grid);
        out
    }

    /// The "turn on transcription" call-to-action (#83). ASR ships unbundled, so until a runtime
    /// engine AND a model are both installed, captures record but don't transcribe — this invites the
    /// user to set it up (deep-linking to Settings → Voice), and confirms once it's on. Returns `None`
    /// only in the rare no-data state.
    fn render_asr_cta(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let active_rt = self.runtimes.runtimes.iter().find(|r| r.active);
        let engine_installed = active_rt.map(|r| r.kind == "local" && r.installed).unwrap_or(false);
        let model_downloaded = self.asr.models.iter().any(|m| m.downloaded);
        let ready = engine_installed && model_downloaded;
        let dismissed = self.asr_cta_dismissed;

        // A leading icon in a rounded accent-subtle (or success) square.
        let icon_square = |name: &str, tint: u32, bg: u32| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(40.0))
                .rounded(px(theme::RADIUS_MD))
                .bg(rgb(bg))
                .child(icon(name, 18.0, tint))
        };
        if ready {
            // State 5 — "Transcription is on" confirmation strip.
            let rt_label = active_rt.map(|r| r.label.clone()).unwrap_or_default();
            let model_name = self.asr.models.iter().find(|m| m.active).map(|m| m.name.clone()).unwrap_or_default();
            return Some(
                card(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SP_3))
                        .child(icon_square("check", theme::SUCCESS, theme::SUCCESS_SUBTLE))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(theme::TS_HEADING))
                                        .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                                        .text_color(rgb(theme::TEXT_PRIMARY))
                                        .child("Transcription is on"),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TS_SMALL))
                                        .text_color(rgb(theme::TEXT_MUTED))
                                        .child(format!("{rt_label} · {model_name} · new captures now transcribe")),
                                ),
                        )
                        .child(crate::components::button("Settings → Voice", ButtonVariant::Ghost,
                            cx.listener(|this, _, _, cx| this.open_voice_settings(cx)))),
                )
                .into_any_element(),
            );
        }

        // Not ready. Pick the message for the missing piece.
        let (title, body, btn_label, btn_id) = if engine_installed && !model_downloaded {
            let rt = active_rt.map(|r| r.label.clone()).unwrap_or_else(|| "The engine".into());
            ("Almost there — pick a model", format!("{rt} is installed — one model finishes setup."), "Pick a model", "asr-cta-model")
        } else if !engine_installed && model_downloaded {
            ("Almost there — pick an engine", "A model is ready — pick a speech engine to finish.".to_string(), "Pick an engine", "asr-cta-engine")
        } else {
            ("Turn on transcription",
             "Transcription is off — it needs a speech engine and a model, downloaded once. Captures keep recording audio + screenshots meanwhile.".to_string(),
             "Set up transcription", "asr-cta-setup")
        };

        // Fully-dismissed "none" state → the minimised pill by the rows; everything else → the hero card.
        let none_state = !engine_installed && !model_downloaded;
        if none_state && dismissed {
            return Some(
                div()
                    .id("asr-cta-pill")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(theme::SP_2))
                    .py(px(theme::SP_1))
                    .px(px(theme::SP_3))
                    .rounded(px(theme::RADIUS_SM))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .child(icon("volume", 14.0, theme::TEXT_MUTED))
                    .child(div().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::TEXT_MUTED)).child("Transcription is off"))
                    .child(div().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::TEXT_DISABLED)).child("·"))
                    .child(div().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::ACCENT_TEXT)).child("Set it up"))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.open_voice_settings(cx)))
                    .into_any_element(),
            );
        }

        let mut actions = div().flex().items_center().gap(px(theme::SP_3));
        if none_state {
            // Only the hero offers "Not now" (→ the minimised pill).
            actions = actions.child(crate::components::button("Not now", ButtonVariant::Ghost,
                cx.listener(|this, _, _, cx| { this.asr_cta_dismissed = true; cx.notify(); })));
        }
        actions = actions.child(crate::components::button_id(
            btn_id,
            btn_label,
            ButtonVariant::Primary,
            cx.listener(|this, _, _, cx| this.open_voice_settings(cx)),
        ));

        Some(
            card(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SP_3))
                    .child(icon_square("volume", theme::ACCENT_TEXT, theme::ACCENT_SUBTLE))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(px(theme::TS_HEADING))
                                    .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                                    .text_color(rgb(theme::TEXT_PRIMARY))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TS_SMALL))
                                    .line_height(px(18.0))
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .child(body),
                            ),
                    )
                    .child(actions),
            )
            .into_any_element(),
        )
    }

    /// Deep-link to Settings → Voice (the runtime + model pickers) — the CTA's destination.
    fn open_voice_settings(&mut self, cx: &mut Context<Self>) {
        self.playback = None;
        self.show_settings = true;
        self.settings_section = crate::state::SettingsSection::Voice;
        cx.notify();
    }
}
