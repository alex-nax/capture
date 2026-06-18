//! The GPUI view: a daemon dashboard + a live session detail pane.
//!
//! Lists (health, window picker, sessions) are polled over /v1; the selected
//! session's transcript + screenshot preview are fed LIVE by a background SSE
//! reader on /v1/events into a shared `LiveState` that render() reads. #33 slice 2.
//!
//! #68 refactor: the per-screen render bodies live in `screens/`, the reusable widgets in
//! `components/`, the action methods in `domain/`, and the state TYPES + helpers in `state`.
//! `app.rs` keeps the struct, `new()`, the poll/SSE plumbing, and `Render::render` (the shell
//! + dispatch to `render_dashboard`/`render_settings`/`render_playback` + the overlays).

use std::collections::HashSet;
use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{
    div, point, prelude::*, px, rgb, Context, FocusHandle, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, ScrollHandle, SharedString, Timer, Window,
};
use muda::MenuEvent;

use crate::daemon::{self, Daemon, Health, Session, WindowInfo};
use crate::tray::{self, Tray};
use crate::hotkey;
use crate::skill;
use crate::update;

use crate::components::icon;
use crate::theme;
use crate::state::{load_settings, short_id, ConfirmKind, IndexCfg, LiveState, PlaybackState};

/// macOS Screen Recording prompt — triggered from THIS GUI process, which is a real
/// app with a window-server connection. The headless daemon must NOT call this
/// (`CGRequestScreenCaptureAccess` aborts a process without window-server access).
#[cfg(target_os = "macos")]
pub(crate) mod screen_perm {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    /// Show the prompt (first time); returns whether access is already granted.
    pub(crate) fn request() -> bool {
        unsafe { CGRequestScreenCaptureAccess() }
    }
}

pub struct CaptureApp {
    pub(crate) daemon: Option<Daemon>,
    pub(crate) health: Option<Health>,
    pub(crate) sessions: Vec<Session>,
    pub(crate) windows: Vec<WindowInfo>,
    pub(crate) checked: HashSet<i64>,             // window picker: window_ids checked (multi-select)
    pub(crate) mic_app: Option<String>,           // app_name to attach the mic to (one app only)
    pub(crate) mic_device: Option<String>,        // mic input-device id (None = no mic)
    pub(crate) mics: Vec<daemon::AudioDevice>,    // available input devices (polled)
    pub(crate) mics_loaded: bool,                 // fetched the device list at least once
    pub(crate) selected_session: Option<String>,  // session tracked for live SSE (transcript/shot)
    pub(crate) confirm: Option<ConfirmKind>,      // a destructive action awaiting confirmation
    pub(crate) retranscribing: Option<String>,    // session id currently re-transcribing (SSE-tracked)
    pub(crate) playback: Option<PlaybackState>,   // Some => the playback screen is open
    pub(crate) pb_dragging: bool,                 // scrubber thumb is grabbed
    pub(crate) pb_ticker: bool,                   // an auto-play ticker is running
    pub(crate) live: Arc<Mutex<LiveState>>,
    pub(crate) tray: Option<Tray>,
    pub(crate) _hotkey_mgr: Option<GlobalHotKeyManager>, // kept alive = stays registered
    pub(crate) hotkey_id: u32,
    pub(crate) skill_status: Vec<skill::SkillStatus>, // per skill::AGENTS, cached
    pub(crate) asr: daemon::AsrModels,                // Whisper model catalog, polled
    pub(crate) asr_switching: Option<String>,         // repo being made active (unload+load takes time) → "switching…" row state
    pub(crate) runtimes: daemon::AsrRuntimes,         // ASR runtime registry (install/select), polled
    pub(crate) perms: daemon::Permissions,            // macOS TCC status, polled
    pub(crate) cmd_input: String,                     // "launch a command/URL" field buffer
    pub(crate) cmd_focus: FocusHandle,                // focus for the command field
    pub(crate) root_scroll: ScrollHandle,             // the single page scroll (drives the scrollbar)
    pub(crate) preset_scroll: ScrollHandle,           // the preset-picker card scroll (caps to the viewport)
    pub(crate) sb_drag: Option<(Pixels, Pixels)>,     // scrollbar drag: (mouse-down y, offset at down)
    pub(crate) show_settings: bool,                   // Settings screen vs. the capture dashboard
    pub(crate) settings_section: crate::state::SettingsSection, // active Settings left-nav section (#71)
    pub(crate) show_preset_picker: bool,              // the start-capture preset popup is open
    pub(crate) shot_format: String,                   // "png" | "jpeg" — applied to new captures
    pub(crate) shot_res_ix: usize,                    // index into RES_PRESETS (0 = native)
    pub(crate) jpeg_quality: u32,                     // 1..100, only for jpeg
    pub(crate) capture_screenshots: bool,             // off => audio-only capture (no screenshots)
    pub(crate) update_info: Option<update::UpdateInfo>, // a newer GitHub release than this build (#48), if any
    pub(crate) updating: bool,                        // an update download/install is in flight
    pub(crate) update_progress: Option<(u64, u64)>,   // (downloaded, total) bytes while the update DMG/exe streams (#48); None = idle
    pub(crate) asr_language: String,                  // transcription language filter buffer (#45; searchable dropdown)
    pub(crate) asr_language_focus: FocusHandle,       // focus for the language field
    pub(crate) lang_dropdown_open: bool,              // the searchable language dropdown is expanded
    pub(crate) index_provider: String,                // index endpoint provider id ("lmstudio"|"ollama"|"openai"|"custom") (#52)
    pub(crate) index_host: String,                    // index endpoint host (or full base URL for the "custom" provider)
    pub(crate) index_port: String,                    // index endpoint port (string for easy editing; blank for "custom")
    pub(crate) index_key: String,                     // index endpoint API key (only used when the provider needs one)
    pub(crate) index_model: String,                   // chosen model id (e.g. "qwen/qwen3.5-9b"); blank = server default
    pub(crate) index_models: Vec<String>,             // models the current provider exposes (#53; fetched, drives the dropdown)
    pub(crate) index_sample_rate: f64,                // leaf sampling rate for indexing (caption every round(1/rate)-th frame)
    pub(crate) index_preset: String,                  // index prompt preset: "general" | "meeting" | "lecture"
    pub(crate) index_host_focus: FocusHandle,         // focus for the host / base-URL field
    pub(crate) index_port_focus: FocusHandle,         // focus for the port field
    pub(crate) index_key_focus: FocusHandle,          // focus for the API-key field
    pub(crate) model_dropdown_open: bool,             // the model dropdown is expanded (#53)
    pub(crate) index_status: daemon::IndexStatus,     // polled: is the index endpoint configured + reachable
    pub(crate) indexing: HashSet<String>,             // session ids with an index build in flight (SSE-tracked)
    pub(crate) message: SharedString,
    pub(crate) out_dir: String,
    pub(crate) polling: bool,
}

impl CaptureApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Under the native menu-bar agent (CAPTURE_AGENT=1) the agent owns the tray
        // AND the daemon lifecycle, so this process is just the window — don't build
        // a second tray and don't spawn the daemon. Standalone (dev) keeps both.
        let under_agent = std::env::var_os("CAPTURE_AGENT").is_some();
        if !under_agent {
            // Packaged app: if no daemon is running and we ship one (bundled in the
            // .app), start it detached — the poll loop picks it up within ~1-2s.
            let running = daemon::discover().map_or(false, |d| d.available());
            if !running {
                if let Some(bin) = daemon::bundled_daemon() {
                    daemon::spawn_detached(&bin);
                }
            }
        }
        let (shot_format, shot_res_ix, jpeg_quality, mic_device, capture_screenshots, index_cfg, index_model, index_sample_rate, index_preset) =
            load_settings();
        let IndexCfg {
            provider: index_provider,
            host: index_host,
            port: index_port,
            key: index_key,
        } = index_cfg;
        let mut app = Self {
            daemon: daemon::discover(),
            health: None,
            sessions: Vec::new(),
            windows: Vec::new(),
            checked: HashSet::new(),
            mic_app: None,
            mic_device,
            mics: Vec::new(),
            mics_loaded: false,
            selected_session: None,
            confirm: None,
            retranscribing: None,
            playback: None,
            pb_dragging: false,
            pb_ticker: false,
            live: Arc::new(Mutex::new(LiveState::default())),
            tray: if under_agent { None } else { tray::build() },
            _hotkey_mgr: None,
            hotkey_id: 0,
            skill_status: Vec::new(),
            asr: daemon::AsrModels::default(),
            asr_switching: None,
            runtimes: daemon::AsrRuntimes::default(),
            perms: daemon::Permissions::default(),
            cmd_input: String::new(),
            cmd_focus: cx.focus_handle(),
            root_scroll: ScrollHandle::new(),
            preset_scroll: ScrollHandle::new(),
            sb_drag: None,
            show_settings: false,
            settings_section: crate::state::SettingsSection::CaptureQuality,
            show_preset_picker: false,
            shot_format,
            shot_res_ix,
            jpeg_quality,
            capture_screenshots,
            update_info: None,
            updating: false,
            update_progress: None,
            asr_language: String::new(),
            asr_language_focus: cx.focus_handle(),
            lang_dropdown_open: false,
            index_provider,
            index_host,
            index_port,
            index_key,
            index_model,
            index_models: Vec::new(),
            index_sample_rate,
            index_preset,
            index_host_focus: cx.focus_handle(),
            index_port_focus: cx.focus_handle(),
            index_key_focus: cx.focus_handle(),
            model_dropdown_open: false,
            index_status: daemon::IndexStatus::default(),
            indexing: HashSet::new(),
            message: "".into(),
            out_dir: crate::state::default_out_dir(),
            polling: false,
        };
        if let Some((mgr, id)) = hotkey::build() {
            app._hotkey_mgr = Some(mgr);
            app.hotkey_id = id;
        }
        app.refresh_skill_status();
        app.refresh_blocking();
        app.check_for_update(cx);
        app.start_poll(cx);
        app.fetch_index_models(cx);
        app.start_index_status_poll(cx);
        app.spawn_sse();
        app.start_tray_loop(cx);
        app
    }

    /// Drain menu-bar events (~4×/s) and keep the menu-bar title in sync with the
    /// running-capture count. Runs on the GPUI main thread (tray UI is main-thread).
    fn start_tray_loop(&mut self, cx: &mut Context<Self>) {
        if self.tray.is_none() && self.hotkey_id == 0 {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_millis(250)).await;
            // Menu-bar clicks.
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                let id = ev.id().as_ref().to_string();
                if this.update(cx, |v, cx| v.on_menu(&id, cx)).is_err() {
                    return;
                }
            }
            // Global hotkey (⌃⌘R) — toggle on key-down.
            while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                if ev.state == HotKeyState::Pressed {
                    if this
                        .update(cx, |v, cx| {
                            if ev.id == v.hotkey_id {
                                v.toggle_capture(cx);
                            }
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
            // Keep the menu-bar title synced to the running count.
            if this
                .update(cx, |v, _cx| {
                    let n = v.sessions.iter().filter(|s| s.state == "running").count();
                    if let Some(t) = v.tray.as_mut() {
                        t.set_running(n);
                    }
                })
                .is_err()
            {
                return;
            }
        })
        .detach();
    }

    fn on_menu(&mut self, id: &str, cx: &mut Context<Self>) {
        match id {
            tray::ID_STOP_ALL => self.stop_all(cx),
            tray::ID_OPEN => cx.activate(true),
            tray::ID_QUIT => std::process::exit(0),
            _ => {}
        }
    }

    fn refresh_blocking(&mut self) {
        if let Some(d) = &self.daemon {
            self.health = d.health().ok();
            self.sessions = d.sessions().unwrap_or_default();
            if self.windows.is_empty() {
                self.windows = d.windows().unwrap_or_default();
            }
            self.asr = d.asr_models().unwrap_or_default();
            self.runtimes = d.asr_runtimes().unwrap_or_default();
            self.perms = d.permissions().unwrap_or_default();
        }
    }

    /// Background thread: read /v1/events forever and accumulate the tracked
    /// session's transcript + latest screenshot into the shared LiveState.
    fn spawn_sse(&self) {
        let live = self.live.clone();
        std::thread::spawn(move || loop {
            // Re-discover each reconnect so it attaches to whatever daemon is
            // running now (incl. the bundled one started after the GUI launched).
            if let Some(daemon) = daemon::discover() {
                if let Ok(reader) = daemon.open_events() {
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let Some(json) = line.strip_prefix("data: ") else { continue };
                    let Ok(ev) = serde_json::from_str::<serde_json::Value>(json) else { continue };
                    let ev_type = ev.get("type").and_then(|v| v.as_str());
                    let mut st = live.lock().unwrap();
                    // ASR model downloads are daemon-wide (no session_id) — handle
                    // them before the session filter, which would drop them.
                    match ev_type {
                        Some("asr_download") => {
                            if let (Some(repo), Some(frac)) = (
                                ev.get("repo").and_then(|v| v.as_str()),
                                ev.get("fraction").and_then(|v| v.as_f64()),
                            ) {
                                st.asr_progress.insert(repo.to_string(), frac as f32);
                            }
                            continue;
                        }
                        Some("asr_download_done") | Some("asr_download_error") => {
                            if let Some(repo) = ev.get("repo").and_then(|v| v.as_str()) {
                                st.asr_progress.remove(repo);
                            }
                            continue;
                        }
                        // ASR runtime-pack installs are daemon-wide (no session_id).
                        Some("asr_runtime_install") => {
                            if let (Some(id), Some(frac)) = (
                                ev.get("id").and_then(|v| v.as_str()),
                                ev.get("fraction").and_then(|v| v.as_f64()),
                            ) {
                                st.runtime_install.insert(id.to_string(), frac as f32);
                            }
                            continue;
                        }
                        Some("asr_runtime_install_done") | Some("asr_runtime_install_error") => {
                            if let Some(id) = ev.get("id").and_then(|v| v.as_str()) {
                                st.runtime_install.remove(id);
                            }
                            continue;
                        }
                        // Re-transcribe is session-keyed but daemon-wide (no tracked filter).
                        Some("retranscribe") => {
                            if let (Some(sid), Some(frac)) = (
                                ev.get("session_id").and_then(|v| v.as_str()),
                                ev.get("fraction").and_then(|v| v.as_f64()),
                            ) {
                                st.retranscribe.insert(sid.to_string(), frac as f32);
                            }
                            continue;
                        }
                        Some("retranscribe_done") | Some("retranscribe_error") => {
                            if let Some(sid) = ev.get("session_id").and_then(|v| v.as_str()) {
                                st.retranscribe.remove(sid);
                                st.retranscribe_done.push(sid.to_string());
                            }
                            continue;
                        }
                        // Import is daemon-wide (no session_id until it finishes).
                        Some("import") => {
                            let phase = ev.get("phase").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let frac = ev.get("fraction").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            st.import_progress = Some((phase, frac));
                            continue;
                        }
                        Some("import_done") => {
                            st.import_progress = None;
                            if let Some(sid) = ev.get("session_id").and_then(|v| v.as_str()) {
                                st.import_result = Some(Ok(sid.to_string()));
                            }
                            continue;
                        }
                        Some("import_error") => {
                            st.import_progress = None;
                            let msg = ev.get("error").and_then(|v| v.as_str()).unwrap_or("import failed");
                            st.import_result = Some(Err(msg.to_string()));
                            continue;
                        }
                        // Index build is session-keyed but daemon-wide (no tracked filter).
                        Some("index") => {
                            if let Some(sid) = ev.get("session_id").and_then(|v| v.as_str()) {
                                let phase = ev.get("phase").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let frac = ev.get("fraction").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                                st.index_progress.insert(sid.to_string(), (phase, frac));
                            }
                            continue;
                        }
                        Some("index_done") => {
                            if let Some(sid) = ev.get("session_id").and_then(|v| v.as_str()) {
                                st.index_progress.remove(sid);
                                st.index_done.push((sid.to_string(), None));
                            }
                            continue;
                        }
                        Some("index_error") => {
                            if let Some(sid) = ev.get("session_id").and_then(|v| v.as_str()) {
                                st.index_progress.remove(sid);
                                let err = ev.get("error").and_then(|v| v.as_str()).unwrap_or("index failed").to_string();
                                st.index_done.push((sid.to_string(), Some(err)));
                            }
                            continue;
                        }
                        _ => {}
                    }
                    let sid = ev.get("session_id").and_then(|v| v.as_str());
                    if st.tracked.is_none() || st.tracked.as_deref() != sid {
                        continue;
                    }
                    match ev_type {
                        Some("transcript_segment") => {
                            if let Some(t) = ev.get("text").and_then(|v| v.as_str()) {
                                st.transcript.push(t.trim().to_string());
                            }
                        }
                        Some("screenshot_taken") => {
                            if let Some(p) = ev.get("path").and_then(|v| v.as_str()) {
                                st.last_shot = Some(p.to_string());
                            }
                        }
                        _ => {}
                    }
                }
                }
            }
            std::thread::sleep(Duration::from_secs(1)); // reconnect backoff
        });
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_millis(1000)).await;
            // Re-discover each tick so a daemon that starts later (incl. the
            // bundled one we spawned) is picked up, and a restarted daemon too.
            let result = cx
                .background_executor()
                .spawn(async move {
                    match daemon::discover() {
                        Some(d) if d.available() => {
                            let h = d.health().ok();
                            let s = d.sessions().unwrap_or_default();
                            let a = d.asr_models().unwrap_or_default();
                            let p = d.permissions().unwrap_or_default();
                            (Some(d), h, s, a, p)
                        }
                        _ => (
                            None,
                            None,
                            Vec::new(),
                            daemon::AsrModels::default(),
                            daemon::Permissions::default(),
                        ),
                    }
                })
                .await;
            if this
                .update(cx, |v, cx| {
                    v.daemon = result.0;
                    v.health = result.1;
                    v.sessions = result.2;
                    v.asr = result.3;
                    v.perms = result.4;
                    // Drop the "switching…" model state once the target is actually active
                    // (covers the case the in-flight set_active_model task hasn't cleared it).
                    if let Some(sw) = v.asr_switching.clone() {
                        if v.asr.models.iter().any(|m| m.repo == sw && m.active) {
                            v.asr_switching = None;
                        }
                    }
                    // Fetch the mic list once, the first time a daemon is available.
                    if !v.mics_loaded && v.daemon.is_some() {
                        v.refresh_mics(cx);
                    }
                    // A re-transcribe finished (SSE): clear the flag + reload the open session
                    // so its fresh transcript shows.
                    let done: Vec<String> = std::mem::take(&mut v.live.lock().unwrap().retranscribe_done);
                    for sid in done {
                        if v.retranscribing.as_deref() == Some(sid.as_str()) {
                            v.retranscribing = None;
                            v.message = format!("re-transcribed {}", short_id(&sid)).into();
                        }
                        if v.playback.as_ref().map(|p| p.sid.as_str()) == Some(sid.as_str()) {
                            v.select_session(sid.clone(), cx);
                        }
                    }
                    // An import finished (SSE): surface the result; the new session is
                    // already in v.sessions from this same poll.
                    let import_result = v.live.lock().unwrap().import_result.take();
                    match import_result {
                        Some(Ok(sid)) => {
                            v.message = format!("imported session {}", short_id(&sid)).into();
                            v.select_session(sid, cx);
                        }
                        Some(Err(e)) => v.message = format!("import failed: {e}").into(),
                        None => {}
                    }
                    // An index build finished (SSE): show success or the real error, clear the
                    // flag, and reload the open session so a fresh index shows.
                    let idx_done: Vec<(String, Option<String>)> =
                        std::mem::take(&mut v.live.lock().unwrap().index_done);
                    for (sid, err) in idx_done {
                        if v.indexing.remove(&sid) {
                            v.message = match &err {
                                None => format!("indexed {}", short_id(&sid)).into(),
                                Some(e) => format!("index failed: {e}").into(),
                            };
                        }
                        // Only reload on success; a failed build wrote nothing.
                        if err.is_none()
                            && v.playback.as_ref().map(|p| p.sid.as_str()) == Some(sid.as_str())
                        {
                            v.select_session(sid.clone(), cx);
                        }
                    }
                    // Default the live pane to the newest running capture.
                    if v.selected_session.is_none() {
                        if let Some(s) = v.sessions.iter().rev().find(|s| s.state == "running") {
                            let id = s.session_id.clone();
                            v.select_session(id, cx);
                        }
                    }
                    cx.notify(); // also repaints the live SSE-fed detail pane
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    pub(crate) fn refresh_windows(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        cx.spawn(async move |this, cx| {
            let ws = cx
                .background_executor()
                .spawn(async move { d.windows().unwrap_or_default() })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.windows = ws;
                cx.notify();
            });
        })
        .detach();
        self.refresh_mics(cx);
    }

    /// Fetch the mic device list (spawns the helper `--list-mics` on the daemon, so
    /// fetch sparingly — once when a daemon appears, and on "Refresh windows").
    fn refresh_mics(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.mics_loaded = true;
        cx.spawn(async move |this, cx| {
            let ms = cx
                .background_executor()
                .spawn(async move { d.audio_mics().unwrap_or_default().devices })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.mics = ms;
                cx.notify();
            });
        })
        .detach();
    }

    /// Select a session: track it for SSE, clear the pane, and backfill the
    /// existing transcript over REST (SSE then appends new segments).
    pub(crate) fn select_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected_session = Some(id.clone());
        {
            let mut st = self.live.lock().unwrap();
            st.tracked = Some(id.clone());
            st.transcript.clear();
            st.last_shot = None;
        }
        // Open the full Playback screen for this session.
        let (dir, finished) = self
            .sessions
            .iter()
            .find(|s| s.session_id == id)
            .map(|s| (s.dir.clone(), !matches!(s.state.as_str(), "running" | "starting" | "stopping")))
            .unwrap_or_default();
        self.playback = Some(PlaybackState {
            sid: id.clone(),
            finished,
            ..Default::default()
        });
        if finished && !dir.is_empty() {
            // Load frames + subtitles off the main thread, then fill the timeline.
            let want = id.clone();
            cx.spawn(async move |this, cx| {
                let data = cx
                    .background_executor()
                    .spawn(async move { crate::state::load_playback_data(&dir) })
                    .await;
                let _ = this.update(cx, |v, cx| {
                    if let Some(pb) = v.playback.as_mut() {
                        if pb.sid == want {
                            let (frames, subs, t0, t1) = data;
                            pb.frames = frames;
                            pb.subs = subs;
                            pb.t0 = t0;
                            pb.t1 = t1;
                            pb.pos = t0;
                            pb.loaded = true;
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        let Some(d) = self.daemon.clone() else {
            cx.notify();
            return;
        };
        // Load the built index (if any) so the Manage panel can show its root summary.
        if finished {
            let d_idx = d.clone();
            let want = id.clone();
            cx.spawn(async move |this, cx| {
                let idx = cx
                    .background_executor()
                    .spawn(async move { d_idx.get_index(&want).ok() })
                    .await;
                let _ = this.update(cx, |v, cx| {
                    if let (Some(pb), Some(idx)) = (v.playback.as_mut(), idx) {
                        pb.index_summary = idx
                            .get("root_summary")
                            .and_then(|s| s.as_str())
                            .filter(|s| !s.is_empty())
                            .map(String::from);
                        pb.index_nodes = idx.get("node_count").and_then(|n| n.as_u64()).map(|n| n as usize);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        let live = self.live.clone();
        cx.spawn(async move |_this, cx| {
            let id2 = id.clone();
            let segs = cx
                .background_executor()
                .spawn(async move { d.transcript(&id2, 200).unwrap_or_default() })
                .await;
            let mut st = live.lock().unwrap();
            if st.tracked.as_deref() == Some(id.as_str()) {
                st.transcript = segs.into_iter().map(|s| s.text.trim().to_string()).collect();
            }
        })
        .detach();
        cx.notify();
    }

    /// The screenshot quality fields (from the Settings panel) to merge into a
    /// `/v1/sessions` body: format, optional resolution, jpeg quality (jpeg only).
    pub(crate) fn shot_settings(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("capture_screenshots".into(), serde_json::json!(self.capture_screenshots));
        m.insert("screenshot_format".into(), serde_json::json!(self.shot_format));
        if let Some(res) = crate::state::RES_PRESETS.get(self.shot_res_ix).and_then(|p| p.1) {
            m.insert("screenshot_resolution".into(), serde_json::json!(res));
        }
        if self.shot_format == "jpeg" {
            m.insert("screenshot_jpeg_quality".into(), serde_json::json!(self.jpeg_quality));
        }
        serde_json::Value::Object(m)
    }

    /// Persist the capture-quality prefs so they survive a GUI relaunch. Called from
    /// each quality control's on-click (best-effort; a write failure is silent).
    pub(crate) fn save_settings(&self) {
        let Some(p) = crate::state::settings_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let v = serde_json::json!({
            "shot_format": self.shot_format,
            "shot_res_ix": self.shot_res_ix,
            "jpeg_quality": self.jpeg_quality,
            "mic_device": self.mic_device,
            "capture_screenshots": self.capture_screenshots,
            "index_provider": self.index_provider,
            "index_host": self.index_host,
            "index_port": self.index_port,
            "index_key": self.index_key,
            "index_model": self.index_model,
            "index_sample_rate": self.index_sample_rate,
            "index_preset": self.index_preset,
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&v) {
            let _ = std::fs::write(&p, bytes);
        }
    }

    /// Check GitHub for a newer release (once, at startup), off the UI thread (#48).
    fn check_for_update(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let info = cx.background_executor().spawn(async move { update::check() }).await;
            if let Some(info) = info {
                let _ = this.update(cx, |v, cx| {
                    v.update_info = Some(info);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Download + install a confirmed update; the detached updater quits + relaunches the app.
    pub(crate) fn start_update(&mut self, info: update::UpdateInfo, cx: &mut Context<Self>) {
        self.updating = true;
        self.update_progress = Some((0, 0));
        self.message = format!("downloading update v{}…", info.version).into();
        cx.notify();
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;
        // The download is blocking, so it runs on the background executor while it bumps these shared
        // atomics; a 150ms ticker mirrors them into `update_progress` and breaks on the done flag.
        let dl = Arc::new(AtomicU64::new(0));
        let tot = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let (dl2, tot2, done2) = (dl.clone(), tot.clone(), done.clone());
        cx.spawn(async move |this, cx| {
            let task = cx.background_executor().spawn(async move {
                let res = update::download_and_install(&info, move |d, t| {
                    dl2.store(d, Ordering::Relaxed);
                    tot2.store(t, Ordering::Relaxed);
                });
                done2.store(true, Ordering::Relaxed);
                res
            });
            loop {
                Timer::after(Duration::from_millis(150)).await;
                let (d, t) = (dl.load(Ordering::Relaxed), tot.load(Ordering::Relaxed));
                let _ = this.update(cx, |v, cx| {
                    v.update_progress = Some((d, t));
                    cx.notify();
                });
                if done.load(Ordering::Relaxed) {
                    break;
                }
            }
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.update_progress = None;
                if let Err(e) = r {
                    v.updating = false;
                    v.message = format!("update failed: {e}").into();
                }
                // On success the updater script quits this app shortly; nothing more to do.
                cx.notify();
            });
        })
        .detach();
    }

    /// Build the page scrollbar thumb from the root ScrollHandle's prior-frame
    /// metrics; `None` when the content fits. Dragging is `on_scrollbar_drag`.
    fn scrollbar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let vp = self.root_scroll.bounds().size.height;
        let maxv = self.root_scroll.max_offset().height;
        if maxv <= px(1.0) || vp <= px(0.0) {
            return None;
        }
        let mut thumb_h = vp * (vp / (vp + maxv));
        if thumb_h < px(28.0) {
            thumb_h = px(28.0);
        }
        let scrolled = px(0.0) - self.root_scroll.offset().y;
        let frac = (scrolled / maxv).clamp(0.0, 1.0);
        let thumb_top = (vp - thumb_h) * frac;
        let dragging = self.sb_drag.is_some();
        Some(
            div().absolute().top_0().right_0().w(px(12.0)).h(vp).child(
                div()
                    .id("sb-thumb")
                    .absolute()
                    .top(thumb_top)
                    .right(px(2.0))
                    .w(px(7.0))
                    .h(thumb_h)
                    .rounded_full()
                    .bg(if dragging { rgb(theme::BORDER_STRONG) } else { rgb(theme::BORDER) })
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                            let off = px(0.0) - this.root_scroll.offset().y;
                            this.sb_drag = Some((ev.position.y, off));
                            cx.notify();
                        }),
                    ),
            ),
        )
    }

    /// While the scrollbar thumb is grabbed, map mouse Y → a scroll offset.
    fn on_scrollbar_drag(&mut self, ev: &MouseMoveEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some((y0, off0)) = self.sb_drag else { return };
        let vp = self.root_scroll.bounds().size.height;
        let maxv = self.root_scroll.max_offset().height;
        if maxv <= px(0.0) || vp <= px(0.0) {
            return;
        }
        let mut thumb_h = vp * (vp / (vp + maxv));
        if thumb_h < px(28.0) {
            thumb_h = px(28.0);
        }
        let track = vp - thumb_h;
        if track <= px(0.0) {
            return;
        }
        let dy = ev.position.y - y0;
        let mut new_off = off0 + dy * (maxv / track);
        if new_off < px(0.0) {
            new_off = px(0.0);
        }
        if new_off > maxv {
            new_off = maxv;
        }
        self.root_scroll.set_offset(point(px(0.0), px(0.0) - new_off));
        cx.notify();
    }
}

impl Render for CaptureApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = match &self.health {
            Some(h) if h.ok => {
                format!("daemon v{} (api {}) · pid {}", h.version, h.api_version, h.pid)
            }
            _ => "no daemon — run: capture daemon start".to_string(),
        };
        let hotkey_hint = if self.hotkey_id != 0 {
            format!("{} toggles capture from anywhere", hotkey::LABEL)
        } else {
            String::new()
        };

        let scrollbar = self.scrollbar(cx);
        let settings = self.show_settings;
        // Three top-level screens: dashboard (default), settings, and the session
        // playback screen. Only one renders at a time.
        let playback = self.playback.is_some();
        let sett = settings && !playback;
        let dash = !settings && !playback;

        // Build each screen's children up front (the prep + the rows live in the screen
        // modules now). Only the active screen's children are emitted below.
        let dashboard_children = if dash { self.render_dashboard(window, cx) } else { Vec::new() };
        let settings_children = if sett { self.render_settings(window, cx) } else { Vec::new() };
        let overlays = self.render_overlays(cx);

        div()
            .relative()
            .size_full()
            .bg(rgb(theme::BG))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                this.on_scrollbar_drag(ev, window, cx);
                if this.pb_dragging {
                    this.pb_seek_x(ev.position.x, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev, _w, cx| {
                    let mut changed = this.sb_drag.take().is_some();
                    if this.pb_dragging {
                        this.pb_dragging = false;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }),
            )
            .child(if sett {
                // The Settings screen owns the whole window: a flush left-nav + content
                // two-pane (it renders its own brand, daemon line, status, and Back). No
                // shared "capture" header bar, no outer padding/scroll — the content pane
                // scrolls itself and the nav reaches the window edges.
                div().size_full().children(settings_children).into_any_element()
            } else {
                div()
                    .id("root")
                    .track_scroll(&self.root_scroll)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .size_full()
                    .overflow_y_scroll() // single page scroll; the scrollbar overlay drives it
                    .text_color(rgb(theme::TEXT_PRIMARY))
                    .text_sm()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_xl().child("capture"))
                            .child({
                                // Back from playback, else open Settings.
                                let in_sub = playback;
                                div()
                                    .id("hdr-btn")
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(theme::ACCENT))
                                    .child(icon(
                                        if in_sub { "chevron-left" } else { "settings" },
                                        14.0,
                                        theme::ON_ACCENT,
                                    ))
                                    .child(if in_sub { "Back" } else { "Settings" })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.playback.take().is_none() {
                                            this.show_settings = !this.show_settings;
                                        }
                                        cx.notify();
                                    }))
                            }),
                    )
                    .child(div().text_color(rgb(theme::TEXT_MUTED)).child(header))
                    .child(div().text_color(rgb(theme::ACCENT_TEXT)).child(hotkey_hint))
                    .child(div().text_color(rgb(theme::WARNING)).child(self.message.clone()))
                    .children(dashboard_children)
                    .children(playback.then(|| self.render_playback(window, cx)))
                    .into_any_element()
            })
            .children(if sett { None } else { scrollbar })
            // Overlays: the confirmation modal + the start-capture preset picker.
            .children(overlays)
    }
}
