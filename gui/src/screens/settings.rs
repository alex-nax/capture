//! The Settings screen render branch + its field/row helper methods. Relocated verbatim
//! from `app.rs` `render()` (#68). ONE file for now (per #68; per-section split is later).
//! Returns the ordered list of settings children; the shell in `app.rs` appends them via
//! `.children(...)` exactly as before.

use gpui::{div, prelude::*, px, relative, rgb, Context, SharedString, Window};

use crate::app::CaptureApp;
use crate::components::{button, chip};
use crate::skill;
use crate::state::{ConfirmKind, IndexField, INDEX_PROVIDERS, LANGUAGES, RES_PRESETS};
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

        // Whisper model manager: per-model status + Download / Use actions. Live
        // download progress comes from the SSE-fed `asr_progress` map.
        let asr_progress = self.live.lock().unwrap().asr_progress.clone();
        let model_rows: Vec<_> = self
            .asr
            .models
            .iter()
            .map(|m| {
                let repo = m.repo.clone();
                let prog = asr_progress.get(&repo).copied();
                // An active model that isn't downloaded yet still needs a Download —
                // call that out (amber) so "active" doesn't look ready when it isn't.
                let (status, status_color) = if let Some(f) = prog {
                    (format!("↓ {:.0}%", (f * 100.0).clamp(0.0, 100.0)), theme::SUCCESS)
                } else if m.downloading {
                    ("↓ downloading…".to_string(), theme::SUCCESS)
                } else if m.active && m.downloaded {
                    ("● active".to_string(), theme::SUCCESS)
                } else if m.active {
                    ("● active · needs download".to_string(), theme::WARNING)
                } else if m.downloaded {
                    ("✓ downloaded".to_string(), theme::SUCCESS)
                } else {
                    (String::new(), theme::TEXT_MUTED)
                };
                let busy = prog.is_some() || m.downloading;
                let mut header = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(format!("{}  ·  {}", m.name, m.size_label)),
                    )
                    .child(div().text_color(rgb(status_color)).child(status));
                if !m.downloaded && !busy {
                    let r = repo.clone();
                    header = header.child(
                        div()
                            .id(SharedString::from(format!("dl-{repo}")))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(theme::ACCENT))
                            .child("Download")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.download_model(r.clone(), cx)
                            })),
                    );
                } else if m.downloaded {
                    // "Use" only when it isn't already active; "Remove" for any
                    // downloaded model (removing the active one just reverts it to
                    // "active · needs download" until re-fetched).
                    if !m.active {
                        let r = repo.clone();
                        header = header.child(
                            div()
                                .id(SharedString::from(format!("use-{repo}")))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgb(theme::ACCENT))
                                .child("Use")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_active_model(r.clone(), cx)
                                })),
                        );
                    }
                    let r = repo.clone();
                    header = header.child(
                        div()
                            .id(SharedString::from(format!("rm-{repo}")))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(theme::ERROR_SUBTLE))
                            .text_color(rgb(theme::ERROR))
                            .child("Remove")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_model(r.clone(), cx)
                            })),
                    );
                }
                let mut row = div().flex().flex_col().gap_1().child(header);
                if busy {
                    // A thin determinate bar — the fill width tracks the SSE-fed
                    // fraction (0.0 until the first progress event lands).
                    let frac = prog.unwrap_or(0.0).clamp(0.0, 1.0);
                    row = row.child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded_full()
                            .bg(rgb(theme::ELEVATED))
                            .child(
                                div()
                                    .h(px(4.0))
                                    .w(relative(frac))
                                    .rounded_full()
                                    .bg(rgb(theme::SUCCESS)),
                            ),
                    );
                }
                row
            })
            .collect();
        let asr_label = if self.asr.backend_available {
            "Whisper models  (downloaded on demand · ~/.cache/huggingface)".to_string()
        } else {
            "Whisper models  (runtime unavailable in this daemon — capture still works)".to_string()
        };
        // Voice-recognition runtime picker (#58): no engine is bundled by default — the user installs
        // a runtime pack matching their hardware, then picks a model (below). Install progress comes
        // from the SSE-fed `runtime_install` map; a GPU hint suggests the right one.
        let rt_install = self.live.lock().unwrap().runtime_install.clone();
        let rt_rows: Vec<_> = self
            .runtimes
            .runtimes
            .iter()
            .map(|rt| {
                let id = rt.id.clone();
                let prog = rt_install.get(&id).copied();
                let (status, color) = if rt.active {
                    ("● active".to_string(), theme::SUCCESS)
                } else if let Some(f) = prog {
                    (format!("↓ {:.0}%", (f * 100.0).clamp(0.0, 100.0)), theme::SUCCESS)
                } else if rt.installed {
                    ("✓ installed".to_string(), theme::TEXT_MUTED)
                } else {
                    (String::new(), theme::TEXT_MUTED)
                };
                let busy = prog.is_some();
                let mut header = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(div().flex_1().child(rt.label.clone()))
                    .child(div().text_color(rgb(color)).child(status));
                // remote: "Use" (no install); local not-installed: "Install"; installed & inactive: "Use".
                if rt.kind == "remote" && !rt.active {
                    let i = id.clone();
                    header = header.child(
                        div().id(SharedString::from(format!("rt-use-{id}"))).px_2().py_1().rounded_md()
                            .cursor_pointer().bg(rgb(theme::ACCENT)).child("Use")
                            .on_click(cx.listener(move |this, _, _, cx| this.set_runtime(i.clone(), cx))),
                    );
                } else if rt.kind != "remote" && !rt.installed && !busy {
                    let i = id.clone();
                    header = header.child(
                        div().id(SharedString::from(format!("rt-inst-{id}"))).px_2().py_1().rounded_md()
                            .cursor_pointer().bg(rgb(theme::ACCENT)).child("Install")
                            .on_click(cx.listener(move |this, _, _, cx| this.install_runtime(i.clone(), cx))),
                    );
                } else if rt.installed && !rt.active {
                    let i = id.clone();
                    header = header.child(
                        div().id(SharedString::from(format!("rt-use-{id}"))).px_2().py_1().rounded_md()
                            .cursor_pointer().bg(rgb(theme::ACCENT)).child("Use")
                            .on_click(cx.listener(move |this, _, _, cx| this.set_runtime(i.clone(), cx))),
                    );
                }
                let mut row = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(header)
                    .child(div().text_color(rgb(theme::TEXT_MUTED)).child(rt.requires.clone()));
                if busy {
                    let frac = prog.unwrap_or(0.0).clamp(0.0, 1.0);
                    row = row.child(
                        div().w_full().h(px(4.0)).rounded_full().bg(rgb(theme::ELEVATED)).child(
                            div().h(px(4.0)).w(relative(frac)).rounded_full().bg(rgb(theme::SUCCESS)),
                        ),
                    );
                }
                row
            })
            .collect();
        let rt_hint = if self.runtimes.gpu.nvidia {
            "Voice recognition runtime  (NVIDIA GPU detected — the CUDA runtime is recommended)"
        } else {
            "Voice recognition runtime  (no NVIDIA GPU detected — use CPU or a remote endpoint)"
        };
        let runtime_panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_color(rgb(theme::TEXT_PRIMARY)).child(rt_hint.to_string()))
            .children(rt_rows);

        let mut asr_panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_color(rgb(theme::TEXT_PRIMARY)).child(asr_label));
        if self.asr.backend_available {
            asr_panel = asr_panel.children(model_rows);
        }

        // Capture-quality settings (Settings screen): screenshot format + resolution
        // + jpeg quality, applied to new captures via shot_settings().
        let is_jpeg = self.shot_format == "jpeg";
        let mut quality_panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_color(rgb(theme::TEXT_PRIMARY)).child("Capture quality"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().min_w(px(96.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("Screenshots"))
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
                    .child(div().min_w(px(96.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("Format"))
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
                    .child(div().min_w(px(96.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("Resolution"))
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
                    .child(div().min_w(px(96.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("JPEG quality"))
                    .children([60u32, 80, 95].into_iter().map(|q| {
                        chip(&format!("q-{q}"), &q.to_string(), self.jpeg_quality == q, cx.listener(move |this, _, _, cx| {
                            this.jpeg_quality = q;
                            this.save_settings();
                            cx.notify();
                        }))
                    })),
            );
        }

        let mut out: Vec<gpui::AnyElement> = Vec::new();

        // Settings screen: capture quality (+ voice model / permissions / skill below).
        out.push(quality_panel.into_any_element());

        out.push({
            // App update (#48): offer a newer GitHub release; install only after confirm.
            let mut row = div()
                .flex()
                .gap_2()
                .items_center()
                .child(div().min_w(px(70.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("App"));
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
                            .child(
                                div()
                                    .w(px(160.0))
                                    .h(px(6.0))
                                    .rounded_sm()
                                    .bg(rgb(theme::ELEVATED))
                                    .child(
                                        div()
                                            .h(px(6.0))
                                            .w(px(160.0 * frac))
                                            .rounded_sm()
                                            .bg(rgb(theme::ACCENT)),
                                    ),
                            )
                            .child(div().text_color(rgb(theme::ACCENT_TEXT)).child(format!(
                                "downloading update… {}%  ({:.0}/{:.0} MB)",
                                (frac * 100.0) as i32,
                                dmb,
                                tmb,
                            )));
                    } else {
                        row = row.child(
                            div()
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
            row.into_any_element()
        });

        out.push({
            // Transcription settings (#45): language + chunk length. Pinning the language
            // stops Whisper hallucinating "Thank you." on short non-English chunks; a 30s
            // chunk is the reliable default.
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(rgb(theme::TEXT_PRIMARY)).child("Transcription"))
                .child(self.language_field(asr_lang_focused, cx))
                .child(self.chunk_chips(cx))
                .into_any_element()
        });

        out.push({
            // Multimodal index endpoint (#52/#53): structured provider + host:port + key, and a
            // model dropdown. Indexing is OFF until set AND reachable (the dot reflects status).
            let (dot, label) = if self.index_status.available {
                (theme::SUCCESS, "reachable")
            } else if self.index_status.configured {
                (theme::ERROR, "unreachable")
            } else {
                (theme::TEXT_MUTED, "not set")
            };
            let is_base = self.index_is_base_url();
            let mut panel = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(div().text_color(rgb(theme::TEXT_PRIMARY)).child("Index endpoint"))
                        .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(rgb(dot)))
                        .child(div().text_color(rgb(theme::TEXT_MUTED)).child(label)),
                )
                // Provider chips: selecting prefills the port + re-fetches models.
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(div().min_w(px(60.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("provider"))
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
                        .child(
                            div().min_w(px(60.0)).text_color(rgb(theme::TEXT_SECONDARY))
                                .child(if is_base { "base URL" } else { "host" }),
                        )
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
                        .child(button("Check", cx.listener(|this, _, _, cx| this.probe_index_status(cx)))),
                );
            // Port (host:port providers only — custom hides it).
            if !is_base {
                panel = panel.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(div().min_w(px(60.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("port"))
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
                        .child(div().min_w(px(60.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("API key"))
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
                        .child(div().min_w(px(44.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("frames"))
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
                        .child(div().min_w(px(44.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("about"))
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

        out.push({
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(div().text_color(rgb(theme::TEXT_PRIMARY)).child("Skill →"))
                .children(skill::AGENTS.iter().enumerate().map(|(ix, a)| {
                    let label = match self.skill_status.get(ix) {
                        Some(skill::SkillStatus::UpToDate) => format!("{} ✓", a.label),
                        Some(skill::SkillStatus::UpdateAvailable) => format!("{} ↑ update", a.label),
                        _ => format!("{} — install", a.label),
                    };
                    button(&label, cx.listener(move |this, _, _, cx| this.install_skill(ix, cx)))
                }))
                .into_any_element()
        });

        out.push({
            // Permissions (macOS): Screen Recording + Microphone status, Grant
            // (prompt), Settings (grant/revoke), Restart daemon (apply a new Screen
            // Recording grant without quitting the app — the agent respawns it).
            let sr = self.perms.screen_recording.clone();
            let mic = self.perms.microphone.clone();
            let show = matches!(sr.as_str(), "granted" | "denied")
                || matches!(mic.as_str(), "granted" | "denied" | "undetermined");
            let mut panel = div().flex().flex_col().gap_1();
            if show {
                panel = panel
                    .child(div().text_color(rgb(theme::TEXT_PRIMARY)).child("Permissions"))
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
                    .child(button(
                        "Restart daemon",
                        cx.listener(|this, _, _, cx| this.restart_daemon(cx)),
                    ));
            }
            panel.into_any_element()
        });

        out.push(runtime_panel.into_any_element());
        out.push(asr_panel.into_any_element());

        out
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
        let field_text = if self.lang_dropdown_open && !self.asr_language.is_empty() {
            format!("{}▏", self.asr_language)
        } else if self.lang_dropdown_open && focused {
            "type to search…".to_string()
        } else if active.is_empty() {
            "Auto-detect ▾".to_string()
        } else {
            format!("{active} · {active_name} ▾")
        };
        let dim = self.lang_dropdown_open && self.asr_language.is_empty();

        let mut col = div().flex().flex_col().gap_1().child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(div().min_w(px(70.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("Language"))
                .child(
                    div()
                        .id("asr-lang-input")
                        .track_focus(&self.asr_language_focus)
                        .key_context("asr-lang")
                        .on_key_down(cx.listener(Self::on_asr_language_key))
                        .w(px(220.0))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(if focused { rgb(theme::ACCENT_BORDER) } else { rgb(theme::BORDER) })
                        .bg(rgb(theme::PANEL))
                        .cursor_pointer()
                        .text_color(if dim { rgb(theme::TEXT_MUTED) } else { rgb(theme::TEXT_PRIMARY) })
                        .child(field_text)
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
            let mut list = div()
                .flex()
                .flex_col()
                .ml(px(78.0))
                .w(px(220.0))
                .rounded_md()
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
                list = list.child(
                    div()
                        .id(SharedString::from(format!("lang-row-{code}")))
                        .flex()
                        .gap_2()
                        .items_center()
                        .px_2()
                        .py_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::BORDER)))
                        .when(is_active, |s| s.bg(rgb(theme::ACCENT_SUBTLE)))
                        .text_color(rgb(theme::TEXT_SECONDARY))
                        .child(div().min_w(px(28.0)).text_color(rgb(theme::ACCENT_TEXT)).child(if code.is_empty() { "—" } else { *code }))
                        .child(div().child(*name))
                        .on_click(cx.listener(move |this, _, _, cx| this.apply_language_code(code_s.clone(), cx))),
                );
            }
            if !any {
                list = list.child(div().px_2().py_1().text_color(rgb(theme::TEXT_MUTED)).child("no match"));
            } else if total > 12 {
                list = list.child(
                    div().px_2().py_1().text_xs().text_color(rgb(theme::TEXT_MUTED)).child(format!("+{} more — keep typing", total - 12)),
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
            .child(div().min_w(px(70.0)).text_color(rgb(theme::TEXT_SECONDARY)).child("Chunk"))
            .children([8.0f64, 15.0, 30.0, 60.0].into_iter().map(|s| {
                chip(
                    &format!("chunk-{s}"),
                    &format!("{s:.0}s"),
                    (cur - s).abs() < 0.5,
                    cx.listener(move |this, _, _, cx| this.set_asr_chunk(s, cx)),
                )
            }))
    }

    /// One permission row: status + (a Grant button if it's promptable here) + Settings.
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
        let (label, color, granted) = match status {
            "granted" => (format!("{title}: ✓ granted"), theme::SUCCESS, true),
            "undetermined" => (format!("{title}: not requested"), theme::TEXT_MUTED, false),
            _ => (format!("{title}: ✗ not granted — needed for {why}"), theme::WARNING, false),
        };
        let mut row = div()
            .flex()
            .gap_2()
            .items_center()
            .child(div().min_w(px(140.0)).text_color(rgb(color)).child(label));
        if !granted && can_prompt {
            row = row.child(
                div()
                    .id(SharedString::from(format!("grant-{kind}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(theme::ACCENT))
                    .child("Grant")
                    .on_click(cx.listener(move |this, _, _, cx| this.request_permission(kind, cx))),
            );
        }
        row.child(
            div()
                .id(SharedString::from(format!("settings-{kind}")))
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .bg(rgb(theme::CHIP_IDLE))
                .child("Settings")
                .on_click(cx.listener(move |this, _, _, cx| this.open_privacy_settings(pane, cx))),
        )
    }
}
