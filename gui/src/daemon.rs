//! Thin blocking client for the `captured` /v1 API (the GUI's only backend).
//!
//! Mirrors `capture_mcp/daemon/client.py`: reads `~/.capture/daemon.json` for the
//! endpoint + bearer token, then GET/POSTs the /v1 routes. Blocking (ureq) — the
//! GUI calls these off the main thread via the background executor.

use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone)]
pub struct Daemon {
    pub endpoint: String,
    pub token: String,
}

#[derive(Deserialize)]
struct DaemonJson {
    endpoint: String,
    token: String,
}

#[derive(Deserialize, Clone, Default)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub api_version: String,
    pub pid: u32,
}

#[derive(Deserialize, Clone, Default)]
#[allow(dead_code)] // mirrors the /v1 SessionSummary wire shape; not all fields are shown yet
pub struct Session {
    pub session_id: String,
    pub state: String,
    #[serde(default)]
    pub screenshots: i64,
    #[serde(default)]
    pub transcript_segments: i64,
    #[serde(default)]
    pub audio_status: String,
    #[serde(default)]
    pub window_title: Option<String>,
    #[serde(default)]
    pub dir: String,
}

#[derive(Deserialize)]
struct Sessions {
    sessions: Vec<Session>,
}

#[derive(Deserialize, Clone)]
#[allow(dead_code)] // mirrors the /v1 WindowInfo wire shape (window_id/width/height for slice 2)
pub struct WindowInfo {
    pub window_id: i64,
    pub pid: i64,
    pub app_name: String,
    pub title: String,
    pub width: i64,
    pub height: i64,
}

#[derive(Deserialize)]
struct Windows {
    windows: Vec<WindowInfo>,
}

#[derive(Deserialize, Clone)]
pub struct TranscriptSeg {
    pub text: String,
}

#[derive(Deserialize, Clone)]
pub struct AsrModel {
    pub repo: String,
    pub name: String,
    pub size_label: String,
    pub downloaded: bool,
    pub active: bool,
    #[serde(default)]
    pub downloading: bool,
}

#[derive(Deserialize, Clone, Default)]
pub struct Permissions {
    #[serde(default)]
    #[allow(dead_code)] // wire shape; UI keys off the per-permission fields
    pub platform: String,
    #[serde(default)]
    pub screen_recording: String,
    #[serde(default)]
    pub microphone: String,
}

#[derive(Deserialize, Clone, Default)]
pub struct AsrModels {
    #[serde(default)]
    pub backend_available: bool,
    #[serde(default)]
    #[allow(dead_code)] // wire shape; UI reads the per-model `active` flag instead
    pub active: String,
    #[serde(default)]
    pub models: Vec<AsrModel>,
}

#[derive(Deserialize, Clone, Default)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub default: bool,
}

#[derive(Deserialize, Clone, Default)]
pub struct AudioDevices {
    #[serde(default)]
    pub devices: Vec<AudioDevice>,
}

#[derive(Deserialize)]
struct Transcript {
    segments: Vec<TranscriptSeg>,
}

fn daemon_json_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CAPTURE_DAEMON_JSON") {
        return Some(PathBuf::from(p));
    }
    dirs::home_dir().map(|h| h.join(".capture").join("daemon.json"))
}

/// Discover a running daemon from its 0600 discovery file (None if absent).
pub fn discover() -> Option<Daemon> {
    let path = daemon_json_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let dj: DaemonJson = serde_json::from_str(&text).ok()?;
    Some(Daemon {
        endpoint: dj.endpoint,
        token: dj.token,
    })
}

/// Path to the daemon bundled inside the packaged app, if present:
/// `Capture.app/Contents/Resources/captured/captured` (next to the GUI binary's
/// `MacOS` dir). None in a dev build (run the daemon from the venv instead).
pub fn bundled_daemon() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let cand = exe.parent()?.join("../Resources/captured/captured");
    if cand.exists() {
        Some(cand)
    } else {
        None
    }
}

/// Spawn a daemon binary **detached** (own process group → outlives the GUI, so
/// captures survive the app quitting). Returns true if it launched.
pub fn spawn_detached(bin: &std::path::Path) -> bool {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(bin)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}

impl Daemon {
    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .build()
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// True iff the daemon answers /v1/health (cheap liveness probe).
    pub fn available(&self) -> bool {
        self.health().is_ok()
    }

    pub fn health(&self) -> Result<Health, String> {
        Self::agent()
            .get(&format!("{}/v1/health", self.endpoint))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())
    }

    pub fn sessions(&self) -> Result<Vec<Session>, String> {
        let r: Sessions = Self::agent()
            .get(&format!("{}/v1/sessions", self.endpoint))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())?;
        Ok(r.sessions)
    }

    pub fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        let r: Windows = Self::agent()
            .get(&format!("{}/v1/windows", self.endpoint))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())?;
        Ok(r.windows)
    }

    pub fn transcript(&self, id: &str, tail: u32) -> Result<Vec<TranscriptSeg>, String> {
        let r: Transcript = Self::agent()
            .get(&format!("{}/v1/sessions/{}/transcript", self.endpoint, id))
            .query("tail", &tail.to_string())
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())?;
        Ok(r.segments)
    }

    /// Open the `/v1/events` SSE stream as a line reader. Uses a NO-timeout agent
    /// (the stream is long-lived); the caller reads `data: {json}` lines forever.
    pub fn open_events(&self) -> Result<Box<dyn std::io::BufRead + Send>, String> {
        let resp = ureq::agent()
            .get(&format!("{}/v1/events", self.endpoint))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?;
        Ok(Box::new(std::io::BufReader::new(resp.into_reader())))
    }

    pub fn start(&self, body: serde_json::Value) -> Result<Session, String> {
        let resp = Self::agent()
            .post(&format!("{}/v1/sessions", self.endpoint))
            .set("Authorization", &self.auth())
            .send_json(body);
        parse_session_or_error(resp)
    }

    pub fn stop(&self, id: &str) -> Result<Session, String> {
        let resp = Self::agent()
            .post(&format!("{}/v1/sessions/{}/stop", self.endpoint, id))
            .set("Authorization", &self.auth())
            .send_json(serde_json::json!({}));
        parse_session_or_error(resp)
    }

    /// Delete a finished capture (its dir + record). Errs (400) on a live session.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let resp = Self::agent()
            .post(&format!("{}/v1/sessions/{}/delete", self.endpoint, id))
            .set("Authorization", &self.auth())
            .send_json(serde_json::json!({}));
        match resp {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(_, r)) => Err(r
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| "delete failed".into())),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Available microphone/input devices for the mic selector.
    pub fn audio_mics(&self) -> Result<AudioDevices, String> {
        Self::agent()
            .get(&format!("{}/v1/audio/mics", self.endpoint))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())
    }

    /// The Whisper model catalog (+ downloaded/active/downloading flags).
    pub fn asr_models(&self) -> Result<AsrModels, String> {
        Self::agent()
            .get(&format!("{}/v1/asr/models", self.endpoint))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())
    }

    /// Start a background download of `repo` (progress arrives over /v1/events).
    pub fn asr_download(&self, repo: &str) -> Result<(), String> {
        self.asr_post("/v1/asr/models/download", repo)
    }

    /// Set the active Whisper model (persisted; new captures use it).
    pub fn asr_set_model(&self, repo: &str) -> Result<(), String> {
        self.asr_post("/v1/asr/model", repo)
    }

    /// Remove a downloaded model's weights from the HF cache (frees disk).
    pub fn asr_delete(&self, repo: &str) -> Result<(), String> {
        self.asr_post("/v1/asr/models/delete", repo)
    }

    /// Ask the daemon to stop (the menu-bar agent respawns it — used to restart so a
    /// just-granted Screen Recording right takes effect).
    pub fn shutdown(&self) -> Result<(), String> {
        Self::agent()
            .post(&format!("{}/v1/admin/shutdown", self.endpoint))
            .set("Authorization", &self.auth())
            .send_json(serde_json::json!({}))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// macOS TCC status (e.g. `screen_recording: "granted"|"denied"`).
    pub fn permissions(&self) -> Result<Permissions, String> {
        Self::agent()
            .get(&format!("{}/v1/permissions", self.endpoint))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())
    }

    fn asr_post(&self, path: &str, repo: &str) -> Result<(), String> {
        let resp = Self::agent()
            .post(&format!("{}{}", self.endpoint, path))
            .set("Authorization", &self.auth())
            .send_json(serde_json::json!({ "repo": repo }));
        match resp {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(_, r)) => Err(r
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| "request failed".into())),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Turn a ureq response into a Session, surfacing the daemon's {"error": ...} body.
fn parse_session_or_error(resp: Result<ureq::Response, ureq::Error>) -> Result<Session, String> {
    match resp {
        Ok(r) => r.into_json().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(_, r)) => {
            let msg = r
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| "request failed".into());
            Err(msg)
        }
        Err(e) => Err(e.to_string()),
    }
}
