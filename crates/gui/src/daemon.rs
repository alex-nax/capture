//! Thin blocking client for the `captured` /v1 API (the GUI's only backend).
//!
//! Mirrors `capture_mcp/daemon/client.py`: reads `~/.capture/daemon.json` for the
//! endpoint + bearer token, then GET/POSTs the /v1 routes. Blocking (ureq) — the
//! GUI calls these off the main thread via the background executor.

use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

// The /v1 contract types now live in capture-core (the v3 source of truth). Re-export them so the
// GUI's call sites (`daemon::Session`, `daemon::AsrModels`, …) keep resolving unchanged.
pub use capture_core::v1::*;

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

#[derive(Deserialize)]
struct Sessions {
    sessions: Vec<Session>,
}

/// Model list from GET /v1/index/models (empty if the provider is unreachable).
#[derive(Deserialize, Default)]
struct IndexModels {
    #[serde(default)]
    models: Vec<String>,
}

#[derive(Deserialize)]
struct Windows {
    windows: Vec<WindowInfo>,
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

/// The basename of the Rust daemon binary (`captured`, `.exe` on Windows).
fn daemon_bin_name() -> &'static str {
    if cfg!(windows) {
        "captured.exe"
    } else {
        "captured"
    }
}

/// Path to the daemon bundled inside the packaged app, if present. None in a dev build
/// (the v3 daemon is built into the shared workspace target instead — see [`dev_daemon`]).
/// Layout differs per OS:
/// - macOS: `Capture.app/Contents/Resources/captured/captured` (capture-gui lives in MacOS/).
/// - Windows: `captured\captured.exe` beside `capture-gui.exe` at the install root
///   (see docs/specs/windows-release.md).
pub fn bundled_daemon() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    #[cfg(target_os = "macos")]
    let cand = dir.join("../Resources/captured/captured");
    #[cfg(target_os = "windows")]
    let cand = dir.join("captured").join("captured.exe");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cand = dir.join("captured").join("captured");
    cand.exists().then_some(cand)
}

/// The bundled `capture-mcp` MCP stdio binary — the `command` an MCP client puts in `.mcp.json` to
/// drive capture (#78). Same layout as [`bundled_daemon`] (macOS: `…/Resources/captured/capture-mcp`),
/// with a dev fallback to the copy built beside this `capture-gui` in the shared cargo target. The
/// path is canonicalized so it's clean (no `../`) for pasting. `None` if absent.
pub fn bundled_mcp() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(target_os = "windows") { "capture-mcp.exe" } else { "capture-mcp" };
    #[cfg(target_os = "macos")]
    let bundled = dir.join("../Resources/captured").join(name);
    #[cfg(not(target_os = "macos"))]
    let bundled = dir.join("captured").join(name);
    if bundled.exists() {
        return Some(bundled.canonicalize().unwrap_or(bundled));
    }
    // Dev/workspace: capture-mcp builds beside capture-gui in target/<profile>/.
    let beside = dir.join(name);
    beside.exists().then_some(beside)
}

/// The Rust `captured` sitting **beside** this `capture-gui` in `dir` (`None` if absent). In the v3
/// cargo workspace both binaries build into the shared `target/<profile>/`, so a dev `capture-gui`
/// finds the daemon right next to itself — that's the GUI flip onto the Rust daemon. The packaged
/// app keeps the daemon in `Resources/`, not beside the GUI, so this only matches in dev.
fn daemon_beside(dir: &std::path::Path) -> Option<PathBuf> {
    let cand = dir.join(daemon_bin_name());
    cand.exists().then_some(cand)
}

/// The Rust daemon built alongside this `capture-gui` (the dev/workspace build).
fn dev_daemon() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    daemon_beside(exe.parent()?)
}

/// The daemon to auto-spawn: the one bundled in the packaged app, else (dev) the workspace-built
/// `captured` next to this binary. None if neither exists (then start the daemon yourself).
pub fn resolve_daemon() -> Option<PathBuf> {
    bundled_daemon().or_else(dev_daemon)
}

/// Spawn a daemon binary **detached** so it outlives the GUI (captures survive the app
/// quitting). POSIX: its own process group. Windows: a new process group + no console
/// window (a stray console would steal foreground and pollute whole-screen captures).
/// Returns true if it launched.
pub fn spawn_detached(bin: &std::path::Path) -> bool {
    let mut cmd = std::process::Command::new(bin);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW (0x0800_0000) | CREATE_NEW_PROCESS_GROUP (0x0000_0200)
        cmd.creation_flags(0x0800_0000 | 0x0000_0200);
    }
    cmd.spawn().is_ok()
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

    /// Start a capture (POST /v1/sessions). A non-empty `preset` (meeting/coding/
    /// lecture/auto/general/custom) is recorded on the session and steers a later index.
    pub fn start(&self, mut body: serde_json::Value, preset: &str) -> Result<Session, String> {
        if !preset.trim().is_empty() {
            body["preset"] = serde_json::json!(preset.trim());
        }
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
    /// `provider`/`host`/`port`/`model` overrides carry the GUI-configured endpoint config;
    /// the daemon composes the chat URL from provider+host (+port) when present.
    pub fn index(
        &self,
        id: &str,
        provider: &str,
        host: &str,
        port: &str,
        model: &str,
        sample_rate: f64,
        preset: &str,
    ) -> Result<(), String> {
        let mut body = serde_json::json!({ "sample_rate": sample_rate });
        if !provider.trim().is_empty() {
            body["provider"] = serde_json::json!(provider.trim());
        }
        if !host.trim().is_empty() {
            body["host"] = serde_json::json!(host.trim());
        }
        if let Ok(p) = port.trim().parse::<u32>() {
            body["port"] = serde_json::json!(p);
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

    /// The models a provider exposes (GET /v1/index/models). Empty if unreachable.
    pub fn index_models(&self, provider: &str, host: &str, port: &str, key: &str) -> Result<Vec<String>, String> {
        let mut req = Self::agent()
            .get(&format!("{}/v1/index/models", self.endpoint))
            .set("Authorization", &self.auth());
        if !provider.trim().is_empty() {
            req = req.query("provider", provider.trim());
        }
        if !host.trim().is_empty() {
            req = req.query("host", host.trim());
        }
        if !port.trim().is_empty() {
            req = req.query("port", port.trim());
        }
        if !key.trim().is_empty() {
            req = req.query("key", key.trim());
        }
        let r: IndexModels = req
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())?;
        Ok(r.models)
    }

    /// Whether indexing is available against `url` (configured + a reachable preflight).
    pub fn index_status(&self, url: &str, model: &str) -> Result<IndexStatus, String> {
        let mut req = Self::agent()
            .get(&format!("{}/v1/index/status", self.endpoint))
            .set("Authorization", &self.auth());
        if !url.trim().is_empty() {
            req = req.query("url", url.trim());
        }
        // Pass the MODEL too: the daemon remembers (url, model) as `last_index` for LIVE indexing
        // (#84). Without it the live worker falls back to "local", which most servers reject — so the
        // auto-index silently produces nothing even though the endpoint is reachable.
        if !model.trim().is_empty() {
            req = req.query("model", model.trim());
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

    /// The selectable ASR runtimes (registry + installed/active + a GPU hint).
    pub fn asr_runtimes(&self) -> Result<AsrRuntimes, String> {
        Self::agent()
            .get(&format!("{}/v1/asr/runtimes", self.endpoint))
            .set("Authorization", &self.auth())
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())
    }

    /// Install a runtime pack (background; progress over /v1/events as asr_runtime_install).
    pub fn asr_runtime_install(&self, id: &str) -> Result<(), String> {
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/asr/runtimes/install", self.endpoint))
                .set("Authorization", &self.auth())
                .send_json(serde_json::json!({ "id": id })),
            "runtime install failed",
        )
    }

    /// Set the active runtime (loaded into the running daemon; a switch may need a restart).
    pub fn asr_set_runtime(&self, id: &str) -> Result<(), String> {
        Self::ok_or_error(
            Self::agent()
                .post(&format!("{}/v1/asr/runtime", self.endpoint))
                .set("Authorization", &self.auth())
                .send_json(serde_json::json!({ "id": id })),
            "set runtime failed",
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_beside_finds_a_present_binary() {
        let dir = std::env::temp_dir().join("capture_gui_daemon_beside_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Absent → None (the dev build hasn't compiled the daemon yet).
        assert!(daemon_beside(&dir).is_none());
        // Present → that exact path (what resolve_daemon spawns in dev).
        let bin = dir.join(daemon_bin_name());
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        assert_eq!(daemon_beside(&dir), Some(bin));
        let _ = std::fs::remove_dir_all(&dir);
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
