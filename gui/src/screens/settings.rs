//! The Settings screen: a two-pane frame (left nav + content) matching the LOCKED
//! design artboard (`design/unpacked/_template.html`, the SETTINGS block). The nav
//! switches `settings_section`; the content pane renders that section's card(s).
//! Markup/styling only — every listener, focus handle, and the section gating below
//! is preserved verbatim from the prior single-column implementation (#68/#71).

use gpui::{deferred, div, prelude::*, px, relative, rgb, rgba, Context, SharedString, Window};

use crate::app::CaptureApp;
use crate::components::{
    button, button_disabled, button_id, button_sm_id, card, chip, eyebrow, icon, progress_bar,
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
        let body = div()
            .flex()
            .size_full() // fill the window; the nav is fixed-width, the content pane scrolls
            .child(self.settings_nav(cx))
            .child(self.settings_content(window, cx));
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
                .child(self.language_field(asr_lang_focused, cx))
                .child(self.chunk_chips(cx)),
        )
        .into_any_element()
    }

    // ── Section: Voice recognition ───────────────────────────────────────────
    /// First a Runtime card (eyebrow + note + wrapping runtime chips), then — based on the
    /// ACTIVE runtime — a remote-config card, a CUDA-unavailable dashed box, or the Whisper
    /// models card.
    fn section_voice(&mut self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let (asr_progress, rt_install) = {
            let live = self.live.lock().unwrap();
            (live.asr_progress.clone(), live.runtime_install.clone())
        };
        let nvidia = self.runtimes.gpu.nvidia;

        // Classify the active runtime to pick the second card.
        let active_rt = self.runtimes.runtimes.iter().find(|r| r.active).cloned();
        let active_remote = active_rt.as_ref().map(|r| r.kind == "remote").unwrap_or(false);
        let active_needs_nvidia = active_rt
            .as_ref()
            .map(|r| {
                let req = r.requires.to_lowercase();
                r.kind != "remote" && (req.contains("nvidia") || req.contains("cuda"))
            })
            .unwrap_or(false);
        let cuda_unavailable = active_needs_nvidia && !nvidia;

        // The ACTUAL local engine on this Mac (Apple MLX) serves the models below, but it isn't
        // in the runtime registry (which only lists the pluggable faster-whisper/remote packs),
        // whose default "remote" otherwise looks active. Surface mlx explicitly so the runtime
        // isn't lying about where transcription runs. Proper unification of the two = #77.
        let mlx_active = self.asr.backend_available;
        let note = if mlx_active {
            "Transcription runs locally on Apple MLX (Metal), the Apple-silicon GPU engine. The runtimes below are alternative/cross-platform engines."
        } else if active_remote {
            "Transcription runs on the configured remote endpoint."
        } else if cuda_unavailable {
            "The active runtime needs an NVIDIA GPU. Switch to CPU or a remote endpoint."
        } else {
            "Choose where transcription runs. Local engines download models on demand; remote sends audio to an endpoint."
        };
        let mut chips = div().flex().gap(px(theme::SP_2)).flex_wrap();
        if mlx_active {
            chips = chips.child(chip("rt-mlx", "Apple MLX · Metal", true, cx.listener(|_, _, _, _| {})));
        }
        for rt in &self.runtimes.runtimes {
            let id = rt.id.clone();
            // Selecting a registry runtime = activating it (set_runtime). Don't show one as
            // active while mlx is actually serving (the registry's flag is disconnected — #77).
            chips = chips.child(chip(
                &format!("rt-{id}"),
                &rt.label,
                rt.active && !mlx_active,
                cx.listener(move |this, _, _, cx| this.set_runtime(id.clone(), cx)),
            ));
        }
        let runtime_card = card(
            div()
                .flex()
                .flex_col()
                .child(eyebrow("Runtime"))
                .child(
                    div()
                        .mt(px(theme::SP_2))
                        .mb(px(14.0))
                        .text_size(px(theme::TS_SMALL))
                        .line_height(px(18.0))
                        .text_color(rgb(theme::TEXT_MUTED))
                        .child(note),
                )
                .child(chips),
        )
        .into_any_element();

        let mut out = vec![runtime_card];

        if self.asr.backend_available {
            // A local engine with a model catalog is available (mlx on this daemon). Always show the
            // Whisper model manager so the downloaded/selectable models stay reachable — the runtime
            // registry's `active` flag (e.g. "remote") is NOT wired to the mlx backend in this daemon,
            // so gating the model list on it wrongly hid the user's local models.
            out.push(self.asr_models_card(&asr_progress, cx));
        } else if active_remote {
            // Remote-config card. No backend wiring exists for editing the remote endpoint
            // yet, so the fields are read-only (mirrors the frame; invents no calls).
            out.push(
                card(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(13.0))
                        .child(
                            self.field_row("Endpoint")
                                .child(
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
                .into_any_element(),
            );
        } else if cuda_unavailable {
            // CUDA-unavailable dashed box: amber dot + message + a disabled install button.
            out.push(
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
                                    .child("CUDA runtime not available"),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TS_SMALL))
                                    .text_color(rgb(theme::TEXT_MUTED))
                                    .child("No NVIDIA GPU detected on this machine. Use CPU or a remote endpoint."),
                            ),
                    )
                    .child(div().flex_1())
                    .child(button_disabled("Install runtime pack"))
                    .into_any_element(),
            );
        }

        // The runtime-install progress is surfaced inline per-runtime in the frame's intent;
        // we keep the SSE feed read (`rt_install`) consulted so an in-flight install shows
        // its progress on the active engine row when one exists.
        let _ = rt_install;
        out
    }

    /// The Whisper models card: a baseline title row + one row per model with its state.
    fn asr_models_card(
        &mut self,
        asr_progress: &std::collections::HashMap<String, f32>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let title_row = div()
            .flex()
            .items_baseline()
            .gap(px(theme::SP_2))
            .mb(px(14.0))
            .child(
                div()
                    .text_size(px(theme::TS_BODY))
                    .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child("Whisper models"),
            )
            .child(
                div()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(if self.asr.backend_available {
                        "downloaded on demand · ~/.cache/huggingface"
                    } else {
                        "runtime unavailable in this daemon — capture still works"
                    }),
            );

        let mut body = div().flex().flex_col().child(title_row);

        if self.asr.backend_available {
            let mut rows = div().flex().flex_col().gap(px(1.0));
            for m in &self.asr.models {
                let repo = m.repo.clone();
                let prog = asr_progress.get(&repo).copied();
                let downloading = prog.is_some() || m.downloading;

                let mut right = div().flex().flex_none().items_center().gap(px(theme::SP_2));
                if downloading {
                    let f = prog.unwrap_or(0.0).clamp(0.0, 1.0);
                    right = right
                        .child(div().w(px(140.0)).child(progress_bar(f, false)))
                        .child(
                            div()
                                .text_size(px(theme::TS_SMALL))
                                .text_color(rgb(theme::TEXT_MUTED))
                                .child(format!("{:.0}%", f * 100.0)),
                        );
                } else if m.active {
                    // Active model: a SUCCESS dot+"active" plus a Remove (it's downloaded).
                    right = right
                        .child(
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
                        let r = repo.clone();
                        right = right.child(button_sm_id(
                            &format!("dl-{repo}"),
                            "Download",
                            ButtonVariant::Primary,
                            cx.listener(move |this, _, _, cx| this.download_model(r.clone(), cx)),
                        ));
                    }
                } else if m.downloaded {
                    // Downloaded, not active: check+"downloaded" + Use + Remove.
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
                    // Available: a Download action.
                    let r = repo.clone();
                    right = right.child(button_sm_id(
                        &format!("dl-{repo}"),
                        "Download",
                        ButtonVariant::Primary,
                        cx.listener(move |this, _, _, cx| this.download_model(r.clone(), cx)),
                    ));
                }

                // Left group (name · size) is allowed to shrink + truncate so the action
                // buttons (flex_none) always stay on-screen when the window is narrow.
                let left = div()
                    .flex()
                    .flex_1() // GROW to fill + SHRINK-truncate when tight, so the action buttons
                    .min_w_0() // (flex_none, right) always stay on-screen instead of being pushed off
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(theme::TS_BODY))
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
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SP_3))
                        .py(px(9.0))
                        .px(px(10.0))
                        .rounded(px(theme::RADIUS_MD))
                        .when(m.active, |d| d.bg(rgb(theme::ACTIVE_ROW)))
                        .child(left)
                        .child(right),
                );
            }
            body = body.child(rows);
        }

        card(body).into_any_element()
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

        card(div().flex().flex_col().gap(px(theme::SP_2)).children(rows)).into_any_element()
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
        match (&self.update_info, self.updating) {
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
    pub(crate) fn language_field(&self, focused: bool, cx: &mut Context<Self>) -> impl IntoElement {
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
                .top(relative(1.0))
                .left(px(132.0))
                .flex()
                .flex_col()
                .w(px(240.0))
                .p(px(theme::SP_1))
                .rounded(px(theme::RADIUS_MD))
                .border_1()
                .border_color(rgb(theme::BORDER))
                .bg(rgb(theme::ELEVATED));
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
