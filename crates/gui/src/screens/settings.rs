//! The Settings screen: a two-pane frame (left nav + content) matching the LOCKED
//! design artboard (`design/unpacked/_template.html`, the SETTINGS block). The nav
//! switches `settings_section`; the content pane renders that section's card(s).
//! Markup/styling only — every listener, focus handle, and the section gating below
//! is preserved verbatim from the prior single-column implementation (#68/#71).

use gpui::{
    deferred, div, prelude::*, px, relative, rgb, rgba, Context, MouseButton, SharedString, Window,
};

use crate::app::CaptureApp;
use crate::components::{
    button, button_id, button_sm_id, card, chip, eyebrow, icon, progress_bar,
    status_pill, ButtonVariant,
};
use crate::skill;
use crate::state::{ConfirmKind, IndexField, SettingsSection, INDEX_PROVIDERS, LANGUAGES, RES_PRESETS};
use crate::theme;
use crate::update;

impl CaptureApp {
    /// Build the Settings screen: a single two-pane row (left nav · content), min-height
    /// 580. The content pane shows the active section's card(s). Returns the screen's
    /// children (the shell in `app.rs` appends them via `.children(...)`).
    pub(crate) fn render_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let dropdown_open = self.model_dropdown_open || self.lang_dropdown_open;
        let nav = self.settings_nav(cx);
        let content = self.settings_content(window, cx);
        let mut body = div()
            .flex()
            .size_full() // fill the window; the nav is fixed-width, the content pane scrolls
            .relative()
            .child(nav)
            .child(content);
        if dropdown_open {
            // Click-anywhere-outside backdrop that dismisses an open dropdown popover. It occludes
            // the panes (an outside click only closes — it doesn't also hit the control beneath),
            // and sits BELOW the deferred menu, so clicks on the menu still select.
            body = body.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.model_dropdown_open = false;
                            this.lang_dropdown_open = false;
                            cx.notify();
                        }),
                    ),
            );
        }
        vec![body.into_any_element()]
    }

    /// The content pane (right side): the section header (title + Back) then the active
    /// section's card(s). `flex-1, min-width 0, padding 22×26, gap 16`.
    fn settings_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.settings_section;
        let header = div()
            .flex()
            .justify_between()
            .items_center()
            .child(
                div()
                    .text_size(px(theme::TS_SECTION))
                    .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child(section.label()),
            )
            .child(
                // Back to the dashboard. Same effect as the shell's header Back button.
                div()
                    .id("settings-back")
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(11.0))
                    .py(px(7.0))
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .text_color(rgb(theme::TEXT_SECONDARY))
                    .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                    .text_size(px(theme::TS_BODY))
                    .hover(|s| s.bg(rgba(theme::GHOST_HOVER)))
                    .child(icon("chevron-left", 15.0, theme::TEXT_SECONDARY))
                    .child("Back")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_settings = false;
                        cx.notify();
                    })),
            );

        let mut content: Vec<gpui::AnyElement> = Vec::new();
        match section {
            SettingsSection::CaptureQuality => content.push(self.section_capture(cx)),
            SettingsSection::Transcription => content.push(self.section_transcription(window, cx)),
            SettingsSection::Voice => content.extend(self.section_voice(cx)),
            SettingsSection::IndexEndpoint => content.push(self.section_index(window, cx)),
            SettingsSection::Skills => content.push(self.section_skills(cx)),
            SettingsSection::Permissions => content.push(self.section_permissions(cx)),
            SettingsSection::Updates => content.push(self.section_updates(cx)),
        }

        div()
            .id("settings-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll() // long sections scroll within the pane, not the whole window
            .flex()
            .flex_col()
            .gap(px(theme::SP_4))
            .px(px(26.0))
            .py(px(22.0))
            .child(header)
            .children(content)
    }

    /// The Settings left-nav (#71): brand + daemon line at top, the section list, a spacer,
    /// then the hotkey hint + daemon status pinned to the bottom. Clicking a row sets
    /// `settings_section` (→ re-render).
    pub(crate) fn settings_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (daemon_line, reachable) = match &self.health {
            Some(h) if h.ok => (format!("daemon v{} · pid {}", h.version, h.pid), true),
            _ => ("no daemon".to_string(), false),
        };

        // Brand block: "Capture" 15/600 + the daemon line in mono 11px below it.
        let brand = div()
            .px(px(6.0))
            .child(
                div()
                    .text_size(px(theme::TS_HEADING))
                    .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child("Capture"),
            )
            .child(
                // The frame renders this in mono; the app bundles no mono face, so it stays
                // in the default family at the mono size/color (matches the prior nav).
                div()
                    .mt(px(5.0))
                    .text_size(px(theme::TS_EYEBROW))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(daemon_line),
            );

        // The section list: one button per SettingsSection.
        let mut items = div().flex().flex_col().gap(px(3.0));
        for s in SettingsSection::ALL {
            let active = s == self.settings_section;
            items = items.child(
                div()
                    .id(SharedString::from(s.label()))
                    .flex()
                    .items_center()
                    .gap(px(theme::SP_2) + px(2.0))
                    .w_full()
                    .px(px(10.0))
                    .py(px(theme::SP_2))
                    .rounded(px(theme::RADIUS_MD))
                    .cursor_pointer()
                    .text_size(px(theme::TS_BODY))
                    .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                    .when(active, |d| d.bg(rgb(theme::ACCENT_SUBTLE)))
                    .text_color(rgb(if active { theme::ACCENT_TEXT_STRONG } else { theme::TEXT_SECONDARY }))
                    .when(!active, |d| d.hover(|st| st.bg(rgb(theme::ELEVATED))))
                    .child(icon(s.icon(), 15.0, if active { theme::ACCENT_TEXT_STRONG } else { theme::TEXT_MUTED }))
                    .child(div().child(s.label()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_section = s;
                        cx.notify();
                    })),
            );
        }

        // Bottom block: hotkey hint (if bound) + the inline daemon status dot+label.
        let mut bottom = div()
            .pt(px(theme::SP_3))
            .border_t_1()
            .border_color(rgb(theme::HAIRLINE))
            .flex()
            .flex_col()
            .gap(px(9.0));
        if self.hotkey_id != 0 {
            bottom = bottom.child(
                div()
                    .px(px(6.0))
                    .text_size(px(theme::TS_EYEBROW))
                    .text_color(rgb(theme::ACCENT_TEXT))
                    .child(format!("{} toggles capture", crate::hotkey::LABEL)),
            );
        }
        let (dot_color, status_label) = if reachable {
            (theme::SUCCESS, "reachable")
        } else {
            (theme::ERROR, "offline")
        };
        bottom = bottom.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(6.0))
                .text_size(px(theme::TS_EYEBROW))
                .text_color(rgb(dot_color))
                .child(div().flex_none().size(px(6.0)).rounded_full().bg(rgb(dot_color)))
                .child(status_label),
        );

        div()
            .flex_none()
            .w(px(214.0))
            .flex()
            .flex_col()
            .gap(px(theme::SP_4))
            .px(px(theme::SP_3))
            .py(px(theme::SP_4))
            .bg(rgb(theme::NAV_BG))
            .border_r_1()
            .border_color(rgb(theme::HAIRLINE))
            .child(brand)
            .child(items)
            .child(div().flex_1())
            .child(bottom)
    }

    // ── Section: Capture quality ─────────────────────────────────────────────
    /// One card of `.row`s — screenshots / format / resolution / jpeg-quality.
    fn section_capture(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let is_jpeg = self.shot_format == "jpeg";
        let mut rows = div()
            .flex()
            .flex_col()
            .gap(px(13.0))
            .child(
                self.field_row("Screenshots")
                    .child(chip("cap-shots-on", "On", self.capture_screenshots, cx.listener(|this, _, _, cx| {
                        this.capture_screenshots = true;
                        this.save_settings();
                        cx.notify();
                    })))
                    .child(chip("cap-shots-off", "Off (audio only)", !self.capture_screenshots, cx.listener(|this, _, _, cx| {
                        this.capture_screenshots = false;
                        this.save_settings();
                        cx.notify();
                    }))),
            )
            .child(
                self.field_row("Format")
                    .child(chip("fmt-png", "PNG", self.shot_format == "png", cx.listener(|this, _, _, cx| {
                        this.shot_format = "png".into();
                        this.save_settings();
                        cx.notify();
                    })))
                    .child(chip("fmt-jpeg", "JPEG", is_jpeg, cx.listener(|this, _, _, cx| {
                        this.shot_format = "jpeg".into();
                        this.save_settings();
                        cx.notify();
                    }))),
            )
            .child(
                self.field_row("Resolution")
                    .children(RES_PRESETS.iter().enumerate().map(|(i, p)| {
                        chip(&format!("res-{i}"), p.0, self.shot_res_ix == i, cx.listener(move |this, _, _, cx| {
                            this.shot_res_ix = i;
                            this.save_settings();
                            cx.notify();
                        }))
                    })),
            );
        if is_jpeg {
            rows = rows.child(
                self.field_row("JPEG quality")
                    .children([60u32, 80, 95].into_iter().map(|q| {
                        chip(&format!("q-{q}"), &q.to_string(), self.jpeg_quality == q, cx.listener(move |this, _, _, cx| {
                            this.jpeg_quality = q;
                            this.save_settings();
                            cx.notify();
                        }))
                    })),
            );
        }
        card(rows).into_any_element()
    }

    // ── Section: Transcription ───────────────────────────────────────────────
    /// One card: Language (searchable dropdown) + Chunk (chips).
    fn section_transcription(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let asr_lang_focused = self.asr_language_focus.is_focused(window);
        // Transcription settings (#45): language + chunk length. Pinning the language
        // stops Whisper hallucinating "Thank you." on short non-English chunks; a 30s
        // chunk is the reliable default.
        card(
            div()
                .flex()
                .flex_col()
                .gap(px(13.0))
                .child(self.language_field(asr_lang_focused, false, cx))
                .child(self.chunk_chips(cx)),
        )
        .into_any_element()
    }

    // ── Section: Voice recognition (#83 ASR onboarding) ──────────────────────
    /// A two-step picker. Step 1 = the speech-engine list (one row per registry runtime, with
    /// download / use / unavailable / in-progress states). Once a runtime is active the list
    /// collapses to a one-line "✓ Engine: … · Change" summary and Step 2 appears: the model
    /// catalog for that runtime (local), the endpoint card (remote), or a CUDA-unavailable note.
    /// "Change" re-expands the engine list (`asr_engine_expanded`). Transcription is on only when
    /// BOTH a runtime and a model are active — nothing here auto-selects.
    fn section_voice(&mut self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let (asr_progress, rt_install, asr_dl, asr_failed) = {
            let live = self.live.lock().unwrap();
            (
                live.asr_progress.clone(),
                live.runtime_install.clone(),
                // Flatten DlStat into a plain tuple map so render doesn't hold the lock.
                live.asr_dl
                    .iter()
                    .map(|(k, v)| (k.clone(), (v.done, v.total, v.speed_bps)))
                    .collect::<std::collections::HashMap<String, (u64, u64, f64)>>(),
                live.asr_failed.clone(),
            )
        };
        let nvidia = self.runtimes.gpu.nvidia;

        // Step 2 shows the config for the engine the user is CONFIGURING — their explicit pick
        // (asr_engine_selected), else the resolved-active engine. This decouples "what am I
        // configuring" from "what's actually running": picking Remote shows Remote's config even
        // though the daemon keeps a local engine resolved-active until Remote has an endpoint.
        let active_id = self.runtimes.runtimes.iter().find(|r| r.active).map(|r| r.id.clone());
        let selected_id = self.asr_engine_selected.clone().or(active_id);
        let selected_rt = selected_id
            .as_ref()
            .and_then(|id| self.runtimes.runtimes.iter().find(|r| &r.id == id).cloned());
        let sel_remote = selected_rt.as_ref().map(|r| r.kind == "remote").unwrap_or(false);
        let sel_local = selected_rt.as_ref().map(|r| r.kind == "local").unwrap_or(false);
        let sel_cuda_unavailable = selected_rt
            .as_ref()
            .map(|r| Self::runtime_unavailable(r, nvidia))
            .unwrap_or(false);
        let sel_label = selected_rt.as_ref().map(|r| r.label.clone()).unwrap_or_default();

        // The engine list is shown while setting up (nothing selectable yet) or when "Change" is
        // clicked; otherwise it collapses to the summary strip + Step 2.
        let expanded = selected_rt.is_none() || self.asr_engine_expanded;

        let mut out: Vec<gpui::AnyElement> = Vec::new();

        if expanded {
            // Step-1 banner only before any engine is chosen — don't nag once one is selected.
            if selected_rt.is_none() {
                out.push(self.voice_step_banner(
                    1,
                    "Pick a speech engine, then a model — transcription turns on once both are set.",
                    "Step 1 of 2",
                ));
            }
            out.push(self.engine_picker_card(&rt_install, nvidia, cx));
        } else {
            out.push(self.engine_summary_strip(&sel_label, cx));
            if sel_remote {
                out.push(self.remote_config_card());
            } else if sel_cuda_unavailable {
                out.push(self.cuda_unavailable_box());
            } else if sel_local {
                out.push(self.model_picker_card(&asr_progress, &asr_dl, &asr_failed, &sel_label, cx));
            }
        }

        out
    }

    /// A runtime can't run here if it needs an NVIDIA GPU and none is present. (Remote never
    /// "needs" hardware.) Derived from the registry `requires`/`device` strings + the GPU probe.
    fn runtime_unavailable(rt: &crate::daemon::AsrRuntime, nvidia: bool) -> bool {
        if rt.kind == "remote" {
            return false;
        }
        let req = rt.requires.to_lowercase();
        (req.contains("nvidia") || req.contains("cuda") || rt.device.as_deref() == Some("cuda")) && !nvidia
    }

    /// A numbered step banner (`accent_subtle` strip): the step badge + one-line guidance +
    /// a right-aligned "Step N of M" marker.
    fn voice_step_banner(&self, n: u32, body: &str, step: &str) -> gpui::AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(theme::SP_3))
            .px(px(16.0))
            .py(px(12.0))
            .rounded(px(theme::RADIUS_MD))
            .border_1()
            .border_color(rgb(theme::ACCENT_BORDER))
            .bg(rgb(theme::ACCENT_SUBTLE))
            .child(step_badge(n))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(theme::TS_BODY))
                    .line_height(px(18.0))
                    .text_color(rgb(theme::ACCENT_TEXT_STRONG))
                    .child(body.to_string()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::ACCENT_TEXT))
                    .child(step.to_string()),
            )
            .into_any_element()
    }

    /// The collapsed engine summary (shown once a runtime is active): "✓ Engine: <label>" with a
    /// ghost "Change" that re-expands the engine list.
    fn engine_summary_strip(&self, label: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(theme::SP_2))
            .px(px(16.0))
            .py(px(12.0))
            .rounded(px(theme::RADIUS_LG))
            .border_1()
            .border_color(rgb(theme::CARD_BORDER))
            .bg(rgb(theme::PANEL))
            .child(icon("check", 15.0, theme::SUCCESS))
            .child(
                div()
                    .text_size(px(theme::TS_BODY))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("Engine:"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(theme::TS_BODY))
                    .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child(label.to_string()),
            )
            .child(button_id(
                "voice-engine-change",
                "Change",
                ButtonVariant::Ghost,
                cx.listener(|this, _, _, cx| {
                    this.asr_engine_expanded = true;
                    cx.notify();
                }),
            ))
            .into_any_element()
    }

    /// The speech-engine card (Step 1): eyebrow + one-liner + a row per registry runtime.
    fn engine_picker_card(
        &mut self,
        rt_install: &std::collections::HashMap<String, f32>,
        nvidia: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let runtimes = self.runtimes.runtimes.clone();
        let mut rows = div().flex().flex_col().gap(px(4.0));
        for rt in &runtimes {
            rows = rows.child(self.runtime_row(rt, rt_install, nvidia, cx));
        }
        card(
            div()
                .flex()
                .flex_col()
                .child(eyebrow("Speech engine"))
                .child(
                    div()
                        .mt(px(theme::SP_2))
                        .mb(px(14.0))
                        .text_size(px(theme::TS_SMALL))
                        .line_height(px(18.0))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child("A small download for your hardware. It keeps itself up to date."),
                )
                .child(rows),
        )
        .into_any_element()
    }

    /// One speech-engine row: name + requirement line on the left; the state-dependent control on
    /// the right (Active · Use · Configure · Download · Unavailable · in-flight progress). The
    /// active engine gets the selected-row treatment.
    fn runtime_row(
        &mut self,
        rt: &crate::daemon::AsrRuntime,
        rt_install: &std::collections::HashMap<String, f32>,
        nvidia: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let unavailable = Self::runtime_unavailable(rt, nvidia);
        let installing = rt_install.get(&rt.id).copied();
        // The remote backend isn't wired yet (#80) — gate it as "Coming soon": disabled action + a
        // chip, so the user can't pick an engine that can't run.
        let coming_soon = rt.kind == "remote";
        let id = rt.id.clone();

        let mut right = div().flex().flex_none().items_center().gap(px(theme::SP_2));
        if let Some(frac) = installing {
            // Engine packs are small — a thin inline bar + % is enough (no byte detail).
            right = right
                .child(div().w(px(150.0)).child(progress_bar(frac, false)))
                .child(
                    div()
                        .text_size(px(theme::TS_SMALL))
                        .text_color(rgb(theme::ACCENT_TEXT))
                        .child(format!("downloading · {:.0}%", frac * 100.0)),
                );
        } else if rt.active {
            right = right
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(theme::TS_SMALL))
                        .text_color(rgb(theme::SUCCESS))
                        .child(icon("check", 13.0, theme::SUCCESS))
                        .child("Active"),
                )
                // "Configure" opens the active engine's config (the model step for a local engine, the
                // endpoint card for Remote) — it also collapses the expanded list so the user isn't
                // stranded in the picker after clicking "Change".
                .child({
                    let id_cfg = id.clone();
                    button_sm_id(
                        &format!("rt-configure-{id}"),
                        "Configure",
                        ButtonVariant::Secondary,
                        cx.listener(move |this, _, _, cx| {
                            this.asr_engine_selected = Some(id_cfg.clone());
                            this.asr_engine_expanded = false;
                            cx.notify();
                        }),
                    )
                });
        } else if unavailable {
            right = right.child(
                div()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::TEXT_DISABLED))
                    .child("Unavailable"),
            );
        } else if coming_soon {
            // Not yet selectable — a disabled "Select" (no listener, no hover) reads as "soon", paired
            // with the "Coming soon" chip under the description below.
            right = right.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .py(px(4.0))
                    .px(px(11.0))
                    .rounded(px(theme::RADIUS_SM))
                    .bg(rgb(theme::CHIP_DISABLED))
                    .text_size(px(theme::TS_SMALL))
                    .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                    .text_color(rgb(theme::TEXT_DISABLED))
                    .child("Select"),
            );
        } else if rt.installed {
            // Ready to use — an installed local engine, or Remote (which needs no download). "Select"
            // makes it the engine you're configuring (activates it) + collapses to its config: the model
            // step for a local engine, the endpoint card for Remote. Step 2 follows this explicit
            // selection rather than the resolved-active flag, so selecting Remote shows Remote's config
            // even though the daemon keeps a local engine resolved-active until Remote has an endpoint.
            let id2 = id.clone();
            right = right.child(button_sm_id(
                &format!("rt-select-{id}"),
                "Select",
                ButtonVariant::Secondary,
                cx.listener(move |this, _, _, cx| {
                    this.asr_engine_selected = Some(id2.clone());
                    this.asr_engine_expanded = false;
                    this.set_runtime(id2.clone(), cx);
                }),
            ));
        } else {
            // Not installed → download the engine pack (#81). Stays expanded; the row flips to the
            // in-flight bar, then to "Use" when the pack lands.
            let id2 = id.clone();
            right = right.child(download_button(
                &format!("rt-dl-{id}"),
                cx.listener(move |this, _, _, cx| this.install_runtime(id2.clone(), cx)),
            ));
        }

        let name_color = if unavailable { theme::TEXT_DISABLED } else { theme::TEXT_PRIMARY };
        // Stacked name + requirement. Each line gets `w_full` so its width is DEFINITE within the
        // (flex-resolved, min-width-0) column — gpui's text_ellipsis otherwise mis-measures a column
        // child and clips to a few characters. overflow_hidden makes the min-size 0 so the column can
        // still shrink when the window is narrow.
        let left = div()
            .flex()
            .flex_1()
            .min_w_0()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(theme::TS_BODY))
                    .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                    .text_color(rgb(name_color))
                    .child(rt.label.clone()),
            )
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(theme::TS_SMALL))
                    .line_height(px(16.0))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(rt.requires.clone()),
            )
            // "Coming soon" chip under the description for not-yet-supported engines (Remote, #80). The
            // trailing flex_1 spacer keeps the chip hugging the left edge instead of stretching the column.
            .when(coming_soon, |d| {
                d.child(
                    div().flex().mt(px(4.0)).child(
                        div()
                            .flex_none()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(theme::RADIUS_SM))
                            .bg(rgb(theme::CHIP_DISABLED))
                            .text_size(px(theme::TS_EYEBROW))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("Coming soon"),
                    )
                    .child(div().flex_1()),
                )
            });

        picker_row(rt.active).child(left).child(right).into_any_element()
    }

    /// The remote-runtime endpoint card (read-only mirror — endpoint editing lives on the remote
    /// daemon). Shown when the active runtime is `remote`, in place of a local model list.
    fn remote_config_card(&self) -> gpui::AnyElement {
        card(
            div()
                .flex()
                .flex_col()
                .gap(px(13.0))
                .child(
                    self.field_row("Endpoint").child(
                        div()
                            .flex_1()
                            .px(px(theme::SP_3))
                            .py(px(theme::SP_2))
                            .rounded(px(theme::RADIUS_MD))
                            .border_1()
                            .border_color(rgb(theme::BORDER))
                            .bg(rgb(theme::BG))
                            .text_size(px(theme::TS_BODY))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("configured on the remote daemon"),
                    ),
                )
                .child(
                    self.field_row("Model").child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(theme::SP_3))
                            .py(px(theme::SP_2))
                            .rounded(px(theme::RADIUS_MD))
                            .border_1()
                            .border_color(rgb(theme::BORDER))
                            .bg(rgb(theme::BG))
                            .text_size(px(theme::TS_BODY))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .child("server default")
                            .child(icon("chevron-down", 15.0, theme::TEXT_MUTED)),
                    ),
                )
                .child(
                    div()
                        .text_size(px(theme::TS_SMALL))
                        .line_height(px(18.0))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child("Models are served by the remote endpoint — no local download required."),
                ),
        )
        .into_any_element()
    }

    /// A dashed note shown when the active runtime needs hardware this machine lacks (e.g. CUDA on
    /// a Mac). No install button — there's nothing here to download.
    fn cuda_unavailable_box(&self) -> gpui::AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .p(px(theme::SP_4))
            .rounded(px(theme::RADIUS_LG))
            .border_1()
            .border_dashed()
            .border_color(rgb(theme::BORDER))
            .bg(rgb(theme::BG))
            .child(div().flex_none().size(px(8.0)).rounded_full().bg(rgb(theme::WARNING)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(theme::TS_BODY))
                            .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .child("This engine needs hardware this Mac doesn't have"),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TS_SMALL))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("No NVIDIA GPU detected. Switch to a local Metal/CPU engine or a remote endpoint."),
                    ),
            )
            .into_any_element()
    }

    /// The model card (Step 2): a "Pick a model" header (scoped to the active engine) + a row per
    /// catalog model. Honest about the engine being unloadable — no faked list in that case.
    fn model_picker_card(
        &mut self,
        asr_progress: &std::collections::HashMap<String, f32>,
        asr_dl: &std::collections::HashMap<String, (u64, u64, f64)>,
        asr_failed: &std::collections::HashMap<String, String>,
        rt_label: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let header = div()
            .flex()
            .items_center()
            .gap(px(theme::SP_2))
            .child(step_badge(2))
            .child(
                div()
                    .text_size(px(theme::TS_BODY))
                    .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child("Pick a model"),
            )
            .child(
                div()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(format!("for {rt_label}")),
            );

        let mut body = div().flex().flex_col().child(header);

        if !self.asr.backend_available {
            // The active local engine can't load in the running daemon — say so plainly rather than
            // present a model list that can't be used.
            return card(body.child(
                div()
                    .mt(px(10.0))
                    .text_size(px(theme::TS_SMALL))
                    .line_height(px(18.0))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(
                        "This engine isn't available in the running daemon — capture still records; \
                         transcription resumes once it loads.",
                    ),
            ))
            .into_any_element();
        }

        body = body.child(
            div()
                .mt(px(4.0))
                .mb(px(14.0))
                .text_size(px(theme::TS_SMALL))
                .line_height(px(18.0))
                .text_color(rgb(theme::TEXT_MUTED))
                .child(
                    "Bigger models are more accurate, but slower and larger to download. Downloads run \
                     in the background — this happens once.",
                ),
        );

        let models = self.asr.models.clone();
        let mut rows = div().flex().flex_col().gap(px(4.0));
        for m in &models {
            rows = rows.child(self.model_row(m, asr_progress, asr_dl, asr_failed, cx));
        }
        card(body.child(rows)).into_any_element()
    }

    /// One model row. In flight it becomes a vertical block — name + "downloading" on top, a
    /// full-width bar, then `656 MB / 1.6 GB · 41%` (left) and `~2 min left · 9.4 MB/s` (right).
    /// Otherwise it's a horizontal row: name · size on the left; the state control on the right
    /// (active+Remove · downloaded+Use+Remove · Download · failed+Retry).
    fn model_row(
        &mut self,
        m: &crate::daemon::AsrModel,
        asr_progress: &std::collections::HashMap<String, f32>,
        asr_dl: &std::collections::HashMap<String, (u64, u64, f64)>,
        asr_failed: &std::collections::HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let repo = m.repo.clone();
        let prog = asr_progress.get(&repo).copied();
        let downloading = prog.is_some() || m.downloading;
        let failed = asr_failed.contains_key(&repo);

        // Left group (name · size): shrink + truncate so the right-hand controls never clip.
        let left = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_baseline()
            .gap(px(8.0))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(theme::TS_BODY))
                    .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child(m.name.clone()),
            )
            .child(div().flex_none().text_size(px(theme::TS_SMALL)).text_color(rgb(theme::TEXT_DISABLED)).child("·"))
            .child(
                div()
                    .flex_none()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(m.size_label.clone()),
            );

        if downloading {
            // Prefer real byte counts/speed from the SSE; fall back to fraction × parsed size.
            let frac = prog.unwrap_or(0.0).clamp(0.0, 1.0);
            let (mut done, mut total, speed) = asr_dl.get(&repo).copied().unwrap_or((0, 0, 0.0));
            if total == 0 {
                total = parse_size_to_bytes(&m.size_label).unwrap_or(0);
            }
            if done == 0 && total > 0 {
                done = (frac as f64 * total as f64) as u64;
            }
            let detail_left = if total > 0 {
                format!("{} / {} · {:.0}%", human_size(done), human_size(total), frac * 100.0)
            } else {
                format!("{:.0}%", frac * 100.0)
            };
            let mut detail_right = String::new();
            if speed > 1.0 {
                if total > done {
                    detail_right.push_str(&human_eta((total - done) as f64 / speed));
                    detail_right.push_str(" · ");
                }
                detail_right.push_str(&human_rate(speed));
            }

            let top = div()
                .flex()
                .items_center()
                .gap(px(theme::SP_3))
                .child(left)
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(theme::TS_SMALL))
                        .text_color(rgb(theme::ACCENT_TEXT))
                        .child(icon("download", 13.0, theme::ACCENT_TEXT))
                        .child("downloading"),
                );
            let detail = div()
                .flex()
                .items_center()
                .justify_between()
                .mt(px(7.0))
                .child(
                    div()
                        .text_size(px(theme::TS_SMALL))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child(detail_left),
                )
                .when(!detail_right.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(theme::TS_SMALL))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(detail_right),
                    )
                });
            return div()
                .flex()
                .flex_col()
                .py(px(8.0))
                .px(px(12.0))
                .child(top)
                .child(div().mt(px(8.0)).child(progress_bar(frac, false)))
                .child(detail)
                .into_any_element();
        }

        let mut right = div().flex().flex_none().items_center().gap(px(theme::SP_2));
        if failed {
            // Recoverable: surface the failure + a Retry (routes back through download_model).
            let r = repo.clone();
            right = right
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(theme::TS_SMALL))
                        .text_color(rgb(theme::ERROR))
                        .child(icon("x", 12.0, theme::ERROR))
                        .child("download failed"),
                )
                .child(button_sm_id(
                    &format!("retry-{repo}"),
                    "Retry",
                    ButtonVariant::Secondary,
                    cx.listener(move |this, _, _, cx| this.download_model(r.clone(), cx)),
                ));
        } else if m.active {
            right = right.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::SUCCESS))
                    .child(div().flex_none().size(px(6.0)).rounded_full().bg(rgb(theme::SUCCESS)))
                    .child("active"),
            );
            if m.downloaded {
                let r = repo.clone();
                right = right.child(button_sm_id(
                    &format!("rm-{repo}"),
                    "Remove",
                    ButtonVariant::Destructive,
                    cx.listener(move |this, _, _, cx| this.delete_model(r.clone(), cx)),
                ));
            } else {
                // The configured-active model isn't downloaded yet (the fresh-install default). It can't
                // transcribe until fetched, so still offer Download — otherwise the row would be a
                // dead end ("active" with no action).
                let r = repo.clone();
                right = right.child(download_button(
                    &format!("dl-{repo}"),
                    cx.listener(move |this, _, _, cx| this.download_model(r.clone(), cx)),
                ));
            }
        } else if m.downloaded {
            let (r_use, r_rm) = (repo.clone(), repo.clone());
            right = right
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(theme::TS_SMALL))
                        .text_color(rgb(theme::SUCCESS))
                        .child(icon("check", 13.0, theme::SUCCESS))
                        .child("downloaded"),
                )
                .child(button_sm_id(
                    &format!("use-{repo}"),
                    "Use",
                    ButtonVariant::Secondary,
                    cx.listener(move |this, _, _, cx| this.set_active_model(r_use.clone(), cx)),
                ))
                .child(button_sm_id(
                    &format!("rm-{repo}"),
                    "Remove",
                    ButtonVariant::Destructive,
                    cx.listener(move |this, _, _, cx| this.delete_model(r_rm.clone(), cx)),
                ));
        } else {
            let r = repo.clone();
            right = right.child(download_button(
                &format!("dl-{repo}"),
                cx.listener(move |this, _, _, cx| this.download_model(r.clone(), cx)),
            ));
        }

        picker_row(m.active).child(left).child(right).into_any_element()
    }

    // ── Section: Index endpoint ──────────────────────────────────────────────
    /// One card: eyebrow + status pill, then Provider / Host / Port / Model / Frames / Preset.
    fn section_index(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let index_host_focused = self.index_host_focus.is_focused(window);
        let index_port_focused = self.index_port_focus.is_focused(window);
        let index_key_focused = self.index_key_focus.is_focused(window);

        // Multimodal index endpoint (#52/#53): structured provider + host:port + key, and a
        // model dropdown. Indexing is OFF until set AND reachable (the pill reflects status).
        let status_pill_el = if self.index_status.available {
            status_pill("reachable", theme::SUCCESS, theme::SUCCESS_SUBTLE)
        } else if self.index_status.configured {
            status_pill("unreachable", theme::ERROR, theme::ERROR_SUBTLE)
        } else {
            status_pill("not set", theme::TEXT_MUTED, theme::CHIP_IDLE)
        };
        let is_base = self.index_is_base_url();

        let mut rows = div()
            .flex()
            .flex_col()
            .gap(px(13.0))
            // Provider chips: selecting prefills the port + re-fetches models.
            .child(
                self.field_row("Provider")
                    .flex_wrap()
                    .children(INDEX_PROVIDERS.iter().map(|(id, plabel, _, _, _)| {
                        let pid = id.to_string();
                        chip(
                            &format!("idx-prov-{id}"),
                            plabel,
                            self.index_provider == *id,
                            cx.listener(move |this, _, _, cx| this.set_index_provider(&pid, cx)),
                        )
                    })),
            )
            // Host (or "Base URL" for the custom provider) + Check.
            .child(
                self.field_row(if is_base { "Base URL" } else { "Host" })
                    .child(self.index_input(
                        "index-host-input",
                        &self.index_host_focus,
                        IndexField::Host,
                        index_host_focused,
                        &self.index_host,
                        if is_base { "http://1.2.3.4:8000/v1  (Enter to check)" } else { "192.168.31.217  (Enter to check)" },
                        true,
                        cx,
                    ))
                    .child(button("Check", ButtonVariant::Secondary, cx.listener(|this, _, _, cx| this.probe_index_status(cx)))),
            );

        // Port (host:port providers only — custom hides it).
        if !is_base {
            rows = rows.child(
                self.field_row("Port").child(self.index_input(
                    "index-port-input",
                    &self.index_port_focus,
                    IndexField::Port,
                    index_port_focused,
                    &self.index_port,
                    "1234",
                    false,
                    cx,
                )),
            );
        }
        // API key (openai only).
        if self.index_needs_key() {
            rows = rows.child(
                self.field_row("API key").child(self.index_input(
                    "index-key-input",
                    &self.index_key_focus,
                    IndexField::Key,
                    index_key_focused,
                    &self.index_key,
                    "sk-…  (Enter to check)",
                    true,
                    cx,
                )),
            );
        }
        // Model dropdown (#53) + Refresh — restyled in indexing.rs to match `.fld`.
        rows = rows
            .child(self.index_model_field(cx))
            .child(
                // Leaf sampling rate: caption every round(1/rate)-th frame. Coarser =
                // far fewer vision calls (a long session has thousands of frames).
                self.field_row("Frames").children([1.0f64, 0.5, 0.25, 0.1, 0.05].into_iter().map(|r| {
                    let label = if r >= 1.0 {
                        "all".to_string()
                    } else {
                        format!("1/{}", (1.0 / r).round() as i32)
                    };
                    chip(
                        &format!("idx-rate-{r}"),
                        &label,
                        (self.index_sample_rate - r).abs() < 1e-3,
                        cx.listener(move |this, _, _, cx| {
                            this.index_sample_rate = r;
                            this.save_settings();
                            cx.notify();
                        }),
                    )
                })),
            )
            .child(
                // Prompt preset: what's right for a meeting is wrong for a lecture.
                self.field_row("Preset").children(
                    [("auto", "Auto"), ("meeting", "Meeting"), ("lecture", "Lecture"), ("general", "General")]
                        .into_iter()
                        .map(|(key, label)| {
                            chip(
                                &format!("idx-preset-{key}"),
                                label,
                                self.index_preset == key,
                                cx.listener(move |this, _, _, cx| {
                                    this.index_preset = key.to_string();
                                    this.save_settings();
                                    cx.notify();
                                }),
                            )
                        }),
                ),
            );

        let header = div()
            .flex()
            .items_center()
            .gap(px(theme::SP_2) + px(2.0))
            .mb(px(theme::SP_4))
            .child(eyebrow("Endpoint"))
            .child(status_pill_el);

        card(div().flex().flex_col().child(header).child(rows)).into_any_element()
    }

    /// A `.inp`-styled, focus-tracked index text field (host / port / key). Keeps the
    /// per-field focus handle + the shared `on_index_field_key` handler.
    #[allow(clippy::too_many_arguments)]
    fn index_input(
        &self,
        id: &'static str,
        focus: &gpui::FocusHandle,
        field: IndexField,
        focused: bool,
        value: &str,
        placeholder: &str,
        grow: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let value = value.to_string();
        let placeholder = placeholder.to_string();
        let mut el = div()
            .id(SharedString::from(id))
            .track_focus(focus)
            .key_context(id)
            .on_key_down(cx.listener(move |this, ev, _w, cx| this.on_index_field_key(field, ev, cx)))
            .px(px(theme::SP_3))
            .py(px(theme::SP_2))
            .rounded(px(theme::RADIUS_MD))
            .border_1()
            .border_color(if focused { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
            .bg(rgb(theme::BG))
            .text_size(px(theme::TS_BODY))
            .text_color(if value.is_empty() { rgb(theme::TEXT_MUTED) } else { rgb(theme::TEXT_PRIMARY) })
            .cursor_pointer()
            .child(if value.is_empty() {
                placeholder
            } else if focused {
                format!("{value}▏")
            } else {
                value
            })
            .on_click(cx.listener({
                let focus = focus.clone();
                move |_this, _, window, cx| {
                    window.focus(&focus);
                    cx.notify();
                }
            }));
        if grow {
            el = el.flex_1();
        } else {
            el = el.w(px(140.0));
        }
        el
    }

    // ── Section: Skills ──────────────────────────────────────────────────────
    /// One card: a bordered row per coding agent — clipboard icon + name/desc + action.
    fn section_skills(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = skill::AGENTS.iter().enumerate().map(|(ix, a)| {
            let status = self.skill_status.get(ix).copied();
            let installed = matches!(status, Some(skill::SkillStatus::UpToDate) | Some(skill::SkillStatus::UpdateAvailable));
            let desc = match status {
                Some(skill::SkillStatus::UpToDate) => "Capture skill · installed",
                Some(skill::SkillStatus::UpdateAvailable) => "Capture skill · update available",
                _ => "Capture skill · not installed",
            };

            let mut right = div().flex().items_center().gap(px(theme::SP_2));
            match status {
                Some(skill::SkillStatus::UpToDate) => {
                    right = right.child(status_pill("installed", theme::SUCCESS, theme::SUCCESS_SUBTLE));
                }
                Some(skill::SkillStatus::UpdateAvailable) => {
                    right = right
                        .child(status_pill("update available", theme::WARNING, theme::WARNING_SUBTLE))
                        .child(button_id(
                            &format!("skill-update-{ix}"),
                            "Update",
                            ButtonVariant::Primary,
                            cx.listener(move |this, _, _, cx| this.install_skill(ix, cx)),
                        ));
                }
                _ => {
                    right = right.child(button_id(
                        &format!("skill-install-{ix}"),
                        "Install",
                        ButtonVariant::Secondary,
                        cx.listener(move |this, _, _, cx| this.install_skill(ix, cx)),
                    ));
                }
            }

            div()
                .flex()
                .items_center()
                .gap(px(theme::SP_3))
                .p(px(theme::SP_3))
                .rounded(px(theme::RADIUS_MD))
                .border_1()
                .border_color(rgb(theme::ELEVATED))
                .bg(rgb(theme::BG))
                .child(icon("clipboard", 16.0, if installed { theme::ACCENT_TEXT } else { theme::TEXT_MUTED }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(theme::TS_BODY))
                                .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                                .text_color(rgb(theme::TEXT_PRIMARY))
                                .child(a.label),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TS_SMALL))
                                .text_color(rgb(theme::TEXT_MUTED))
                                .child(desc),
                        ),
                )
                .child(div().flex_1())
                .child(right)
        });

        // The MCP command (#78): the bundled `capture-mcp` path for an MCP client's `.mcp.json` —
        // app users get a working `command` with no build. Copy it to the clipboard.
        let mcp_path = crate::daemon::bundled_mcp();
        let has_mcp = mcp_path.is_some();
        let mcp_display = mcp_path
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "capture-mcp not found next to the app".to_string());
        let mut mcp_row = div()
            .flex()
            .items_center()
            .gap(px(theme::SP_3))
            .p(px(theme::SP_3))
            .rounded(px(theme::RADIUS_MD))
            .border_1()
            .border_color(rgb(theme::ELEVATED))
            .bg(rgb(theme::BG))
            .child(icon("clipboard", 16.0, if has_mcp { theme::ACCENT_TEXT } else { theme::TEXT_MUTED }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(theme::TS_BODY))
                            .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .child("MCP command (.mcp.json)"),
                    )
                    .child(
                        // w_full so the path's width is DEFINITE in the flex column — without it
                        // gpui's text_ellipsis mis-measures and clips to a few chars ("/Ap").
                        div()
                            .w_full()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(theme::TS_SMALL))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(mcp_display),
                    ),
            );
        if has_mcp {
            mcp_row = mcp_row.child(button_id(
                "mcp-copy",
                "Copy",
                ButtonVariant::Secondary,
                cx.listener(|this, _, _, cx| this.copy_mcp_command(cx)),
            ));
        }

        card(div().flex().flex_col().gap(px(theme::SP_2)).child(mcp_row).children(rows)).into_any_element()
    }

    // ── Section: Permissions ─────────────────────────────────────────────────
    /// One card: a perm row per permission, then a full-width Restart daemon button.
    fn section_permissions(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Permissions (macOS): Screen Recording + Microphone status, Grant (prompt),
        // Settings (grant/revoke), Restart daemon (apply a new Screen Recording grant
        // without quitting — the agent respawns it).
        let sr = self.perms.screen_recording.clone();
        let mic = self.perms.microphone.clone();
        let mut rows = div().flex().flex_col().gap(px(theme::SP_3));
        rows = rows
            .child(self.perm_row(
                "Screen Recording",
                &sr,
                "screenshots + window titles",
                "screen_recording",
                "Privacy_ScreenCapture",
                true, // promptable here (CoreGraphics FFI)
                cx,
            ))
            .child(self.perm_row(
                "Microphone",
                &mic,
                "mic-fallback audio",
                "microphone",
                "Privacy_Microphone",
                true, // promptable via the bundled agent one-shot (shared Team ID)
                cx,
            ));

        let restart = div()
            .mt(px(theme::SP_4))
            .id("perm-restart")
            .flex()
            .items_center()
            .justify_center()
            .gap(px(7.0))
            .w_full()
            .h(px(32.0))
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .bg(rgb(theme::ELEVATED))
            .text_size(px(theme::TS_BODY))
            .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
            .text_color(rgb(theme::TEXT_PRIMARY))
            .cursor_pointer()
            .hover(|s| s.border_color(rgb(theme::BORDER_STRONG)))
            .child(icon("refresh", 15.0, theme::TEXT_PRIMARY))
            .child("Restart daemon")
            .on_click(cx.listener(|this, _, _, cx| this.restart_daemon(cx)));

        card(div().flex().flex_col().child(rows).child(restart)).into_any_element()
    }

    // ── Section: App & updates ───────────────────────────────────────────────
    /// One card: Version + Update (progress while downloading, else up-to-date / available).
    fn section_updates(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        // App update (#48): offer a newer GitHub release; install only after confirm.
        let mut update_row = self.field_row("Update");
        if self.update_staged() {
            // The bundle was already replaced (the daemon is on the new version) but this GUI/agent
            // still runs the old binary — a clean restart finishes the update. We can't kill the agent
            // from here (the updater hit the same wall), so the button asks the agent to restart itself.
            let dv = self.health.as_ref().map(|h| h.version.clone()).unwrap_or_default();
            update_row = update_row
                .child(
                    div()
                        .text_size(px(theme::TS_BODY))
                        .text_color(rgb(theme::WARNING))
                        .child(format!("v{dv} installed — restart to finish")),
                )
                .child(div().flex_1())
                .child(button(
                    "Restart",
                    ButtonVariant::Primary,
                    cx.listener(|this, _, _, cx| this.request_restart(cx)),
                ));
        } else {
        match (&self.update_info, self.updating) {
            // Download finished (update_progress cleared) but still `updating`: the detached updater is
            // replacing the bundle and will relaunch the whole app. Show this, not a 0% bar (#48).
            (_, true) if self.update_progress.is_none() => {
                update_row = update_row
                    .child(
                        div()
                            .text_size(px(theme::TS_BODY))
                            .text_color(rgb(theme::TEXT_SECONDARY))
                            .child("installing — the app will restart…"),
                    )
                    .child(div().flex_1())
                    .child(div().w(px(200.0)).child(progress_bar(1.0, false)));
            }
            (_, true) => {
                // The DMG/exe is ~175 MB, so show a real progress bar (#48). `t == 0` means the
                // server didn't send Content-Length yet → indeterminate (just downloaded MB).
                let (d, t) = self.update_progress.unwrap_or((0, 0));
                let dmb = d as f64 / 1_048_576.0;
                if t > 0 {
                    let frac = (d as f32 / t as f32).clamp(0.0, 1.0);
                    let tmb = t as f64 / 1_048_576.0;
                    update_row = update_row
                        .child(
                            div()
                                .text_size(px(theme::TS_BODY))
                                .text_color(rgb(theme::TEXT_SECONDARY))
                                .child(format!("downloading v{}", self.update_info.as_ref().map(|i| i.version.clone()).unwrap_or_default())),
                        )
                        .child(div().flex_1())
                        .child(div().w(px(200.0)).child(progress_bar(frac, false)))
                        .child(
                            div()
                                .text_size(px(theme::TS_SMALL))
                                .text_color(rgb(theme::TEXT_SECONDARY))
                                .child(format!("{}% · {:.0} / {:.0} MB", (frac * 100.0) as i32, dmb, tmb)),
                        );
                } else {
                    update_row = update_row
                        .child(
                            div()
                                .text_size(px(theme::TS_BODY))
                                .text_color(rgb(theme::TEXT_SECONDARY))
                                .child(format!("downloading update… ({:.0} MB)", dmb)),
                        )
                        .child(div().flex_1())
                        .child(div().w(px(200.0)).child(progress_bar(0.0, false)));
                }
            }
            (Some(info), false) => {
                let info2 = info.clone();
                update_row = update_row
                    .child(
                        div()
                            .text_size(px(theme::TS_BODY))
                            .text_color(rgb(theme::WARNING))
                            .child(format!("v{} available", info.version)),
                    )
                    .child(div().flex_1())
                    .child(button(
                        "Update…",
                        ButtonVariant::Primary,
                        cx.listener(move |this, _, _, cx| {
                            this.confirm = Some(ConfirmKind::Update(info2.clone()));
                            cx.notify();
                        }),
                    ));
            }
            (None, false) => {
                update_row = update_row.child(
                    div()
                        .text_size(px(theme::TS_BODY))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child("up to date"),
                );
            }
        }
        }

        card(
            div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    self.field_row("Version").child(
                        div()
                            .text_size(px(theme::TS_BODY))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .child(format!("v{}", update::CURRENT)),
                    ),
                )
                .child(update_row),
        )
        .into_any_element()
    }

    /// The transcription-language control (#45): an editable ISO-code field + the active
    /// value. Shown in Settings and the playback pane (change it on the fly during a live
    /// capture; the next chunk uses it). `focused` is the field's focus state.
    pub(crate) fn language_field(&self, focused: bool, open_up: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.asr.language.clone().unwrap_or_default();
        let active_name = LANGUAGES
            .iter()
            .find(|(c, _)| *c == active)
            .map(|(_, n)| *n)
            .unwrap_or(if active.is_empty() { "Auto-detect" } else { active.as_str() });
        // The field shows the active language when idle, or the live filter while typing.
        // The trailing caret is a real `chevron-down` icon (§4), not text.
        let field_text = if self.lang_dropdown_open && !self.asr_language.is_empty() {
            format!("{}▏", self.asr_language)
        } else if self.lang_dropdown_open && focused {
            "type to search…".to_string()
        } else if active.is_empty() {
            "Auto-detect".to_string()
        } else {
            format!("{active} · {active_name}")
        };
        let dim = self.lang_dropdown_open && self.asr_language.is_empty();
        let open = self.lang_dropdown_open;

        let mut col = div().flex().flex_col().gap_1().relative().child(
            self.field_row("Language").child(
                div()
                    .id("asr-lang-input")
                    .track_focus(&self.asr_language_focus)
                    .key_context("asr-lang")
                    .on_key_down(cx.listener(Self::on_asr_language_key))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SP_2))
                    .w(px(240.0))
                    .py(px(theme::SP_2))
                    .px(px(theme::SP_3))
                    .rounded(px(theme::RADIUS_MD))
                    .text_size(px(theme::TS_BODY))
                    .border_1()
                    .border_color(if focused || open { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
                    .bg(rgb(theme::BG))
                    .cursor_pointer()
                    .when(!focused && !open, |d| d.hover(|s| s.border_color(rgb(theme::BORDER_STRONG))))
                    .text_color(if dim { rgb(theme::TEXT_MUTED) } else { rgb(theme::TEXT_PRIMARY) })
                    .child(field_text)
                    .child(icon("chevron-down", 15.0, theme::TEXT_MUTED))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.lang_dropdown_open = !this.lang_dropdown_open;
                        this.asr_language.clear();
                        if this.lang_dropdown_open {
                            window.focus(&this.asr_language_focus);
                        }
                        cx.notify();
                    })),
            ),
        );

        if self.lang_dropdown_open {
            let filter = self.asr_language.trim().to_lowercase();
            // §4 dropdown menu surface, floated as an absolute popover below the field
            // (top:100% of the relative col, left = label width) so it overlays the chunk
            // row below instead of pushing it down.
            let mut list = div()
                .absolute()
                // Open downward by default; upward (anchored to the field's top) when the field sits
                // low on screen (the playback panel) so the menu isn't clipped below the fold.
                .when(open_up, |d| d.bottom(relative(1.0)))
                .when(!open_up, |d| d.top(relative(1.0)))
                .left(px(132.0))
                .flex()
                .flex_col()
                .w(px(240.0))
                .p(px(theme::SP_1))
                .rounded(px(theme::RADIUS_MD))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .bg(rgb(theme::ELEVATED))
                .occlude(); // clicks on the menu select; they must not fall through to the dismiss backdrop
            let matches = LANGUAGES.iter().filter(|(c, n)| {
                filter.is_empty() || c.contains(&filter) || n.to_lowercase().contains(&filter)
            });
            // Cap the visible rows — the search filter narrows it, so no scroll is needed.
            let total = LANGUAGES
                .iter()
                .filter(|(c, n)| filter.is_empty() || c.contains(&filter) || n.to_lowercase().contains(&filter))
                .count();
            let mut any = false;
            for (code, name) in matches.take(12) {
                any = true;
                let code_s = code.to_string();
                let is_active = *code == active;
                // §4 menu row: pad 7×10, radius 4, hover GHOST_HOVER, selected
                // ACCENT_SUBTLE/ACCENT_TEXT. Keeps the two-column code + name layout.
                list = list.child(
                    div()
                        .id(SharedString::from(format!("lang-row-{code}")))
                        .flex()
                        .gap_2()
                        .items_center()
                        .py(px(7.0))
                        .px(px(10.0))
                        .rounded(px(4.0))
                        .text_size(px(theme::TS_BODY))
                        .cursor_pointer()
                        .when(is_active, |s| s.bg(rgb(theme::ACCENT_SUBTLE)).text_color(rgb(theme::ACCENT_TEXT)))
                        .when(!is_active, |s| s.text_color(rgb(theme::TEXT_SECONDARY)).hover(|h| h.bg(rgba(theme::GHOST_HOVER))))
                        .child(div().min_w(px(28.0)).text_color(rgb(theme::ACCENT_TEXT)).child(if code.is_empty() { "—" } else { *code }))
                        .child(div().child(*name))
                        .on_click(cx.listener(move |this, _, _, cx| this.apply_language_code(code_s.clone(), cx))),
                );
            }
            if !any {
                list = list.child(div().py(px(7.0)).px(px(10.0)).text_color(rgb(theme::TEXT_MUTED)).child("no match"));
            } else if total > 12 {
                list = list.child(
                    div().py(px(7.0)).px(px(10.0)).text_xs().text_color(rgb(theme::TEXT_MUTED)).child(format!("+{} more — keep typing", total - 12)),
                );
            }
            col = col.child(deferred(list));
        }
        col
    }

    /// The transcription chunk-length chips (#45). Larger windows avoid Whisper's
    /// short-chunk hallucination; smaller = lower latency.
    pub(crate) fn chunk_chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cur = self.asr.chunk_seconds;
        self.field_row("Chunk").children([8.0f64, 15.0, 30.0, 60.0].into_iter().map(|s| {
            chip(
                &format!("chunk-{s}"),
                &format!("{s:.0}s"),
                (cur - s).abs() < 0.5,
                cx.listener(move |this, _, _, cx| this.set_asr_chunk(s, cx)),
            )
        }))
    }

    /// A `.row`: a 118px muted `.lbl` (via `field_label`) followed by a flex row, gap 14,
    /// items-center — the caller `.child(...)`s the controls. Matches the design `.row`/`.lbl`.
    fn field_row(&self, label: &'static str) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .child(self.field_label(label))
    }

    /// A consistent left-hand field label (design `.lbl`): 118px fixed, TEXT_MUTED body.
    fn field_label(&self, text: &'static str) -> impl IntoElement {
        div()
            .w(px(118.0))
            .flex_none()
            .text_size(px(theme::TS_BODY))
            .text_color(rgb(theme::TEXT_MUTED))
            .child(text)
    }

    /// One permission row (§5): an icon + title + the "why" hint on the left; a status
    /// pill (+ Grant / Settings buttons when not granted) on the right.
    /// `can_prompt` is true only for Screen Recording (CoreGraphics FFI); Microphone
    /// has no Grant button — it's granted via Settings / auto-prompted by ffmpeg.
    pub(crate) fn perm_row(
        &self,
        title: &'static str,
        status: &str,
        why: &'static str,
        kind: &'static str,
        pane: &'static str,
        can_prompt: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Name column: 160px fixed (design), TEXT_PRIMARY body.
        let name = div()
            .w(px(160.0))
            .flex_none()
            .text_size(px(theme::TS_BODY))
            .text_color(rgb(theme::TEXT_PRIMARY))
            .child(title);
        let _ = why;

        // Status: a dot + label. granted→SUCCESS, not-granted→WARNING, undetermined→muted.
        let (dot, label, prompt_btn): (u32, &str, bool) = match status {
            "granted" => (theme::SUCCESS, "granted", false),
            "undetermined" => (theme::TEXT_MUTED, "not requested", false),
            _ => (theme::WARNING, "not granted", true),
        };
        let status_el = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(theme::TS_SMALL))
            .text_color(rgb(dot))
            .child(div().flex_none().size(px(6.0)).rounded_full().bg(rgb(dot)))
            .child(label);

        let mut right = div().flex().items_center().gap(px(theme::SP_2));
        if prompt_btn && can_prompt {
            right = right.child(button_id(
                &format!("grant-{kind}"),
                "Grant",
                ButtonVariant::Primary,
                cx.listener(move |this, _, _, cx| this.request_permission(kind, cx)),
            ));
        }
        right = right.child(button_id(
            &format!("perm-settings-{kind}"),
            "Settings",
            ButtonVariant::Secondary,
            cx.listener(move |this, _, _, cx| this.open_privacy_settings(pane, cx)),
        ));

        div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .child(name)
            .child(status_el)
            .child(div().flex_1())
            .child(right)
    }
}

// ── Voice-picker free helpers (#83) ──────────────────────────────────────────

/// A small numbered step badge (the indigo circle in the step banner / model header).
fn step_badge(n: u32) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .size(px(20.0))
        .rounded_full()
        .bg(rgb(theme::ACCENT))
        .text_size(px(theme::TS_SMALL))
        .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
        .text_color(rgb(theme::ON_ACCENT))
        .child(n.to_string())
}

/// A primary "Download" button with the leading download glyph, sized for a dense picker row.
/// (The kit's `button_sm` has no icon slot, so this composes one directly.)
fn download_button(
    id: &str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .h(px(28.0))
        .px(px(11.0))
        .rounded(px(theme::RADIUS_SM))
        .bg(rgb(theme::ACCENT))
        .text_color(rgb(theme::ON_ACCENT))
        .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
        .text_size(px(theme::TS_SMALL))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(theme::ACCENT_HOVER)))
        .child(icon("download", 13.0, theme::ON_ACCENT))
        .child("Download")
        .on_click(on_click)
}

/// The shell for a runtime/model picker row: padding 10×12, radius 6, with the §4 selected
/// treatment (ACCENT_SUBTLE fill + 1px ACCENT_BORDER + a 2px ACCENT left bar). Unselected rows are
/// transparent (the card bounds them); the 2px bar is always present so selection doesn't shift
/// content. The caller appends the left + right groups.
fn picker_row(selected: bool) -> gpui::Div {
    let left_bar = div()
        .absolute()
        .top_0()
        .left_0()
        .h_full()
        .w(px(2.0))
        .bg(if selected { rgb(theme::ACCENT) } else { rgba(theme::TRANSPARENT) });
    div()
        .relative()
        .flex()
        .items_center()
        .gap(px(theme::SP_3))
        .py(px(10.0))
        .px(px(12.0))
        .rounded(px(theme::RADIUS_MD))
        .border_1()
        .border_color(if selected { rgb(theme::ACCENT_BORDER) } else { rgba(theme::TRANSPARENT) })
        .bg(if selected { rgb(theme::ACCENT_SUBTLE) } else { rgba(theme::TRANSPARENT) })
        .child(left_bar)
}

/// Parse a human size label ("1.6 GB", "150 MB", "75 MB") to SI bytes — used to derive a download's
/// total when the SSE hasn't reported `total` yet. None if the label isn't recognized.
fn parse_size_to_bytes(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace();
    // Catalog labels read like "~1.6 GB" — drop a leading approximation marker before parsing.
    let num: f64 = it.next()?.trim_start_matches('~').parse().ok()?;
    let mul = match it.next()?.to_ascii_uppercase().as_str() {
        "GB" | "GIB" => 1e9,
        "MB" | "MIB" => 1e6,
        "KB" | "KIB" => 1e3,
        "B" => 1.0,
        _ => return None,
    };
    Some((num * mul) as u64)
}

/// Human-readable SI size: GB with one decimal, MB/KB with none (matches "656 MB / 1.6 GB").
fn human_size(n: u64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.1} GB", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.0} MB", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.0} KB", f / 1e3)
    } else {
        format!("{n} B")
    }
}

/// Human-readable transfer rate ("9.4 MB/s").
fn human_rate(bps: f64) -> String {
    if bps >= 1e6 {
        format!("{:.1} MB/s", bps / 1e6)
    } else if bps >= 1e3 {
        format!("{:.0} KB/s", bps / 1e3)
    } else {
        format!("{:.0} B/s", bps)
    }
}

/// Human-readable ETA ("~2 min left" / "~9 sec left").
fn human_eta(secs: f64) -> String {
    if secs >= 90.0 {
        format!("~{} min left", (secs / 60.0).round() as i64)
    } else {
        format!("~{} sec left", secs.round().max(1.0) as i64)
    }
}
