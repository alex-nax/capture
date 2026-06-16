//! The GPUI view: a daemon dashboard + a live session detail pane.
//!
//! Lists (health, window picker, sessions) are polled over /v1; the selected
//! session's transcript + screenshot preview are fed LIVE by a background SSE
//! reader on /v1/events into a shared `LiveState` that render() reads. #33 slice 2.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{
    div, img, point, prelude::*, px, relative, rgb, rgba, svg, App, ClickEvent, ClipboardItem,
    Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    ScrollHandle, SharedString, Timer, Window,
};
use muda::MenuEvent;

use crate::daemon::{self, Daemon, Health, Session, WindowInfo};
use crate::tray::{self, Tray};
use crate::hotkey;
use crate::skill;

/// macOS Screen Recording prompt — triggered from THIS GUI process, which is a real
/// app with a window-server connection. The headless daemon must NOT call this
/// (`CGRequestScreenCaptureAccess` aborts a process without window-server access).
#[cfg(target_os = "macos")]
mod screen_perm {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    /// Show the prompt (first time); returns whether access is already granted.
    pub fn request() -> bool {
        unsafe { CGRequestScreenCaptureAccess() }
    }
}

/// Live data written by the SSE thread, read by render: the selected session's
/// transcript + screenshot, plus in-flight ASR model download progress (repo →
/// fraction 0..1; entries are removed on done/error).
#[derive(Default)]
struct LiveState {
    tracked: Option<String>,
    transcript: Vec<String>,
    last_shot: Option<String>,
    asr_progress: HashMap<String, f32>,
}

/// The session "playback" screen state (loaded from the session's on-disk artifacts).
#[derive(Default)]
struct PlaybackState {
    sid: String,
    finished: bool,
    loaded: bool,                          // disk read finished (finished sessions only)
    frames: Vec<(f64, String)>,            // (epoch_secs, screenshot path), time-sorted
    subs: Vec<(f64, f64, String, bool)>,   // (start, end, text, is_mic), start-sorted
    pos: f64,                              // playhead, epoch seconds
    t0: f64,                               // timeline start (first frame/segment)
    t1: f64,                               // timeline end
    playing: bool,                         // auto-advancing
}

pub struct CaptureApp {
    daemon: Option<Daemon>,
    health: Option<Health>,
    sessions: Vec<Session>,
    windows: Vec<WindowInfo>,
    checked: HashSet<i64>,             // window picker: window_ids checked (multi-select)
    mic_app: Option<String>,           // app_name to attach the mic to (one app only)
    mic_device: Option<String>,        // mic input-device id (None = no mic)
    mics: Vec<daemon::AudioDevice>,    // available input devices (polled)
    mics_loaded: bool,                 // fetched the device list at least once
    selected_session: Option<String>,  // session tracked for live SSE (transcript/shot)
    confirm_delete: Option<String>,    // session id awaiting a delete confirmation
    playback: Option<PlaybackState>,   // Some => the playback screen is open
    pb_dragging: bool,                 // scrubber thumb is grabbed
    pb_ticker: bool,                   // an auto-play ticker is running
    live: Arc<Mutex<LiveState>>,
    tray: Option<Tray>,
    _hotkey_mgr: Option<GlobalHotKeyManager>, // kept alive = stays registered
    hotkey_id: u32,
    skill_status: Vec<skill::SkillStatus>, // per skill::AGENTS, cached
    asr: daemon::AsrModels,                // Whisper model catalog, polled
    perms: daemon::Permissions,            // macOS TCC status, polled
    cmd_input: String,                     // "launch a command/URL" field buffer
    cmd_focus: FocusHandle,                // focus for the command field
    root_scroll: ScrollHandle,             // the single page scroll (drives the scrollbar)
    sb_drag: Option<(Pixels, Pixels)>,     // scrollbar drag: (mouse-down y, offset at down)
    show_settings: bool,                   // Settings screen vs. the capture dashboard
    shot_format: String,                   // "png" | "jpeg" — applied to new captures
    shot_res_ix: usize,                    // index into RES_PRESETS (0 = native)
    jpeg_quality: u32,                     // 1..100, only for jpeg
    message: SharedString,
    out_dir: String,
    polling: bool,
}

/// Screenshot resolution presets for the Settings panel (label, "WxH" or None = native).
const RES_PRESETS: [(&str, Option<&str>); 4] = [
    ("Native", None),
    ("1440p", Some("2560x1440")),
    ("1080p", Some("1920x1080")),
    ("720p", Some("1280x720")),
];

fn default_out_dir() -> String {
    dirs::home_dir()
        .map(|h| h.join(".capture").join("runs").to_string_lossy().into_owned())
        .unwrap_or_else(|| "/tmp/capture-runs".into())
}

/// Where the GUI persists its capture-quality preferences (sibling of `daemon.json`).
fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".capture").join("gui-settings.json"))
}

/// `(shot_format, shot_res_ix, jpeg_quality)` loaded from `gui-settings.json`, or
/// the defaults if the file is missing/unreadable. So a chosen quality survives a
/// GUI relaunch (the settings live in the window process, not the daemon).
fn load_settings() -> (String, usize, u32, Option<String>) {
    let def = ("png".to_string(), 0usize, 80u32);
    let Some(v) = settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return (def.0, def.1, def.2, None);
    };
    (
        v.get("shot_format").and_then(|x| x.as_str()).unwrap_or(&def.0).to_string(),
        v.get("shot_res_ix").and_then(|x| x.as_u64()).map_or(def.1, |n| n as usize),
        v.get("jpeg_quality").and_then(|x| x.as_u64()).map_or(def.2, |n| n as u32),
        v.get("mic_device").and_then(|x| x.as_str()).map(String::from),
    )
}

fn short_id(id: &str) -> &str {
    id.rsplit('-').next().unwrap_or(id)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Seconds → `m:ss` for the playback time read-out.
fn fmt_dur(s: f64) -> String {
    let s = s.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Parse an ISO-8601 stamp to epoch seconds. Handles both transcript form
/// (`2026-06-16T11:00:34.937Z`) and the screenshot-filename form
/// (`2026-06-16T11-00-34.937Z`) — the separators sit at fixed indices, so we read
/// by position regardless of `:`/`-`. No `chrono` dep (civil-days arithmetic).
fn parse_iso_epoch(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let ms = if b.len() >= 23 && b[19] == b'.' {
        s.get(20..23).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0)
    } else {
        0
    };
    // days_from_civil (Howard Hinnant): civil date -> days since 1970-01-01.
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = (if y2 >= 0 { y2 } else { y2 - 399 }) / 400;
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days as f64 * 86400.0 + h as f64 * 3600.0 + mi as f64 * 60.0 + se as f64 + ms as f64 / 1000.0)
}

/// Read a finished session's playback data off disk: screenshots (time-sorted) +
/// transcript/mic-transcript segments, and the timeline bounds. Runs off the main thread.
fn load_playback_data(
    dir: &str,
) -> (Vec<(f64, String)>, Vec<(f64, f64, String, bool)>, f64, f64) {
    let base = std::path::Path::new(dir);
    let mut frames: Vec<(f64, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(base.join("screenshots")) {
        for e in rd.flatten() {
            let p = e.path();
            let is_img = p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                matches!(x, "png" | "jpg" | "jpeg" | "tiff" | "gif" | "bmp")
            });
            if !is_img {
                continue;
            }
            if let Some(t) = p.file_stem().and_then(|s| s.to_str()).and_then(parse_iso_epoch) {
                frames.push((t, p.to_string_lossy().into_owned()));
            }
        }
    }
    frames.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut subs: Vec<(f64, f64, String, bool)> = Vec::new();
    for (fname, is_mic) in [("transcript.jsonl", false), ("mic_transcript.jsonl", true)] {
        if let Ok(text) = std::fs::read_to_string(base.join(fname)) {
            for ln in text.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(ln) else { continue };
                let Some(start) = v.get("start").and_then(|x| x.as_str()).and_then(parse_iso_epoch)
                else {
                    continue;
                };
                let end = v
                    .get("end")
                    .and_then(|x| x.as_str())
                    .and_then(parse_iso_epoch)
                    .unwrap_or(start + 2.0);
                let txt = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
                if !txt.is_empty() {
                    subs.push((start, end, txt, is_mic));
                }
            }
        }
    }
    subs.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for (t, _) in &frames {
        lo = lo.min(*t);
        hi = hi.max(*t);
    }
    for (s, e, _, _) in &subs {
        lo = lo.min(*s);
        hi = hi.max(*e);
    }
    if !lo.is_finite() {
        lo = 0.0;
        hi = 0.0;
    }
    (frames, subs, lo, hi)
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
        let (shot_format, shot_res_ix, jpeg_quality, mic_device) = load_settings();
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
            confirm_delete: None,
            playback: None,
            pb_dragging: false,
            pb_ticker: false,
            live: Arc::new(Mutex::new(LiveState::default())),
            tray: if under_agent { None } else { tray::build() },
            _hotkey_mgr: None,
            hotkey_id: 0,
            skill_status: Vec::new(),
            asr: daemon::AsrModels::default(),
            perms: daemon::Permissions::default(),
            cmd_input: String::new(),
            cmd_focus: cx.focus_handle(),
            root_scroll: ScrollHandle::new(),
            sb_drag: None,
            show_settings: false,
            shot_format,
            shot_res_ix,
            jpeg_quality,
            message: "".into(),
            out_dir: default_out_dir(),
            polling: false,
        };
        if let Some((mgr, id)) = hotkey::build() {
            app._hotkey_mgr = Some(mgr);
            app.hotkey_id = id;
        }
        app.refresh_skill_status();
        app.refresh_blocking();
        app.start_poll(cx);
        app.spawn_sse();
        app.start_tray_loop(cx);
        app
    }

    fn toggle_capture(&mut self, cx: &mut Context<Self>) {
        if self.sessions.iter().any(|s| s.state == "running") {
            self.stop_all(cx);
        } else {
            self.start_capture(cx);
        }
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

    fn stop_all(&mut self, cx: &mut Context<Self>) {
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

    fn refresh_blocking(&mut self) {
        if let Some(d) = &self.daemon {
            self.health = d.health().ok();
            self.sessions = d.sessions().unwrap_or_default();
            if self.windows.is_empty() {
                self.windows = d.windows().unwrap_or_default();
            }
            self.asr = d.asr_models().unwrap_or_default();
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
                    // Fetch the mic list once, the first time a daemon is available.
                    if !v.mics_loaded && v.daemon.is_some() {
                        v.refresh_mics(cx);
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

    fn refresh_windows(&mut self, cx: &mut Context<Self>) {
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
    fn select_session(&mut self, id: String, cx: &mut Context<Self>) {
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
                    .spawn(async move { load_playback_data(&dir) })
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
    fn shot_settings(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("screenshot_format".into(), serde_json::json!(self.shot_format));
        if let Some(res) = RES_PRESETS.get(self.shot_res_ix).and_then(|p| p.1) {
            m.insert("screenshot_resolution".into(), serde_json::json!(res));
        }
        if self.shot_format == "jpeg" {
            m.insert("screenshot_jpeg_quality".into(), serde_json::json!(self.jpeg_quality));
        }
        serde_json::Value::Object(m)
    }

    fn start_capture(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else {
            self.message = "no daemon — run: capture daemon start".into();
            return;
        };
        // One session per CHECKED window, in picker order. Per app (pid): only the
        // first window records the app audio (macOS audio is per-app); the rest are
        // screenshots-only. The mic attaches to the first window of the chosen app.
        let out = self.out_dir.clone();
        let shot = self.shot_settings();
        let mic_device = self.mic_device.clone();
        let mic_app = self.mic_app.clone();
        let mut audio_pids: HashSet<i64> = HashSet::new();
        let mut bodies: Vec<serde_json::Value> = Vec::new();
        for w in self.windows.iter().filter(|w| self.checked.contains(&w.window_id)) {
            let first_for_app = audio_pids.insert(w.pid); // true => first checked window of this pid
            let wants_mic = first_for_app
                && mic_device.is_some()
                && mic_app.as_deref() == Some(w.app_name.as_str());
            let mut body = serde_json::json!({
                // window_id pins screenshots to the EXACT picked window (pid alone
                // can't disambiguate two windows of one process, e.g. Chrome).
                "output_dir": out, "pid": w.pid, "window_id": w.window_id,
                "audio_source": "app", "capture_audio": first_for_app,
                "screenshot_interval": 2.0,
            });
            if wants_mic {
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
        self.message = format!("starting {n} capture(s)…").into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut ok = 0usize;
            let mut last_id: Option<String> = None;
            let mut err: Option<String> = None;
            for body in bodies {
                let d2 = d.clone();
                match cx
                    .background_executor()
                    .spawn(async move { d2.start(body) })
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

    fn stop_capture(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = format!("stopping {}…", short_id(&id)).into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { d.stop(&id) })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = match r {
                    Ok(s) => format!("stopped {}", short_id(&s.session_id)).into(),
                    Err(e) => format!("stop failed: {e}").into(),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_skill_status(&mut self) {
        self.skill_status = skill::AGENTS.iter().map(skill::status).collect();
    }

    fn install_skill(&mut self, ix: usize, cx: &mut Context<Self>) {
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
    fn download_model(&mut self, repo: String, cx: &mut Context<Self>) {
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
    fn set_active_model(&mut self, repo: String, cx: &mut Context<Self>) {
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

    /// Persist the capture-quality prefs so they survive a GUI relaunch. Called from
    /// each quality control's on-click (best-effort; a write failure is silent).
    fn save_settings(&self) {
        let Some(p) = settings_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let v = serde_json::json!({
            "shot_format": self.shot_format,
            "shot_res_ix": self.shot_res_ix,
            "jpeg_quality": self.jpeg_quality,
            "mic_device": self.mic_device,
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&v) {
            let _ = std::fs::write(&p, bytes);
        }
    }

    /// Remove a downloaded model's weights from the HF cache (frees disk). The poll
    /// loop refreshes the catalog so the row flips back to "Download" once gone.
    fn delete_model(&mut self, repo: String, cx: &mut Context<Self>) {
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

    // -- launch a process/URL ---------------------------------------------------

    /// Key handling for the single-line "launch a command/URL" field. Minimal:
    /// printable chars (via `key_char`), backspace, ⌘V paste, Enter = launch.
    fn on_cmd_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
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
    fn launch_command(&mut self, cx: &mut Context<Self>) {
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
            let r = cx.background_executor().spawn(async move { d.start(body) }).await;
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

    // -- per-capture actions ----------------------------------------------------

    /// Reveal a capture's output folder in Finder (macOS `open`).
    fn open_folder(&mut self, dir: String, cx: &mut Context<Self>) {
        if dir.is_empty() {
            self.message = "no folder for this capture".into();
            cx.notify();
            return;
        }
        let ok = std::process::Command::new("open").arg(&dir).spawn().is_ok();
        self.message = if ok {
            format!("opened {dir}").into()
        } else {
            "could not open folder".into()
        };
        cx.notify();
    }

    /// Copy a ready-to-paste prompt that asks a coding agent to summarize this
    /// capture (points it at the session dir's transcript + screenshots + logs).
    fn copy_summary_prompt(&mut self, dir: String, cx: &mut Context<Self>) {
        let prompt = format!(
            "Summarize this screen + audio capture for me.\n\n\
             The capture is in this folder:\n  {dir}\n\n\
             It contains:\n\
             - transcript.jsonl — timestamped speech-to-text (one JSON object per line)\n\
             - screenshots/ — timestamped frames of the captured window\n\
             - session.json — metadata (app/window, timing, counts)\n\
             - output.log / stdout.log / stderr.log — process logs (if a launched process)\n\n\
             Read the transcript and skim the screenshots, then give me:\n\
             1. A concise summary of what happened / was discussed.\n\
             2. Key points, decisions, and action items.\n\
             3. Anything notable on screen the transcript misses.\n\
             Cite timestamps where useful."
        );
        cx.write_to_clipboard(ClipboardItem::new_string(prompt));
        self.message = "copied a summarization prompt — paste it into your coding agent".into();
        cx.notify();
    }

    /// Delete a finished capture (its folder + record) via the daemon.
    fn delete_session(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(d) = self.daemon.clone() else { return };
        self.message = format!("deleting {}…", short_id(&id)).into();
        if self.selected_session.as_deref() == Some(id.as_str()) {
            self.selected_session = None;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn({
                    let id = id.clone();
                    async move { d.delete(&id) }
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.message = match r {
                    Ok(()) => "deleted capture".into(),
                    Err(e) => format!("delete failed: {e}").into(),
                };
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
    fn request_permission(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        match kind {
            "microphone" => self.request_microphone(cx),
            _ => self.request_screen_recording(cx),
        }
    }

    fn request_screen_recording(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        let already = screen_perm::request();
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
    fn request_microphone(&mut self, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    /// Open System Settings → Privacy & Security → `pane` (grant OR revoke happens
    /// there — apps can't toggle a TCC right themselves).
    fn open_privacy_settings(&mut self, pane: &'static str, cx: &mut Context<Self>) {
        let _ = std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{pane}"
            ))
            .spawn();
        self.message = "opened System Settings — toggle Capture to grant or revoke".into();
        cx.notify();
    }

    /// Restart the bundled daemon so a just-granted Screen Recording right takes
    /// effect: ask it to shut down — the menu-bar agent respawns it automatically.
    fn restart_daemon(&mut self, cx: &mut Context<Self>) {
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

    /// One permission row: status + (a Grant button if it's promptable here) + Settings.
    /// `can_prompt` is true only for Screen Recording (CoreGraphics FFI); Microphone
    /// has no Grant button — it's granted via Settings / auto-prompted by ffmpeg.
    fn perm_row(
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
            "granted" => (format!("{title}: ✓ granted"), 0x66d9a0u32, true),
            "undetermined" => (format!("{title}: not requested"), 0x9aa0a6u32, false),
            _ => (format!("{title}: ✗ not granted — needed for {why}"), 0xffcc66u32, false),
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
                    .bg(rgb(0x2d4f67))
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
                .bg(rgb(0x2a2a2a))
                .child("Settings")
                .on_click(cx.listener(move |this, _, _, cx| this.open_privacy_settings(pane, cx))),
        )
    }
}

/// A monochrome SVG icon from the embedded asset source (`gui/assets/icons/`),
/// tinted `color` and sized `sz`×`sz` px. gpui rasterizes the SVG to an alpha mask
/// and fills it with the element's `text_color`.
fn icon(name: &str, sz: f32, color: u32) -> impl IntoElement {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(sz))
        .flex_none()
        .text_color(rgb(color))
}

fn button(
    label: &str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(label.to_string()))
        .px_3()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .bg(rgb(0x2d4f67))
        .child(label.to_string())
        .on_click(on_click)
}

/// A selectable "chip" for Settings toggles (highlighted when `selected`).
fn chip(
    id: &str,
    label: &str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .bg(if selected { rgb(0x2d4f67) } else { rgb(0x2a2a2a) })
        .text_color(if selected { rgb(0xe0e0e0) } else { rgb(0x9aa0a6) })
        .child(label.to_string())
        .on_click(on_click)
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
                            this.confirm_delete = Some(id_del.clone());
                            cx.notify();
                        }),
                    ));
                }
                row
            })
            .collect();
        session_rows.reverse();

        // (The session "detail" is now its own full Playback screen — see render_playback.)

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
                    (format!("↓ {:.0}%", (f * 100.0).clamp(0.0, 100.0)), 0x66d9a0)
                } else if m.downloading {
                    ("↓ downloading…".to_string(), 0x66d9a0)
                } else if m.active && m.downloaded {
                    ("● active".to_string(), 0x66d9a0)
                } else if m.active {
                    ("● active · needs download".to_string(), 0xffcc66)
                } else if m.downloaded {
                    ("✓ downloaded".to_string(), 0x66d9a0)
                } else {
                    (String::new(), 0x9aa0a6)
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
                            .bg(rgb(0x2d4f67))
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
                                .bg(rgb(0x2d4f67))
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
                            .bg(rgb(0x3a2a2a))
                            .text_color(rgb(0xe6a0a0))
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
                            .bg(rgb(0x2a2a2a))
                            .child(
                                div()
                                    .h(px(4.0))
                                    .w(relative(frac))
                                    .rounded_full()
                                    .bg(rgb(0x66d9a0)),
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
        let mut asr_panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_color(rgb(0x9aa0a6)).child(asr_label));
        if self.asr.backend_available {
            asr_panel = asr_panel.children(model_rows);
        }

        let cmd_focused = self.cmd_focus.is_focused(window);
        let scrollbar = self.scrollbar(cx);
        let settings = self.show_settings;
        // Three top-level screens: dashboard (default), settings, and the session
        // playback screen. Only one renders at a time.
        let playback = self.playback.is_some();
        let sett = settings && !playback;
        let dash = !settings && !playback;

        // Capture-quality settings (Settings screen): screenshot format + resolution
        // + jpeg quality, applied to new captures via shot_settings().
        let is_jpeg = self.shot_format == "jpeg";
        let mut quality_panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_color(rgb(0x9aa0a6)).child("Capture quality"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().min_w(px(96.0)).text_color(rgb(0x9aa0a6)).child("Format"))
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
                    .child(div().min_w(px(96.0)).text_color(rgb(0x9aa0a6)).child("Resolution"))
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
                    .child(div().min_w(px(96.0)).text_color(rgb(0x9aa0a6)).child("JPEG quality"))
                    .children([60u32, 80, 95].into_iter().map(|q| {
                        chip(&format!("q-{q}"), &q.to_string(), self.jpeg_quality == q, cx.listener(move |this, _, _, cx| {
                            this.jpeg_quality = q;
                            this.save_settings();
                            cx.notify();
                        }))
                    })),
            );
        }

        div()
            .relative()
            .size_full()
            .bg(rgb(0x141414))
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
            .child(
                div()
                    .id("root")
                    .track_scroll(&self.root_scroll)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .size_full()
                    .overflow_y_scroll() // single page scroll; the scrollbar overlay drives it
                    .text_color(rgb(0xe0e0e0))
                    .text_sm()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_xl().child("capture"))
                            .child({
                                // Back from a sub-screen (playback/settings), else open Settings.
                                let in_sub = playback || settings;
                                div()
                                    .id("hdr-btn")
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(0x2d4f67))
                                    .child(icon(
                                        if in_sub { "chevron-left" } else { "settings" },
                                        14.0,
                                        0xe0e0e0,
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
            .child(div().text_color(rgb(0x9aa0a6)).child(header))
            .child(div().text_color(rgb(0x6a8a9a)).child(hotkey_hint))
            .child(div().text_color(rgb(0xffcc66)).child(self.message.clone()))
            // Settings screen: capture quality (+ voice model / permissions / skill below).
            .children(sett.then(|| quality_panel))
            .children(dash.then(|| {
                div()
                    .flex()
                    .gap_2()
                    .child(button(
                        "Refresh windows",
                        cx.listener(|this, _, _, cx| this.refresh_windows(cx)),
                    ))
                    .child(button(
                        "Start capture",
                        cx.listener(|this, _, _, cx| this.start_capture(cx)),
                    ))
            }))
            .children(dash.then(|| {
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
                row
            }))
            .children(dash.then(|| {
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
                                "command or URL — e.g. open https://…  (Enter to launch)".to_string()
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
            }))
            .children(sett.then(|| {
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(rgb(0x9aa0a6)).child("Skill →"))
                    .children(skill::AGENTS.iter().enumerate().map(|(ix, a)| {
                        let label = match self.skill_status.get(ix) {
                            Some(skill::SkillStatus::UpToDate) => format!("{} ✓", a.label),
                            Some(skill::SkillStatus::UpdateAvailable) => format!("{} ↑ update", a.label),
                            _ => format!("{} — install", a.label),
                        };
                        button(&label, cx.listener(move |this, _, _, cx| this.install_skill(ix, cx)))
                    }))
            }))
            .children(sett.then(|| {
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
                        .child(div().text_color(rgb(0x9aa0a6)).child("Permissions"))
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
                panel
            }))
            .children(dash.then(|| {
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
            }))
            .children(sett.then(|| asr_panel))
            .children(playback.then(|| self.render_playback(window, cx))),
            )
            .children(scrollbar)
            // Delete confirmation modal — an occluding dim backdrop + a centered card.
            .children(self.confirm_delete.clone().map(|sid| {
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x000000aa))
                    .occlude()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .w(px(320.0))
                            .p_4()
                            .rounded_lg()
                            .bg(rgb(0x1c1c1c))
                            .child(div().text_lg().child("Delete this capture?"))
                            .child(div().text_color(rgb(0x9aa0a6)).child(format!(
                                "{} — removes the folder and its record. This can't be undone.",
                                short_id(&sid)
                            )))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(button(
                                        "Cancel",
                                        cx.listener(|this, _, _, cx| {
                                            this.confirm_delete = None;
                                            cx.notify();
                                        }),
                                    ))
                                    .child({
                                        let s = sid.clone();
                                        div()
                                            .id("del-go")
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .px_3()
                                            .py_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .bg(rgb(0x7a2d2d))
                                            .child(icon("trash", 14.0, 0xe6c0c0))
                                            .child("Delete")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.confirm_delete = None;
                                                this.delete_session(s.clone(), cx);
                                            }))
                                    }),
                            ),
                    )
            }))
    }
}

impl CaptureApp {
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
                    .bg(if dragging { rgb(0x6a6a6a) } else { rgb(0x4a4a4a) })
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

    // -- session playback screen ----------------------------------------------

    /// The full playback screen: the screenshot at the playhead (or live latest),
    /// time-synced subtitles, and (for finished captures) a scrubber + transport.
    fn render_playback(&self, _window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(pb) = self.playback.as_ref() else {
            return div().into_any_element();
        };
        let finished = pb.finished;
        let (shot, subs): (Option<String>, Vec<(String, bool)>) = if finished {
            let frame = pb
                .frames
                .iter()
                .rev()
                .find(|(t, _)| *t <= pb.pos)
                .or_else(|| pb.frames.first())
                .map(|(_, p)| p.clone());
            let mut active: Vec<(String, bool)> = pb
                .subs
                .iter()
                .filter(|(s, e, _, _)| *s <= pb.pos && pb.pos <= *e)
                .map(|(_, _, t, m)| (t.clone(), *m))
                .collect();
            if active.is_empty() {
                if let Some((_, _, t, m)) = pb.subs.iter().rev().find(|(s, _, _, _)| *s <= pb.pos) {
                    active.push((t.clone(), *m));
                }
            }
            (frame, active)
        } else {
            let st = self.live.lock().unwrap();
            let lines = st.transcript.iter().rev().take(8).rev().map(|l| (l.clone(), false)).collect();
            (st.last_shot.clone(), lines)
        };

        let mut root = div().flex().flex_col().gap_2().flex_shrink_0();
        root = root.child(div().text_color(rgb(0x9aa0a6)).child(format!(
            "{} · {}",
            short_id(&pb.sid),
            if finished { "saved capture" } else { "● live" }
        )));
        root = match shot {
            Some(p) => root.child(img(PathBuf::from(p)).w_full().h(px(360.0)).rounded_md()),
            None => root.child(
                div()
                    .w_full()
                    .h(px(360.0))
                    .rounded_md()
                    .bg(rgb(0x0e1216))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().text_color(rgb(0x6a6a6a)).child(if finished {
                        "no screenshots"
                    } else {
                        "waiting for first frame…"
                    })),
            ),
        };
        let mut subbox = div().flex().flex_col().gap_1().p_2().rounded_md().bg(rgb(0x0e1216));
        if subs.is_empty() {
            subbox = subbox.child(div().text_color(rgb(0x6a6a6a)).child("…"));
        } else {
            for (txt, is_mic) in subs {
                subbox = subbox.child(if is_mic {
                    div()
                        .flex()
                        .gap_1()
                        .items_center()
                        .child(icon("mic", 12.0, 0x88c0a0))
                        .child(div().text_color(rgb(0x88c0a0)).child(txt))
                } else {
                    div().child(div().text_color(rgb(0xe6e6e6)).child(txt))
                });
            }
        }
        root = root.child(subbox);

        if finished && pb.loaded && pb.t1 > pb.t0 {
            let dur = pb.t1 - pb.t0;
            let frac = (((pb.pos - pb.t0) / dur) as f32).clamp(0.0, 1.0);
            let track = div()
                .id("pb-track")
                .relative()
                .w_full()
                .h(px(10.0))
                .rounded_full()
                .bg(rgb(0x2a2a2a))
                .cursor_pointer()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h(px(10.0))
                        .w(relative(frac))
                        .rounded_full()
                        .bg(rgb(0x2d7f67)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        this.pb_dragging = true;
                        this.pb_seek_x(ev.position.x, window, cx);
                    }),
                );
            let playing = pb.playing;
            let controls = div()
                .flex()
                .items_center()
                .gap_2()
                .child(self.pb_ctrl("pb-start", "skip-back", cx.listener(|this, _, _, cx| this.pb_step(f64::NEG_INFINITY, cx))))
                .child(self.pb_ctrl("pb-rew", "rewind", cx.listener(|this, _, _, cx| this.pb_step(-5.0, cx))))
                .child(self.pb_ctrl("pb-play", if playing { "pause" } else { "play" }, cx.listener(|this, _, _, cx| this.pb_toggle_play(cx))))
                .child(self.pb_ctrl("pb-ff", "fast-forward", cx.listener(|this, _, _, cx| this.pb_step(5.0, cx))))
                .child(self.pb_ctrl("pb-end", "skip-forward", cx.listener(|this, _, _, cx| this.pb_step(f64::INFINITY, cx))))
                .child(div().text_color(rgb(0x9aa0a6)).child(format!("{} / {}", fmt_dur(pb.pos - pb.t0), fmt_dur(dur))));
            root = root.child(div().flex().flex_col().gap_2().child(track).child(controls));
        } else if finished && !pb.loaded {
            root = root.child(div().text_color(rgb(0x6a6a6a)).child("loading…"));
        }
        root.into_any_element()
    }

    /// A small transport-control icon button.
    fn pb_ctrl(
        &self,
        id: &'static str,
        name: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(36.0))
            .h(px(28.0))
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(0x2a2a2a))
            .child(icon(name, 14.0, 0xcfd3d6))
            .on_click(on_click)
    }

    /// Seek the playhead to a scrubber-track mouse-x (the track spans the content
    /// width: left = root padding 16px, width = viewport − 32).
    fn pb_seek_x(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let tw = window.viewport_size().width - px(32.0);
        if tw <= px(0.0) {
            return;
        }
        let frac = ((x - px(16.0)) / tw).clamp(0.0, 1.0);
        if let Some(pb) = self.playback.as_mut() {
            if pb.t1 > pb.t0 {
                pb.pos = pb.t0 + frac as f64 * (pb.t1 - pb.t0);
                pb.playing = false;
                cx.notify();
            }
        }
    }

    fn pb_step(&mut self, delta: f64, cx: &mut Context<Self>) {
        if let Some(pb) = self.playback.as_mut() {
            pb.pos = (pb.pos + delta).clamp(pb.t0, pb.t1);
            pb.playing = false;
            cx.notify();
        }
    }

    fn pb_toggle_play(&mut self, cx: &mut Context<Self>) {
        let mut now_playing = false;
        if let Some(pb) = self.playback.as_mut() {
            if pb.pos >= pb.t1 {
                pb.pos = pb.t0; // replay from the start if parked at the end
            }
            pb.playing = !pb.playing;
            now_playing = pb.playing;
        }
        cx.notify();
        if now_playing {
            self.pb_start_ticker(cx);
        }
    }

    /// Advance the playhead in ~real time while `playing`; exits when paused/closed.
    fn pb_start_ticker(&mut self, cx: &mut Context<Self>) {
        if self.pb_ticker {
            return;
        }
        self.pb_ticker = true;
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(200)).await;
                let go = this
                    .update(cx, |v, cx| {
                        let go = matches!(v.playback.as_ref(), Some(pb) if pb.playing);
                        if go {
                            if let Some(pb) = v.playback.as_mut() {
                                pb.pos = (pb.pos + 0.2).min(pb.t1);
                                if pb.pos >= pb.t1 {
                                    pb.playing = false;
                                }
                            }
                            cx.notify();
                        } else {
                            v.pb_ticker = false;
                        }
                        go
                    })
                    .unwrap_or(false);
                if !go {
                    break;
                }
            }
        })
        .detach();
    }
}
