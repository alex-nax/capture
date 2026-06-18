//! The Settings screen render branch + its field/row helper methods. Relocated verbatim
//! from `app.rs` `render()` (#68). ONE file for now (per #68; per-section split is later).
//! Returns the ordered list of settings children; the shell in `app.rs` appends them via
//! `.children(...)` exactly as before.

use gpui::{div, prelude::*, px, rgb, rgba, Context, SharedString, Window};

use crate::app::CaptureApp;
use crate::components::{
    button, button_disabled, chip, eyebrow, icon, list_row, progress_bar, status_pill,
    ButtonVariant,
};
use crate::skill;
use crate::state::{ConfirmKind, IndexField, SettingsSection, INDEX_PROVIDERS, LANGUAGES, RES_PRESETS};
use crate::theme;
use crate::update;

impl CaptureApp {
    /// Build the Settings screen's children, in render order: capture quality, app update,
    /// transcription, the index endpoint, skill installers, permissions, the voice-recognition
    /// runtime panel, and the Whisper model panel. Mirrors the prior `sett.then(...)` chain.
    pub(crate) fn render_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let index_host_focused = self.index_host_focus.is_focused(window);
        let index_port_focused = self.index_port_focus.is_focused(window);
        let index_key_focused = self.index_key_focus.is_focused(window);
        let asr_lang_focused = self.asr_language_focus.is_focused(window);

        // ── Voice recognition (§5): a runtime selector drives the model list ──────────
        // The user picks a runtime (CPU / Core ML / CUDA / remote), then manages the
        // models the ACTIVE runtime reports. Install / download progress are SSE-fed.
        let (asr_progress, rt_install) = {
            let live = self.live.lock().unwrap();
            (live.asr_progress.clone(), live.runtime_install.clone())
        };
        let nvidia = self.runtimes.gpu.nvidia;
        // Runtime selector: one selectable list_row per runtime. The active runtime gets
        // the accent treatment; the right side shows a status pill or an action button.
        let rt_rows: Vec<gpui::AnyElement> = self
            .runtimes
            .runtimes
            .iter()
            .map(|rt| {
                let id = rt.id.clone();
                let prog = rt_install.get(&id).copied();
                let is_remote = rt.kind == "remote";
                // CUDA-unavailable state: a runtime that needs an NVIDIA GPU on a box
                // without one. Detect from the requirement text (case-insensitive).
                let req_lower = rt.requires.to_lowercase();
                let needs_nvidia = req_lower.contains("nvidia") || req_lower.contains("cuda");
                let gpu_unavailable = !is_remote && needs_nvidia && !nvidia;

                // Right-hand state + action.
                let mut right = div().flex().items_center().gap(px(theme::SP_2));
                if rt.active {
                    right = right.child(status_pill("active", theme::SUCCESS, theme::SUCCESS_SUBTLE));
                } else if let Some(f) = prog {
                    let pct = (f * 100.0).clamp(0.0, 100.0);
                    right = right
                        .child(div().w(px(120.0)).child(progress_bar(f.clamp(0.0, 1.0), false)))
                        .child(
                            div()
                                .text_size(px(theme::TS_SMALL))
                                .text_color(rgb(theme::TEXT_MUTED))
                                .child(format!("{pct:.0}%")),
                        );
                } else if gpu_unavailable {
                    right = right
                        .child(status_pill("unavailable", theme::TEXT_MUTED, theme::CHIP_IDLE))
                        .child(button_disabled("Install"));
                } else if is_remote {
                    let i = id.clone();
                    right = right.child(button(
                        "Use",
                        ButtonVariant::Secondary,
                        cx.listener(move |this, _, _, cx| this.set_runtime(i.clone(), cx)),
                    ));
                } else if !rt.installed {
                    let i = id.clone();
                    right = right.child(button(
                        "Install",
                        ButtonVariant::Primary,
                        cx.listener(move |this, _, _, cx| this.install_runtime(i.clone(), cx)),
                    ));
                } else {
                    let i = id.clone();
                    right = right.child(button(
                        "Use",
                        ButtonVariant::Secondary,
                        cx.listener(move |this, _, _, cx| this.set_runtime(i.clone(), cx)),
                    ));
                }

                // Label + requirement line.
                let mut left = div().flex().flex_col().gap(px(2.0)).child(
                    div()
                        .text_size(px(theme::TS_BODY))
                        .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                        .text_color(rgb(theme::TEXT_PRIMARY))
                        .child(rt.label.clone()),
                );
                if !rt.requires.is_empty() {
                    left = left.child(
                        div()
                            .text_size(px(theme::TS_SMALL))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(rt.requires.clone()),
                    );
                }
                if gpu_unavailable {
                    left = left.child(
                        div()
                            .text_size(px(theme::TS_SMALL))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child("no NVIDIA GPU detected"),
                    );
                }
                let content = div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SP_3))
                    .child(left)
                    .child(right);

                // The whole row is selectable only when picking it is meaningful: an
                // installed local runtime or an available remote, and not already active /
                // installing / gpu-unavailable. Otherwise a no-op listener.
                let selectable = !rt.active
                    && prog.is_none()
                    && !gpu_unavailable
                    && (is_remote || rt.installed);
                let i = id.clone();
                if selectable {
                    list_row(
                        &format!("rt-{id}"),
                        rt.active,
                        cx.listener(move |this, _, _, cx| this.set_runtime(i.clone(), cx)),
                        content,
                    )
                    .into_any_element()
                } else {
                    list_row(
                        &format!("rt-{id}"),
                        rt.active,
                        cx.listener(|_, _, _, _| {}),
                        content,
                    )
                    .into_any_element()
                }
            })
            .collect();

        // Model manager: one row per model the active runtime reports. Each row carries
        // its own active tint (so it's a plain container, not a list_row).
        let model_rows: Vec<gpui::AnyElement> = self
            .asr
            .models
            .iter()
            .map(|m| {
                let repo = m.repo.clone();
                let prog = asr_progress.get(&repo).copied();
                let downloading = prog.is_some() || m.downloading;

                let mut right = div().flex().items_center().gap(px(theme::SP_2));
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
                } else if m.active && m.downloaded {
                    let r = repo.clone();
                    right = right
                        .child(status_pill("active", theme::SUCCESS, theme::SUCCESS_SUBTLE))
                        .child(button(
                            "Remove",
                            ButtonVariant::Destructive,
                            cx.listener(move |this, _, _, cx| this.delete_model(r.clone(), cx)),
                        ));
                } else if m.active {
                    let r = repo.clone();
                    right = right
                        .child(status_pill("needs download", theme::WARNING, theme::WARNING_SUBTLE))
                        .child(button(
                            "Download",
                            ButtonVariant::Primary,
                            cx.listener(move |this, _, _, cx| this.download_model(r.clone(), cx)),
                        ));
                } else if m.downloaded {
                    let (r_use, r_rm) = (repo.clone(), repo.clone());
                    right = right
                        .child(status_pill("downloaded", theme::SUCCESS, theme::SUCCESS_SUBTLE))
                        .child(button(
                            "Use",
                            ButtonVariant::Secondary,
                            cx.listener(move |this, _, _, cx| this.set_active_model(r_use.clone(), cx)),
                        ))
                        .child(button(
                            "Remove",
                            ButtonVariant::Destructive,
                            cx.listener(move |this, _, _, cx| this.delete_model(r_rm.clone(), cx)),
                        ));
                } else {
                    let r = repo.clone();
                    right = right.child(button(
                        "Download",
                        ButtonVariant::Primary,
                        cx.listener(move |this, _, _, cx| this.download_model(r.clone(), cx)),
                    ));
                }

                let mut name = div().flex().items_center().gap(px(theme::SP_2));
                if m.active {
                    name = name.child(
                        div().flex_none().size(px(8.0)).rounded_full().bg(rgb(theme::SUCCESS)),
                    );
                }
                name = name
                    .child(
                        div()
                            .text_size(px(theme::TS_BODY))
                            .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                            .text_color(rgb(theme::TEXT_PRIMARY))
                            .child(m.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TS_SMALL))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(format!("· {}", m.size_label)),
                    );

                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SP_3))
                    .py(px(10.0))
                    .px(px(12.0))
                    .rounded(px(theme::RADIUS_MD))
                    .border_1()
                    .border_color(rgb(theme::BORDER))
                    .when(m.active, |d| d.bg(rgb(theme::ACTIVE_ROW)))
                    .child(name)
                    .child(right)
                    .into_any_element()
            })
            .collect();

        let runtime_panel = div()
            .flex()
            .flex_col()
            .gap(px(theme::SP_2))
            .child(eyebrow("Runtime"))
            .children(rt_rows);

        let mut asr_panel = div().flex().flex_col().gap(px(theme::SP_2)).child(eyebrow("Models"));
        if self.asr.backend_available {
            asr_panel = asr_panel.children(model_rows);
        } else {
            asr_panel = asr_panel.child(
                div()
                    .text_size(px(theme::TS_BODY))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child("Runtime unavailable in this daemon — capture still works."),
            );
        }

        // Capture-quality settings (Settings screen): screenshot format + resolution
        // + jpeg quality, applied to new captures via shot_settings().
        let is_jpeg = self.shot_format == "jpeg";
        let mut quality_panel = div()
            .flex()
            .flex_col()
            .gap(px(theme::SP_3))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(self.field_label("Screenshots", 96.0))
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
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(self.field_label("Format", 96.0))
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
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(self.field_label("Resolution", 96.0))
                    .children(RES_PRESETS.iter().enumerate().map(|(i, p)| {
                        chip(&format!("res-{i}"), p.0, self.shot_res_ix == i, cx.listener(move |this, _, _, cx| {
                            this.shot_res_ix = i;
                            this.save_settings();
                            cx.notify();
                        }))
                    })),
            );
        if is_jpeg {
            quality_panel = quality_panel.child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(self.field_label("JPEG quality", 96.0))
                    .children([60u32, 80, 95].into_iter().map(|q| {
                        chip(&format!("q-{q}"), &q.to_string(), self.jpeg_quality == q, cx.listener(move |this, _, _, cx| {
                            this.jpeg_quality = q;
                            this.save_settings();
                            cx.notify();
                        }))
                    })),
            );
        }

        // Left-nav drives which section's panel(s) render in the content pane (#71). The prep
        // blocks above are cheap to build; only the active section's elements are pushed.
        let section = self.settings_section;
        let mut content: Vec<gpui::AnyElement> = Vec::new();

        if section == SettingsSection::CaptureQuality {
            content.push(quality_panel.into_any_element());
        }

        if section == SettingsSection::Updates {
            content.push({
            // App update (#48): offer a newer GitHub release; install only after confirm.
            let panel = div()
                .flex()
                .flex_col()
                .gap(px(theme::SP_2))
                .child(eyebrow("Version"));
            let mut row = div()
                .flex()
                .gap_2()
                .items_center()
                .child(self.field_label("App", 70.0));
            match (&self.update_info, self.updating) {
                (_, true) => {
                    // The DMG/exe is ~175 MB, so show a real progress bar (#48). `t == 0` means the
                    // server didn't send Content-Length yet → indeterminate (just downloaded MB).
                    let (d, t) = self.update_progress.unwrap_or((0, 0));
                    let dmb = d as f64 / 1_048_576.0;
                    if t > 0 {
                        let frac = (d as f32 / t as f32).clamp(0.0, 1.0);
                        let tmb = t as f64 / 1_048_576.0;
                        row = row
                            .child(div().w(px(160.0)).child(progress_bar(frac, false)))
                            .child(
                                div()
                                    .text_size(px(theme::TS_SMALL))
                                    .text_color(rgb(theme::ACCENT_TEXT))
                                    .child(format!(
                                        "downloading update… {}%  ({:.0}/{:.0} MB)",
                                        (frac * 100.0) as i32,
                                        dmb,
                                        tmb,
                                    )),
                            );
                    } else {
                        row = row.child(
                            div()
                                .text_size(px(theme::TS_SMALL))
                                .text_color(rgb(theme::ACCENT_TEXT))
                                .child(format!("downloading update… ({:.0} MB)", dmb)),
                        );
                    }
                }
                (Some(info), false) => {
                    let info2 = info.clone();
                    row = row
                        .child(div().text_color(rgb(theme::WARNING)).child(format!("v{} available (you have v{})", info.version, update::CURRENT)))
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
                    row = row.child(div().text_color(rgb(theme::TEXT_MUTED)).child(format!("v{} · up to date", update::CURRENT)));
                }
            }
            panel.child(row).into_any_element()
            });
        }

        if section == SettingsSection::Transcription {
            content.push({
            // Transcription settings (#45): language + chunk length. Pinning the language
            // stops Whisper hallucinating "Thank you." on short non-English chunks; a 30s
            // chunk is the reliable default.
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SP_3))
                .child(self.language_field(asr_lang_focused, cx))
                .child(self.chunk_chips(cx))
                .into_any_element()
            });
        }

        if section == SettingsSection::IndexEndpoint {
            content.push({
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
            let mut panel = div()
                .flex()
                .flex_col()
                .gap(px(theme::SP_3))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(theme::TS_BODY))
                                .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                                .text_color(rgb(theme::TEXT_PRIMARY))
                                .child("Index endpoint"),
                        )
                        .child(status_pill_el),
                )
                // Provider chips: selecting prefills the port + re-fetches models.
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(self.field_label("provider", 60.0))
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
                // Host (or "Base URL" for the custom provider).
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(self.field_label(if is_base { "base URL" } else { "host" }, 60.0))
                        .child(
                            div()
                                .id("index-host-input")
                                .track_focus(&self.index_host_focus)
                                .key_context("index-host")
                                .on_key_down(cx.listener(|this, ev, _w, cx| this.on_index_field_key(IndexField::Host, ev, cx)))
                                .flex_1()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(if index_host_focused { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
                                .bg(rgb(theme::PANEL))
                                .text_color(if self.index_host.is_empty() { rgb(theme::TEXT_MUTED) } else { rgb(theme::TEXT_PRIMARY) })
                                .child(if self.index_host.is_empty() {
                                    if is_base { "http://1.2.3.4:8000/v1  (Enter to check)".to_string() }
                                    else { "192.168.31.217  (Enter to check)".to_string() }
                                } else if index_host_focused {
                                    format!("{}▏", self.index_host)
                                } else {
                                    self.index_host.clone()
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    window.focus(&this.index_host_focus);
                                    cx.notify();
                                })),
                        )
                        .child(button("Check", ButtonVariant::Secondary, cx.listener(|this, _, _, cx| this.probe_index_status(cx)))),
                );
            // Port (host:port providers only — custom hides it).
            if !is_base {
                panel = panel.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(self.field_label("port", 60.0))
                        .child(
                            div()
                                .id("index-port-input")
                                .track_focus(&self.index_port_focus)
                                .key_context("index-port")
                                .on_key_down(cx.listener(|this, ev, _w, cx| this.on_index_field_key(IndexField::Port, ev, cx)))
                                .w(px(110.0))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(if index_port_focused { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
                                .bg(rgb(theme::PANEL))
                                .text_color(if self.index_port.is_empty() { rgb(theme::TEXT_MUTED) } else { rgb(theme::TEXT_PRIMARY) })
                                .child(if self.index_port.is_empty() {
                                    "1234".to_string()
                                } else if index_port_focused {
                                    format!("{}▏", self.index_port)
                                } else {
                                    self.index_port.clone()
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    window.focus(&this.index_port_focus);
                                    cx.notify();
                                })),
                        ),
                );
            }
            // API key (openai only).
            if self.index_needs_key() {
                panel = panel.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(self.field_label("API key", 60.0))
                        .child(
                            div()
                                .id("index-key-input")
                                .track_focus(&self.index_key_focus)
                                .key_context("index-key")
                                .on_key_down(cx.listener(|this, ev, _w, cx| this.on_index_field_key(IndexField::Key, ev, cx)))
                                .flex_1()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(if index_key_focused { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
                                .bg(rgb(theme::PANEL))
                                .text_color(if self.index_key.is_empty() { rgb(theme::TEXT_MUTED) } else { rgb(theme::TEXT_PRIMARY) })
                                .child(if self.index_key.is_empty() {
                                    "sk-…  (Enter to check)".to_string()
                                } else if index_key_focused {
                                    format!("{}▏", self.index_key)
                                } else {
                                    self.index_key.clone()
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    window.focus(&this.index_key_focus);
                                    cx.notify();
                                })),
                        ),
                );
            }
            // Model dropdown (#53) + Refresh, reusing the language-dropdown pattern.
            panel = panel.child(self.index_model_field(cx));
            panel
                .child(
                    // Leaf sampling rate: caption every round(1/rate)-th frame. Coarser =
                    // far fewer vision calls (a long session has thousands of frames).
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(self.field_label("frames", 44.0))
                        .children([1.0f64, 0.5, 0.25, 0.1, 0.05].into_iter().map(|r| {
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
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(self.field_label("about", 44.0))
                        .children(
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
                )
                .into_any_element()
            });
        }

        if section == SettingsSection::Skills {
            content.push({
            // One row per agent: the skill label on the left; an install/update action
            // (or an "installed" pill) on the right, per its cached status (§5).
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SP_2))
                .children(skill::AGENTS.iter().enumerate().map(|(ix, a)| {
                    let mut right = div().flex().items_center().gap(px(theme::SP_2));
                    match self.skill_status.get(ix) {
                        Some(skill::SkillStatus::UpToDate) => {
                            right = right.child(status_pill("installed", theme::SUCCESS, theme::SUCCESS_SUBTLE));
                        }
                        Some(skill::SkillStatus::UpdateAvailable) => {
                            right = right
                                .child(status_pill("update", theme::WARNING, theme::WARNING_SUBTLE))
                                .child(button(
                                    "Update",
                                    ButtonVariant::Secondary,
                                    cx.listener(move |this, _, _, cx| this.install_skill(ix, cx)),
                                ));
                        }
                        _ => {
                            right = right.child(button(
                                "Install",
                                ButtonVariant::Primary,
                                cx.listener(move |this, _, _, cx| this.install_skill(ix, cx)),
                            ));
                        }
                    }
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(theme::SP_3))
                        .py(px(10.0))
                        .px(px(12.0))
                        .rounded(px(theme::RADIUS_MD))
                        .border_1()
                        .border_color(rgb(theme::BORDER))
                        .bg(rgb(theme::PANEL))
                        .child(
                            div()
                                .text_size(px(theme::TS_BODY))
                                .font_weight(gpui::FontWeight(theme::FW_MEDIUM as f32))
                                .text_color(rgb(theme::TEXT_PRIMARY))
                                .child(a.label),
                        )
                        .child(right)
                }))
                .into_any_element()
            });
        }

        if section == SettingsSection::Permissions {
            content.push({
            // Permissions (macOS): Screen Recording + Microphone status, Grant
            // (prompt), Settings (grant/revoke), Restart daemon (apply a new Screen
            // Recording grant without quitting the app — the agent respawns it).
            let sr = self.perms.screen_recording.clone();
            let mic = self.perms.microphone.clone();
            let show = matches!(sr.as_str(), "granted" | "denied")
                || matches!(mic.as_str(), "granted" | "denied" | "undetermined");
            let mut panel = div().flex().flex_col().gap(px(theme::SP_2));
            if show {
                panel = panel
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
                    ))
                    .child(
                        div().pt(px(theme::SP_1)).child(button(
                            "Restart daemon",
                            ButtonVariant::Secondary,
                            cx.listener(|this, _, _, cx| this.restart_daemon(cx)),
                        )),
                    );
            }
            panel.into_any_element()
            });
        }

        if section == SettingsSection::Voice {
            content.push(runtime_panel.into_any_element());
            content.push(asr_panel.into_any_element());
        }

        let content_pane = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .pl(px(24.0))
            .child(
                div()
                    .text_size(px(theme::TS_SECTION))
                    .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child(section.label()),
            )
            .children(content);

        vec![div()
            .flex()
            .w_full()
            .child(self.settings_nav(cx))
            .child(content_pane)
            .into_any_element()]
    }

    /// The Settings left-nav (#71): the "Capture" title + daemon status pill + hotkey hint,
    /// then the clickable section list. Clicking a row sets `settings_section` (→ re-render).
    pub(crate) fn settings_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (daemon_line, reachable) = match &self.health {
            Some(h) if h.ok => (format!("daemon v{} · pid {}", h.version, h.pid), true),
            _ => ("no daemon".to_string(), false),
        };
        let mut nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(200.0))
            .gap_1()
            .pr(px(16.0))
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(
                div()
                    .text_size(px(theme::TS_TITLE))
                    .font_weight(gpui::FontWeight(theme::FW_SEMIBOLD as f32))
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .child("Capture"),
            )
            .child(
                div()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::TEXT_MUTED))
                    .child(daemon_line),
            )
            .child(if reachable {
                status_pill("reachable", theme::SUCCESS, theme::SUCCESS_SUBTLE)
            } else {
                status_pill("offline", theme::ERROR, theme::ERROR_SUBTLE)
            });
        if self.hotkey_id != 0 {
            nav = nav.child(
                div()
                    .text_size(px(theme::TS_SMALL))
                    .text_color(rgb(theme::ACCENT_TEXT))
                    .child(format!("{} toggles capture", crate::hotkey::LABEL)),
            );
        }
        nav = nav.child(div().h(px(8.0))); // spacer before the section list
        for s in SettingsSection::ALL {
            let active = s == self.settings_section;
            nav = nav.child(
                div()
                    .id(SharedString::from(s.label()))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py(px(6.0))
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgb(theme::ACCENT_SUBTLE)))
                    .text_color(rgb(if active { theme::ACCENT_TEXT_STRONG } else { theme::TEXT_SECONDARY }))
                    .when(!active, |d| d.hover(|st| st.bg(rgba(theme::GHOST_HOVER))))
                    .child(icon(s.icon(), 15.0, if active { theme::ACCENT_TEXT_STRONG } else { theme::TEXT_MUTED }))
                    .child(div().text_size(px(theme::TS_BODY)).child(s.label()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_section = s;
                        cx.notify();
                    })),
            );
        }
        nav
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

        let mut col = div().flex().flex_col().gap_1().child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(self.field_label("Language", 70.0))
                .child(
                    div()
                        .id("asr-lang-input")
                        .track_focus(&self.asr_language_focus)
                        .key_context("asr-lang")
                        .on_key_down(cx.listener(Self::on_asr_language_key))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(theme::SP_2))
                        .w(px(220.0))
                        .py(px(theme::SP_2))
                        .px(px(theme::SP_3))
                        .rounded(px(theme::RADIUS_MD))
                        .text_size(px(theme::TS_BODY))
                        .border_1()
                        .border_color(if focused || open { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
                        .bg(rgb(theme::PANEL))
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
            // §4 dropdown menu surface: ELEVATED / 1px BORDER, radius 6, pad 4.
            let mut list = div()
                .flex()
                .flex_col()
                .ml(px(78.0))
                .w(px(220.0))
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
            col = col.child(list);
        }
        col
    }

    /// The transcription chunk-length chips (#45). Larger windows avoid Whisper's
    /// short-chunk hallucination; smaller = lower latency.
    pub(crate) fn chunk_chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cur = self.asr.chunk_seconds;
        div()
            .flex()
            .gap_2()
            .items_center()
            .child(self.field_label("Chunk", 70.0))
            .children([8.0f64, 15.0, 30.0, 60.0].into_iter().map(|s| {
                chip(
                    &format!("chunk-{s}"),
                    &format!("{s:.0}s"),
                    (cur - s).abs() < 0.5,
                    cx.listener(move |this, _, _, cx| this.set_asr_chunk(s, cx)),
                )
            }))
    }

    /// A consistent left-hand field label (min-width aligned, TEXT_SECONDARY body)
    /// for the row-based controls (capture quality, transcription, index endpoint).
    fn field_label(&self, text: &'static str, min_w: f32) -> impl IntoElement {
        div()
            .min_w(px(min_w))
            .text_size(px(theme::TS_BODY))
            .text_color(rgb(theme::TEXT_SECONDARY))
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
        let glyph = if kind == "microphone" { "mic" } else { "shield" };
        let left = div()
            .flex()
            .items_center()
            .gap(px(theme::SP_3))
            .child(icon(glyph, 16.0, theme::TEXT_MUTED))
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
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TS_SMALL))
                            .text_color(rgb(theme::TEXT_MUTED))
                            .child(why),
                    ),
            );

        // "not granted" stays WARNING amber here — the ERROR-red blocking treatment is
        // the dashboard's job (#74). Undetermined → neutral "not requested".
        let mut right = div().flex().items_center().gap(px(theme::SP_2));
        match status {
            "granted" => {
                right = right.child(status_pill("granted", theme::SUCCESS, theme::SUCCESS_SUBTLE));
            }
            "undetermined" => {
                right = right
                    .child(status_pill("not requested", theme::TEXT_MUTED, theme::CHIP_IDLE))
                    .child(button(
                        "Settings",
                        ButtonVariant::Secondary,
                        cx.listener(move |this, _, _, cx| this.open_privacy_settings(pane, cx)),
                    ));
            }
            _ => {
                right = right.child(status_pill("not granted", theme::WARNING, theme::WARNING_SUBTLE));
                if can_prompt {
                    right = right.child(button(
                        "Grant",
                        ButtonVariant::Primary,
                        cx.listener(move |this, _, _, cx| this.request_permission(kind, cx)),
                    ));
                }
                right = right.child(button(
                    "Settings",
                    ButtonVariant::Secondary,
                    cx.listener(move |this, _, _, cx| this.open_privacy_settings(pane, cx)),
                ));
            }
        }

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(theme::SP_3))
            .py(px(10.0))
            .px(px(12.0))
            .rounded(px(theme::RADIUS_MD))
            .border_1()
            .border_color(rgb(theme::BORDER))
            .bg(rgb(theme::PANEL))
            .child(left)
            .child(right)
    }
}
