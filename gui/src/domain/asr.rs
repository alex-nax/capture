//! ASR actions: model download/use/remove, runtime install/select, transcription
//! language + chunk, skill install, and the macOS permission prompts. Relocated
//! verbatim from `app.rs` (#68). Bucket note: permissions + skill install landed here
//! as the closest "settings-ish action" bucket (no separate module per #68's guidance).

use gpui::{Context, KeyDownEvent, Window};

use crate::app::CaptureApp;
use crate::skill;

impl CaptureApp {
    pub(crate) fn refresh_skill_status(&mut self) {
        self.skill_status = skill::AGENTS.iter().map(skill::status).collect();
    }

    pub(crate) fn install_skill(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(agent) = skill::AGENTS.get(ix) else { return };
        self.message = match skill::install(agent) {
            Ok(path) => format!("installed the capture skill → {}", path.display()).into(),
            Err(e) => format!("skill install failed ({}): {e}", agent.label).into(),
        };
        self.refresh_skill_status();
        cx.notify();
    }

    /// Kick off a model download on the daemon (progress streams over SSE into
    /// `live.asr_progress`; the poll loop refreshes the catalog's flags).
    pub(crate) fn download_model(&mut self, repo: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else {
            self.message = "no daemon".into();
            cx.notify();
            return;
        };
        // Optimistically show a 0% bar so the row reacts immediately.
        self.live.lock().unwrap().asr_progress.insert(repo.clone(), 0.0);
        self.message = format!("downloading {}…", repo.rsplit('/').next().unwrap_or(&repo)).into();
        cx.notify();
        let live = self.live.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn({
                    let repo = repo.clone();
                    async move { d.asr_download(&repo) }
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Err(e) = r {
                    live.lock().unwrap().asr_progress.remove(&repo);
                    v.message = format!("download failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Set the active Whisper model (new captures transcribe with it).
    pub(crate) fn set_active_model(&mut self, repo: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        let short = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn({
                    let repo = repo.clone();
                    async move { d.asr_set_model(&repo) }
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = match r {
                    Ok(()) => format!("active model: {short}").into(),
                    Err(e) => format!("set model failed: {e}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Install an ASR runtime pack on the daemon (download/extract in the background; progress streams
    /// over SSE into `live.runtime_install`; the daemon makes it active when done).
    pub(crate) fn install_runtime(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else {
            self.message = "no daemon".into();
            cx.notify();
            return;
        };
        self.live.lock().unwrap().runtime_install.insert(id.clone(), 0.0);
        self.message = format!("installing {id} runtime…").into();
        cx.notify();
        let live = self.live.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn({
                    let id = id.clone();
                    async move { d.asr_runtime_install(&id) }
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Err(e) = r {
                    live.lock().unwrap().runtime_install.remove(&id);
                    v.message = format!("runtime install failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Select an installed ASR runtime (new captures transcribe with it).
    pub(crate) fn set_runtime(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn({
                    let id = id.clone();
                    async move { d.asr_set_runtime(&id) }
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = match r {
                    Ok(()) => format!("active runtime: {id}").into(),
                    Err(e) => format!("set runtime failed: {e}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Remove a downloaded model's weights from the HF cache (frees disk). The poll
    /// loop refreshes the catalog so the row flips back to "Download" once gone.
    pub(crate) fn delete_model(&mut self, repo: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        let short = repo.rsplit('/').next().unwrap_or(&repo).to_string();
        self.message = format!("removing {short}…").into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn({
                    let repo = repo.clone();
                    async move { d.asr_delete(&repo) }
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = match r {
                    Ok(()) => format!("removed {short}").into(),
                    Err(e) => format!("remove failed: {e}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Key handling for the transcription-language field (#45). Enter applies it.
    pub(crate) fn on_asr_language_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        if m.platform && ks.key == "v" {
            if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                self.asr_language.push_str(t.trim());
                cx.notify();
            }
            return;
        }
        if m.platform || m.control || m.function {
            return;
        }
        match ks.key.as_str() {
            "backspace" => {
                self.asr_language.pop();
            }
            "escape" => {
                self.lang_dropdown_open = false;
                self.asr_language.clear();
            }
            "enter" => {
                // Pick the top filtered language, else apply the raw text as an ISO code.
                match self.top_lang_match() {
                    Some(code) => self.apply_language_code(code.to_string(), cx),
                    None => self.apply_asr_language(cx),
                }
                return;
            }
            _ => {
                if let Some(c) = ks.key_char.as_deref() {
                    if !c.is_empty() && !c.chars().any(char::is_control) {
                        self.asr_language.push_str(c);
                        self.lang_dropdown_open = true; // typing opens/refines the list
                    }
                }
            }
        }
        cx.notify();
    }

    /// Set the transcription language (persisted; applies to running captures on the next
    /// chunk + to re-transcribes). Blank / "auto" clears it.
    pub(crate) fn apply_asr_language(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        let lang = self.asr_language.trim().to_string();
        self.message = if lang.is_empty() || lang == "auto" {
            "language: auto-detect".into()
        } else {
            format!("language: {lang}").into()
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { d.asr_set_language(&lang) }).await;
            let _ = this.update(cx, |v, cx| {
                if let Err(e) = r {
                    v.message = format!("set language failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The best language match for the current filter text (code/name prefix, then contains).
    pub(crate) fn top_lang_match(&self) -> Option<&'static str> {
        use crate::state::LANGUAGES;
        let f = self.asr_language.trim().to_lowercase();
        if f.is_empty() {
            return None;
        }
        LANGUAGES
            .iter()
            .find(|(c, n)| c.eq_ignore_ascii_case(&f) || c.starts_with(&f) || n.to_lowercase().starts_with(&f))
            .or_else(|| LANGUAGES.iter().find(|(c, n)| n.to_lowercase().contains(&f) || c.contains(&f)))
            .map(|(c, _)| *c)
    }

    /// Apply a language picked from the dropdown (persisted; on-the-fly for running captures).
    pub(crate) fn apply_language_code(&mut self, code: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.lang_dropdown_open = false;
        self.asr_language.clear(); // clear the filter — the field then shows the active value
        self.message = if code.is_empty() {
            "language: auto-detect".into()
        } else {
            format!("language: {code}").into()
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { d.asr_set_language(&code) }).await;
            let _ = this.update(cx, |v, cx| {
                if let Err(e) = r {
                    v.message = format!("set language failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Set the transcription chunk length in seconds (persisted).
    pub(crate) fn set_asr_chunk(&mut self, seconds: f64, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = format!("chunk length: {seconds:.0}s").into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { d.asr_set_chunk(seconds) }).await;
            let _ = this.update(cx, |v, cx| {
                if let Err(e) = r {
                    v.message = format!("set chunk failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    // -- permissions (macOS) ----------------------------------------------------

    /// Dispatch a permission Grant by kind. Neither prompt goes through the headless
    /// daemon (it aborts): **Screen Recording** is prompted in THIS process via
    /// CoreGraphics; **Microphone** via the bundled agent one-shot. Both work because
    /// every binary shares the Developer-ID Team ID, so the grant reaches the daemon.
    pub(crate) fn request_permission(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        match kind {
            "microphone" => self.request_microphone(cx),
            _ => self.request_screen_recording(cx),
        }
    }

    pub(crate) fn request_screen_recording(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        let already = crate::app::screen_perm::request();
        #[cfg(not(target_os = "macos"))]
        let already = false;
        self.message = if already {
            "Screen Recording already granted".into()
        } else {
            "approve the prompt, then click Restart daemon so the daemon picks it up".into()
        };
        cx.notify();
    }

    /// Spawn the bundled menu-bar agent as a one-shot (`CaptureBar --request-mic`) to
    /// show the Microphone prompt — Swift's `AVCaptureDevice.requestAccess` is clean,
    /// and the shared Team ID carries the grant to the daemon. (The daemon itself
    /// can't prompt — it aborts headless.)
    pub(crate) fn request_microphone(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            let spawned = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|d| d.join("CaptureBar")))
                .map(|agent| {
                    std::process::Command::new(agent)
                        .arg("--request-mic")
                        .spawn()
                        .is_ok()
                })
                .unwrap_or(false);
            self.message = if spawned {
                "approve the Microphone prompt…".into()
            } else {
                "could not start the mic request".into()
            };
        }
        #[cfg(target_os = "windows")]
        {
            // Windows has no per-app mic prompt to trigger programmatically; point the
            // user at Settings → Privacy → Microphone.
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "", "ms-settings:privacy-microphone"])
                .spawn();
            self.message = "allow microphone access in Settings → Privacy → Microphone".into();
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            self.message = "grant microphone access in your OS privacy settings".into();
        }
        cx.notify();
    }

    /// Open the OS privacy settings for `pane` (grant OR revoke happens there — apps can't
    /// toggle the right themselves). macOS deep-links the Security pane; Windows opens the
    /// matching `ms-settings:` page.
    pub(crate) fn open_privacy_settings(&mut self, pane: &'static str, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(format!(
                    "x-apple.systempreferences:com.apple.preference.security?{pane}"
                ))
                .spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let uri = if pane.to_lowercase().contains("microphone") {
                "ms-settings:privacy-microphone"
            } else {
                "ms-settings:privacy"
            };
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "", uri])
                .spawn();
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = pane;
        }
        self.message = "opened Settings — adjust the permission there".into();
        cx.notify();
    }

    /// Restart the bundled daemon so a just-granted Screen Recording right takes
    /// effect: ask it to shut down — the menu-bar agent respawns it automatically.
    pub(crate) fn restart_daemon(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = "restarting daemon… (the agent respawns it)".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    let _ = d.shutdown();
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = "daemon restarting — reconnecting…".into();
                cx.notify();
            });
        })
        .detach();
    }
}
