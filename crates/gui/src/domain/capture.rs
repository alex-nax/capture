//! Capture lifecycle actions: start/stop, mic switching, preset picker, launch.
//! Relocated verbatim from `app.rs` (#68).

use std::collections::HashSet;

use gpui::{Context, KeyDownEvent, Window};

use crate::app::CaptureApp;
use crate::state::short_id;

impl CaptureApp {
    pub(crate) fn toggle_capture(&mut self, cx: &mut Context<Self>) {
        if self.sessions.iter().any(|s| s.state == "running") {
            self.stop_all(cx);
        } else {
            self.open_preset_picker(cx);
        }
    }

    pub(crate) fn stop_all(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        let ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| s.state == "running")
            .map(|s| s.session_id.clone())
            .collect();
        if ids.is_empty() {
            self.message = "no running captures".into();
            cx.notify();
            return;
        }
        self.message = format!("stopping {} capture(s)…", ids.len()).into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    for id in &ids {
                        let _ = d.stop(id);
                    }
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = "stopped all captures".into();
                cx.notify();
            });
        })
        .detach();
    }

    /// Open the preset picker — the dashboard "Start capture" entry point. Picking a
    /// preset (or hitting a hotkey path via meeting-default) runs `start_with_preset`.
    pub(crate) fn open_preset_picker(&mut self, cx: &mut Context<Self>) {
        self.show_preset_picker = true;
        cx.notify();
    }

    /// Apply a preset's capture toggles to the GUI state, persist them, close the
    /// picker, then start the capture threading `preset` through to the daemon.
    /// Mapping (mirrors the backend contract):
    ///   meeting → screenshots on + mic on (defaults to the first input device if none);
    ///   coding/lecture → screenshots on, mic off;
    ///   auto/general/custom → screenshots on, mic left as-is.
    pub(crate) fn start_with_preset(&mut self, preset: &str, cx: &mut Context<Self>) {
        self.capture_screenshots = true;
        match preset {
            "meeting" => {
                if self.mic_device.is_none() {
                    // Pick the default input if known, else the first available device.
                    self.mic_device = self
                        .mics
                        .iter()
                        .find(|d| d.default)
                        .or_else(|| self.mics.first())
                        .map(|d| d.id.clone());
                }
            }
            "coding" | "lecture" => self.mic_device = None,
            _ => {} // auto / general / custom: leave the mic as the user set it
        }
        self.save_settings();
        self.show_preset_picker = false;
        self.start_capture(preset, cx);
    }

    pub(crate) fn start_capture(&mut self, preset: &str, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else {
            self.message = "no daemon — run: capture daemon start".into();
            return;
        };
        let preset = preset.to_string();
        // One session per CHECKED window, in picker order. Per app (pid): only the
        // first window records the app audio (macOS audio is per-app); the rest are
        // screenshots-only. The mic attaches to the first window of the chosen app.
        let out = self.out_dir.clone();
        let shot = self.shot_settings();
        let mic_device = self.mic_device.clone();
        let mut audio_pids: HashSet<i64> = HashSet::new();
        // The mic is one device (the user's voice — a separate track), so attach it to the FIRST
        // captured window only. Previously this was gated on a `mic_app` field that was never assigned
        // (always None), so the mic never turned on — e.g. the meeting preset selected a device but
        // started with the mic off.
        let mut mic_attached = false;
        let mut bodies: Vec<serde_json::Value> = Vec::new();
        for w in self.windows.iter().filter(|w| self.checked.contains(&w.window_id)) {
            let first_for_app = audio_pids.insert(w.pid); // true => first checked window of this pid
            let wants_mic = mic_device.is_some() && !mic_attached;
            let mut body = serde_json::json!({
                // window_id pins screenshots to the EXACT picked window (pid alone
                // can't disambiguate two windows of one process, e.g. Chrome).
                "output_dir": out, "pid": w.pid, "window_id": w.window_id,
                "audio_source": "app", "capture_audio": first_for_app,
                "screenshot_interval": 2.0,
            });
            if wants_mic {
                mic_attached = true;
                body["mic_device"] = serde_json::json!(mic_device);
            }
            if let Some(obj) = shot.as_object() {
                for (k, v) in obj {
                    body[k.as_str()] = v.clone();
                }
            }
            bodies.push(body);
        }
        if bodies.is_empty() {
            self.message = "check at least one window".into();
            cx.notify();
            return;
        }
        let n = bodies.len();
        // Live indexing (#84) keys off the daemon's `last_index`, set when GET /v1/index/status finds
        // the endpoint reachable. The 8s status poll is racy right after a daemon (re)start, so probe
        // it ourselves HERE, before starting, whenever an endpoint is configured — this guarantees
        // `last_index` is fresh so the capture indexes live instead of silently skipping it.
        let index_url = self.index_chat_url();
        let index_model = self.index_model.clone();
        self.message = format!("starting {n} capture(s)…").into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            if !index_url.trim().is_empty() {
                let d2 = d.clone();
                let (u, m) = (index_url.clone(), index_model.clone());
                let _ = cx.background_executor().spawn(async move { d2.index_status(&u, &m) }).await;
            }
            let mut ok = 0usize;
            let mut last_id: Option<String> = None;
            let mut err: Option<String> = None;
            for body in bodies {
                let d2 = d.clone();
                let preset = preset.clone();
                match cx
                    .background_executor()
                    .spawn(async move { d2.start(body, &preset) })
                    .await
                {
                    Ok(s) => {
                        ok += 1;
                        last_id = Some(s.session_id);
                    }
                    Err(e) => err = Some(e),
                }
            }
            let _ = this.update(cx, |v, cx| {
                if ok > 0 {
                    v.checked.clear();
                    v.message = format!("started {ok}/{n} capture(s)").into();
                    if let Some(id) = last_id {
                        v.select_session(id, cx); // open the live pane on the last one
                    }
                } else if let Some(e) = err {
                    v.message = format!("start failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn stop_capture(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = format!("stopping {}…", short_id(&id)).into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { d.stop(&id) })
                .await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(s) => {
                        let sid = s.session_id.clone();
                        v.message = format!("stopped {}", short_id(&sid)).into();
                        // Reflect the stop immediately (don't wait for the next poll) so the row flips.
                        if let Some(slot) = v.sessions.iter_mut().find(|x| x.session_id == sid) {
                            *slot = s;
                        } else {
                            v.sessions.insert(0, s);
                        }
                        // Stop pressed on the live playback screen → reload it as the saved capture so
                        // the scrubber + Manage appear in place (instead of a stale "live"/REC view).
                        if v.playback.as_ref().map(|p| p.sid.as_str()) == Some(sid.as_str()) {
                            v.select_session(sid, cx);
                        }
                    }
                    Err(e) => v.message = format!("stop failed: {e}").into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Switch the microphone on a running capture (#46). `device` = None turns it off.
    pub(crate) fn switch_mic(&mut self, sid: String, device: Option<String>, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = match &device {
            Some(_) => "switching microphone…".into(),
            None => "turning microphone off…".into(),
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { d.set_mic(&sid, device.as_deref()) })
                .await;
            let _ = this.update(cx, |v, cx| {
                if let Err(e) = r {
                    v.message = format!("mic switch failed: {e}").into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Key handling for the single-line "launch a command/URL" field. Minimal:
    /// printable chars (via `key_char`), backspace, ⌘V paste, Enter = launch.
    pub(crate) fn on_cmd_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        if m.platform && ks.key == "v" {
            if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
                self.cmd_input.push_str(t.trim());
                cx.notify();
            }
            return;
        }
        if m.platform || m.control || m.function {
            return; // ignore other shortcuts
        }
        match ks.key.as_str() {
            "backspace" => {
                self.cmd_input.pop();
            }
            "enter" => {
                self.launch_command(cx);
                return;
            }
            "space" => self.cmd_input.push(' '),
            _ => {
                if let Some(c) = ks.key_char.as_deref() {
                    if !c.is_empty() && !c.chars().any(char::is_control) {
                        self.cmd_input.push_str(c);
                    }
                }
            }
        }
        cx.notify();
    }

    /// Launch a command (or URL via e.g. `open https://…`) in capture's launch mode
    /// — the engine runs it and captures its window + stdout/stderr + audio.
    pub(crate) fn launch_command(&mut self, cx: &mut Context<Self>) {
        let cmd = self.cmd_input.trim().to_string();
        if cmd.is_empty() {
            return;
        }
        let Some(d) = self.daemon.clone() else {
            self.message = "no daemon".into();
            cx.notify();
            return;
        };
        let out = self.out_dir.clone();
        let shot = self.shot_settings();
        self.message = format!("launching: {cmd}…").into();
        self.cmd_input.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut body = serde_json::json!({
                "output_dir": out, "command": cmd,
                "audio_source": "app", "screenshot_interval": 2.0,
            });
            if let Some(obj) = shot.as_object() {
                for (k, v) in obj {
                    body[k.as_str()] = v.clone();
                }
            }
            let r = cx.background_executor().spawn(async move { d.start(body, "") }).await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(s) => {
                        v.message = format!("launched {}", short_id(&s.session_id)).into();
                        v.select_session(s.session_id, cx);
                    }
                    Err(e) => v.message = format!("launch failed: {e}").into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Import an existing audio/video file as a session: pick a file via the native
    /// macOS dialog (osascript), then hand the path to the daemon (extraction + ASR run
    /// in the background, progress over SSE; the poll loop surfaces the new session).
    pub(crate) fn import_file(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else {
            self.message = "no daemon".into();
            cx.notify();
            return;
        };
        self.message = "choose a file to import…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            // The picker blocks, so run it (and the request) off the UI thread.
            let r = cx
                .background_executor()
                .spawn(async move {
                    let path = crate::state::pick_media_file()?; // None => user cancelled
                    Some(d.import_media(&path).map(|_| path)) // Some(Ok(path)) | Some(Err(msg))
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Some(Ok(path)) => {
                        let name = path.rsplit('/').next().unwrap_or(&path);
                        v.message = format!("importing {name}…").into();
                    }
                    Some(Err(e)) => v.message = format!("import failed: {e}").into(),
                    None => v.message = "import cancelled".into(),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
