// A tray app must never own a console window (closing it would kill the agent + its daemon/GUI).
// Release builds are windows-subsystem (no console); debug keeps it for dev diagnostics.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Capture — Windows system-tray agent (feature #36), the native sibling of macOS `CaptureBar`.
//!
//! A thin always-resident tray app: it owns the persistent tray icon, the **daemon lifecycle**
//! (spawn the bundled `captured` with `CREATE_NO_WINDOW`; adopt an already-running one; auto-respawn
//! on crash unless the user stopped it; stop it on Quit iff idle), and **launches the GPUI window**
//! (`capture-gui.exe` with `CAPTURE_AGENT=1`). It is a peer client of the daemon `/v1` API and holds
//! no capture/ASR logic. See docs/specs/agent-windows.md.
//!
//! Runs a minimal Win32 message loop (the tray needs one) driven by a 2 s `WM_TIMER` for polling;
//! tray menu clicks arrive via `muda`'s `MenuEvent` channel.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use serde::Deserialize;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use windows::core::BOOL;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostQuitMessage, SetTimer, TranslateMessage, MSG, WM_TIMER,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const SPAWN_DEBOUNCE: Duration = Duration::from_secs(6);

// ---- daemon /v1 client (thin) ----------------------------------------------------

#[derive(Deserialize)]
struct DaemonJson {
    endpoint: String,
    token: String,
}

#[derive(Deserialize)]
struct SessionRow {
    session_id: String,
    state: String,
}
#[derive(Deserialize)]
struct SessionsResp {
    sessions: Vec<SessionRow>,
}

struct Daemon {
    endpoint: String,
    token: String,
}

fn daemon_json_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CAPTURE_DAEMON_JSON") {
        return Some(PathBuf::from(p));
    }
    dirs::home_dir().map(|h| h.join(".capture").join("daemon.json"))
}

impl Daemon {
    /// Re-read `daemon.json` each call so a just-spawned daemon is discovered.
    fn discover() -> Option<Daemon> {
        let text = std::fs::read_to_string(daemon_json_path()?).ok()?;
        let dj: DaemonJson = serde_json::from_str(&text).ok()?;
        Some(Daemon { endpoint: dj.endpoint, token: dj.token })
    }
    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new().timeout(Duration::from_secs(3)).build()
    }
    fn bearer(&self) -> String {
        format!("Bearer {}", self.token)
    }
    fn healthy(&self) -> bool {
        Self::agent()
            .get(&format!("{}/v1/health", self.endpoint))
            .set("Authorization", &self.bearer())
            .call()
            .is_ok()
    }
    fn running(&self) -> Vec<String> {
        Self::agent()
            .get(&format!("{}/v1/sessions", self.endpoint))
            .set("Authorization", &self.bearer())
            .call()
            .ok()
            .and_then(|r| r.into_json::<SessionsResp>().ok())
            .map(|s| {
                s.sessions
                    .into_iter()
                    .filter(|r| r.state == "running" || r.state == "starting")
                    .map(|r| r.session_id)
                    .collect()
            })
            .unwrap_or_default()
    }
    fn stop(&self, id: &str) {
        let _ = Self::agent()
            .post(&format!("{}/v1/sessions/{}/stop", self.endpoint, id))
            .set("Authorization", &self.bearer())
            .send_json(serde_json::json!({}));
    }
    fn shutdown(&self) {
        let _ = Self::agent()
            .post(&format!("{}/v1/admin/shutdown", self.endpoint))
            .set("Authorization", &self.bearer())
            .send_json(serde_json::json!({}));
    }
}

// ---- process discovery + spawning -----------------------------------------------

fn sibling(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(name))
}

fn daemon_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CAPTURE_DAEMON_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let c = sibling("captured")?.join("captured.exe");
    c.exists().then_some(c)
}

fn gui_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CAPTURE_GUI_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let c = sibling("capture-gui.exe")?;
    c.exists().then_some(c)
}

/// Spawn the daemon detached + windowless (no console flash; survives the agent).
fn spawn_daemon() -> bool {
    use std::os::windows::process::CommandExt;
    match daemon_bin() {
        Some(bin) => Command::new(bin)
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok(),
        None => false,
    }
}

// ---- tray icons (generated, no asset files yet) ----------------------------------

fn dot_icon(r: u8, g: u8, b: u8) -> Option<Icon> {
    let (w, h) = (32u32, 32u32);
    let (cx, cy, rad) = (15.5f32, 15.5f32, 13.0f32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            if dx * dx + dy * dy <= rad * rad {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = r;
                rgba[i + 1] = g;
                rgba[i + 2] = b;
                rgba[i + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, w, h).ok()
}

// ---- agent state -----------------------------------------------------------------

struct Agent {
    tray: TrayIcon,
    header: MenuItem,
    open: MenuItem,
    stop_all: MenuItem,
    toggle: MenuItem,
    quit: MenuItem,
    gui: Option<Child>,
    user_stopped: bool,
    last_spawn: Option<Instant>,
    recording: bool,
}

impl Agent {
    /// Launch the GPUI window (CAPTURE_AGENT=1) unless one is already open.
    fn open_window(&mut self) {
        let alive = self
            .gui
            .as_mut()
            .map(|c| matches!(c.try_wait(), Ok(None)))
            .unwrap_or(false);
        if alive {
            return; // already open (focusing the existing window is a TODO — see spec)
        }
        if let Some(bin) = gui_bin() {
            use std::os::windows::process::CommandExt;
            self.gui = Command::new(bin)
                .env("CAPTURE_AGENT", "1")
                .creation_flags(CREATE_NEW_PROCESS_GROUP)
                .spawn()
                .ok();
        }
    }

    /// Spawn the daemon if it's down and the user didn't stop it (debounced).
    fn ensure_daemon(&mut self, up: bool) {
        if up || self.user_stopped {
            return;
        }
        if self.last_spawn.map(|t| t.elapsed() < SPAWN_DEBOUNCE).unwrap_or(false) {
            return;
        }
        if spawn_daemon() {
            self.last_spawn = Some(Instant::now());
        }
    }

    /// 2 s tick: read daemon state, (re)spawn if needed, refresh the tray + menu.
    fn poll(&mut self) {
        let d = Daemon::discover();
        let up = d.as_ref().map(|d| d.healthy()).unwrap_or(false);
        let running = if up { d.as_ref().map(|d| d.running()).unwrap_or_default() } else { vec![] };
        let n = running.len();

        self.ensure_daemon(up);

        let state = if !up {
            "daemon: stopped".to_string()
        } else if n == 0 {
            "daemon: running · idle".to_string()
        } else {
            format!("daemon: running · {n} capturing")
        };
        let _ = self.header.set_text(state);
        self.stop_all.set_enabled(n > 0);
        let _ = self.toggle.set_text(if up { "Stop Daemon" } else { "Start Daemon" });

        let rec = n > 0;
        if rec != self.recording {
            self.recording = rec;
            let icon = if rec { dot_icon(0xE0, 0x3A, 0x3A) } else { dot_icon(0x88, 0x8C, 0x90) };
            let _ = self.tray.set_icon(icon);
        }
        let _ = self.tray.set_tooltip(Some(if up {
            format!("Capture — {n} capturing")
        } else {
            "Capture — daemon stopped".to_string()
        }));
    }

    fn on_menu(&mut self, id: &muda::MenuId) {
        if id == self.open.id() {
            self.open_window();
        } else if id == self.stop_all.id() {
            if let Some(d) = Daemon::discover() {
                for s in d.running() {
                    d.stop(&s);
                }
            }
        } else if id == self.toggle.id() {
            match Daemon::discover() {
                Some(d) if d.healthy() => {
                    self.user_stopped = true;
                    d.shutdown();
                }
                _ => {
                    self.user_stopped = false;
                    self.ensure_daemon(false);
                }
            }
        } else if id == self.quit.id() {
            // Stop the daemon only if idle, so an accidental Quit doesn't kill a live capture.
            if let Some(d) = Daemon::discover() {
                if d.healthy() && d.running().is_empty() {
                    d.shutdown();
                }
            }
            unsafe { PostQuitMessage(0) };
        }
    }
}

fn main() {
    let menu = Menu::new();
    let header = MenuItem::new("Capture: starting…", false, None);
    let open = MenuItem::new("Open Window", true, None);
    let stop_all = MenuItem::new("Stop All Captures", false, None);
    let toggle = MenuItem::new("Stop Daemon", true, None);
    let quit = MenuItem::new("Quit Capture", true, None);
    let sep1 = PredefinedMenuItem::separator();
    let sep2 = PredefinedMenuItem::separator();
    menu.append_items(&[&header, &sep1, &open, &stop_all, &sep2, &toggle, &quit])
        .expect("build tray menu");

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Capture")
        .with_icon(dot_icon(0x88, 0x8C, 0x90).expect("icon"))
        .build()
        .expect("create tray icon");

    let mut agent = Agent {
        tray,
        header,
        open,
        stop_all,
        toggle,
        quit,
        gui: None,
        user_stopped: false,
        last_spawn: None,
        recording: false,
    };

    // Bring the daemon up (or adopt it), open the window once, then poll every 2 s.
    agent.ensure_daemon(false);
    agent.open_window();
    agent.poll();
    unsafe { SetTimer(None, 1, 2000, None) };

    let menu_rx = MenuEvent::receiver();
    let mut msg = MSG::default();
    loop {
        let r: BOOL = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break; // 0 = WM_QUIT, -1 = error
        }
        if msg.message == WM_TIMER {
            agent.poll();
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        while let Ok(ev) = menu_rx.try_recv() {
            agent.on_menu(&ev.id);
        }
    }
}
