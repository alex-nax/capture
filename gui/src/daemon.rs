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
    // Capability flags (what the session can still do, disk-computed by the daemon).
    #[serde(default = "default_true")]
    pub has_screenshots: bool,
    #[serde(default = "default_true")]
    pub has_audio: bool,
    #[serde(default)]
    pub has_mic: bool,
    #[serde(default)]
    pub mic_device: Option<String>, // active mic input id (None = off), for the live switcher
    #[serde(default = "default_true")]
    pub can_retranscribe: bool,
    #[serde(default = "default_true")]
    pub can_index: bool,
}

fn default_true() -> bool {
    true
}

/// Availability of the multimodal-index endpoint (GET /v1/index/status). Extra response
/// fields (url/model) are ignored — the GUI only needs the gate.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct IndexStatus {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub configured: bool,
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
    pub language: Option<String>, // transcription language setting (None = auto)
    #[serde(default = "default_chunk")]
    pub chunk_seconds: f64, // transcription chunk length setting
    #[serde(default)]
    pub models: Vec<AsrModel>,
}

fn default_chunk() -> f64 {
    30.0
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

    /// Prune a finished capture's artifacts (parts: screenshots / screenshots_halve / audio).
    pub fn prune(&self, id: &str, parts: &[&str]) -> Result<(), String> {
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/sessions/{}/prune", self.endpoint, id))
                .set("Authorization", &self.auth())
                .send_json(serde_json::json!({ "parts": parts })),
            "prune failed",
        )
    }

    /// Re-transcribe a finished capture's audio with the active (or given) model.
    pub fn retranscribe(&self, id: &str, model: Option<&str>) -> Result<(), String> {
        let mut body = serde_json::json!({});
        if let Some(m) = model {
            body["model"] = serde_json::json!(m);
        }
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/sessions/{}/retranscribe", self.endpoint, id))
                .set("Authorization", &self.auth())
                .send_json(body),
            "retranscribe failed",
        )
    }

    /// Import an audio/video file as a session (background; progress over /v1/events).
    pub fn import_media(&self, path: &str) -> Result<(), String> {
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/sessions/import", self.endpoint))
                .set("Authorization", &self.auth())
                .send_json(serde_json::json!({ "path": path })),
            "import failed",
        )
    }

    /// Build a session's multimodal index (background; progress over /v1/events). The
    /// `endpoint`/`model` overrides carry the GUI-configured LM Studio URL + model id.
    pub fn index(&self, id: &str, endpoint: &str, model: &str, sample_rate: f64, preset: &str) -> Result<(), String> {
        let mut body = serde_json::json!({ "sample_rate": sample_rate });
        if !endpoint.trim().is_empty() {
            body["endpoint"] = serde_json::json!(endpoint.trim());
        }
        if !model.trim().is_empty() {
            body["model"] = serde_json::json!(model.trim());
        }
        if !preset.trim().is_empty() {
            body["prompt_preset"] = serde_json::json!(preset.trim());
        }
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/sessions/{}/index", self.endpoint, id))
                .set("Authorization", &self.auth())
                .send_json(body),
            "index failed",
        )
    }

    /// Whether indexing is available against `url` (configured + a reachable preflight).
    pub fn index_status(&self, url: &str) -> Result<IndexStatus, String> {
        let mut req = Self::agent()
            .get(&format!("{}/v1/index/status", self.endpoint))
            .set("Authorization", &self.auth());
        if !url.trim().is_empty() {
            req = req.query("url", url.trim());
        }
        req.call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())
    }

    /// The built index tree for a session (Err if not indexed yet).
    pub fn get_index(&self, id: &str) -> Result<serde_json::Value, String> {
        Self::agent()
            .get(&format!("{}/v1/sessions/{}/index", self.endpoint, id))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())
    }

    /// Map a ureq response to `Ok(())` or the daemon's `{"error": …}` message.
    fn ok_or_error(resp: Result<ureq::Response, ureq::Error>, fallback: &str) -> Result<(), String> {
        match resp {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(_, r)) => Err(r
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .unwrap_or_else(|| fallback.into())),
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

    /// Switch the microphone on a LIVE capture (empty = off). Appends to the mic track.
    pub fn set_mic(&self, id: &str, device: Option<&str>) -> Result<(), String> {
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/sessions/{}/mic", self.endpoint, id))
                .set("Authorization", &self.auth())
                .send_json(serde_json::json!({ "device": device })),
            "mic switch failed",
        )
    }

    /// Set the transcription language ("" / "auto" = auto-detect). Applies on the fly.
    pub fn asr_set_language(&self, language: &str) -> Result<(), String> {
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/asr/language", self.endpoint))
                .set("Authorization", &self.auth())
                .send_json(serde_json::json!({ "language": language })),
            "set language failed",
        )
    }

    /// Set the transcription chunk length (seconds).
    pub fn asr_set_chunk(&self, seconds: f64) -> Result<(), String> {
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/asr/chunk", self.endpoint))
                .set("Authorization", &self.auth())
                .send_json(serde_json::json!({ "seconds": seconds })),
            "set chunk failed",
        )
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
