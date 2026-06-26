//! capture-daemon — the v3 `/v1` HTTP server (a Rust port of `src/capture_mcp/daemon/server.py`).
//!
//! This is the shared backend the GUI and the (future) MCP server both call. It owns:
//!  - **Discovery + auth**: `~/.capture/daemon.json` (`{endpoint, token, pid, api_version, version}`,
//!    mode 0600) + a bearer token required on every route except `/v1/health`. One daemon per
//!    machine (a single-instance guard refuses to start a second).
//!  - **The route surface** (`routes`): the **ready** read/index routes are served from `capture-core`
//!    (sessions, transcript) + `capture-index` (index build/get/status); the **engine** routes
//!    (capture start/stop, windows, permissions, audio mics, ASR) return `501` until the capture /
//!    asr / platform crates land (#64/#65/#66). The GUI flips onto this daemon once they do (#63/#67).
//!  - **The SSE event bus** (`/v1/events`): a `broadcast` channel the index build streams progress on.
//!
//! Contract source of truth: `capture-core::v1` (wire types) + `docs/specs/v3-architecture.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tokio::sync::Notify;

mod routes;

/// The product version reported by `/v1/health` + `daemon.json`. Mirrors `capture_mcp.__version__`.
/// TODO(#67 cutover): source this from the workspace version once the Rust daemon ships as the product.
pub const VERSION: &str = "0.3.1";

/// The `/v1` API version (wire string). Mirrors `daemon/server.py::API_VERSION` (`"1.0"`).
pub const API_VERSION: &str = "1.0";

/// Live capture states; a session in one of these is in-flight (the engine owns it). The read layer
/// rewrites a recovered live state to `"interrupted"`, so today nothing the daemon serves is live —
/// these are kept so the index/stop guards stay faithful once the engine lands.
pub(crate) const LIVE_STATES: [&str; 3] = ["starting", "running", "stopping"];

// ── Shared state ────────────────────────────────────────────────────────────────────────────────

/// Process-wide state shared by every handler (cheap to clone — all `Arc`/`Sender`).
#[derive(Clone)]
pub struct AppState {
    /// The bearer token required on every route except `/v1/health`.
    pub token: Arc<String>,
    /// SSE event bus — handlers `send` pre-serialized JSON strings; `/v1/events` subscribers stream them.
    pub events: tokio::sync::broadcast::Sender<String>,
    /// Sessions with an in-flight index build (a second build is a no-op while present).
    pub indexing: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// GGML model ids with an in-flight download (a second download is a no-op; delete is refused).
    pub asr_downloading: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Sessions with an in-flight re-transcribe (a second is a no-op while present).
    pub retranscribing: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Live captures by session id — the authoritative source for a running session's state (the
    /// read layer rewrites a recovered "running" to "interrupted", so the daemon serves live ones here).
    pub sessions: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, Arc<capture_engine::CaptureSession>>,
        >,
    >,
    /// Fired by `POST /v1/admin/shutdown` to gracefully stop `serve`.
    pub shutdown: Arc<Notify>,
    /// The last index vision endpoint the GUI probed reachable (`url`, optional `model`). Set by
    /// `GET /v1/index/status` when available; read at capture start to drive LIVE indexing (#84).
    pub last_index: Arc<std::sync::Mutex<Option<(String, Option<String>)>>>,
    /// Per-session live-index worker stop flags — set `true` on capture stop so the worker finalizes.
    pub live_index_stops:
        Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
}

impl AppState {
    pub fn new(token: String) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(256);
        AppState {
            token: Arc::new(token),
            events,
            indexing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            asr_downloading: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            retranscribing: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            sessions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            shutdown: Arc::new(Notify::new()),
            last_index: Arc::new(std::sync::Mutex::new(None)),
            live_index_stops: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

// ── API error ─────────────────────────────────────────────────────────────────────────────────

/// A handler error rendered as `{"error": message}` with a status — mirrors the Python `_ApiError`
/// (which the handler turns into a JSON `{error}` body).
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        ApiError { status, message: msg.into() }
    }
    pub fn bad(msg: impl Into<String>) -> Self {
        ApiError::new(StatusCode::BAD_REQUEST, msg)
    }
    pub fn unknown_session(id: &str) -> Self {
        ApiError::new(StatusCode::NOT_FOUND, format!("unknown session_id {id:?}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}

// ── Discovery (daemon.json) ───────────────────────────────────────────────────────────────────

/// The user's home dir. **Windows: `%USERPROFILE%`** (matching the GUI/agent's `dirs::home_dir()`):
/// `$HOME` is unset when the app is launched from Explorer/Start Menu/the tray (only a shell sets it),
/// and the old `$HOME`-then-`.` fallback then wrote `~/.capture` — including **daemon.json** — into the
/// process cwd, so the GUI/agent never discovered the running daemon. Falls back to `.` so callers
/// never panic. (mirrors `sessions::home`.)
fn home() -> PathBuf {
    #[cfg(windows)]
    if let Some(p) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expanduser(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        return home().join(rest);
    }
    if s == "~" {
        return home();
    }
    p.to_path_buf()
}

/// The discovery file path: env `CAPTURE_DAEMON_JSON` else `~/.capture/daemon.json`.
/// Mirrors `daemon/server.py::daemon_json_path`.
pub fn daemon_json_path() -> PathBuf {
    match std::env::var_os("CAPTURE_DAEMON_JSON") {
        Some(v) if !v.is_empty() => expanduser(Path::new(&v)),
        _ => home().join(".capture").join("daemon.json"),
    }
}

/// A random bearer token (48 hex chars from 24 OS-random bytes). Mirrors the role of Python's
/// `secrets.token_urlsafe(24)` — an opaque secret the GUI/MCP echo back as `Authorization: Bearer`.
fn gen_token() -> String {
    let mut buf = [0u8; 24];
    getrandom::getrandom(&mut buf).expect("OS randomness is unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Write `daemon.json` at mode 0600 (the token is a secret). Mirrors `write_daemon_json`.
pub fn write_daemon_json(path: &Path, endpoint: &str, token: &str, pid: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let data = serde_json::json!({
        "endpoint": endpoint,
        "token": token,
        "pid": pid,
        "api_version": API_VERSION,
        "version": VERSION,
    });
    let body = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        f.write_all(&body).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, &body).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// True iff `daemon.json` points at a daemon that answers `/v1/health` with `ok:true`.
/// Mirrors `_existing_daemon_alive`. Runs BEFORE the tokio runtime, so blocking reqwest is safe.
pub fn existing_daemon_alive(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(info) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(endpoint) = info.get("endpoint").and_then(|v| v.as_str()) else {
        return false;
    };
    let url = format!("{endpoint}/v1/health");
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build()
    else {
        return false;
    };
    match client.get(&url).send().and_then(|r| r.json::<serde_json::Value>()) {
        Ok(v) => v.get("ok").and_then(|b| b.as_bool()) == Some(true),
        Err(_) => false,
    }
}

/// Platform string for `/v1/health` — mapped to Python's `sys.platform` convention so the wire shape
/// matches (`darwin`/`win32`/…).
pub(crate) fn platform_str() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────────────────────

/// Start the daemon: single-instance guard → bind an ephemeral 127.0.0.1 port → write discovery →
/// serve until shutdown. Mirrors `run_daemon`. Blocking (owns the tokio runtime).
pub fn run() -> Result<(), String> {
    // When the menu-bar agent spawned us (CAPTURE_AGENT=1), exit if it dies — so closing/force-quitting
    // the agent shuts the daemon down too (the macOS analog of the Windows agent's job object). A
    // CLI-started daemon has no such flag and keeps running.
    capture_core::exit_when_parent_dies();
    let disco = daemon_json_path();
    if disco.exists() && existing_daemon_alive(&disco) {
        return Err(format!(
            "a daemon is already running ({}); refusing to start a second",
            disco.display()
        ));
    }
    let token = gen_token();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("build runtime: {e}"))?;
    rt.block_on(serve(token, disco))
}

async fn serve(token: String, disco_path: PathBuf) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("bind 127.0.0.1:0: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let endpoint = format!("http://127.0.0.1:{port}");

    let state = AppState::new(token.clone());
    write_daemon_json(&disco_path, &endpoint, &token, std::process::id())?;
    eprintln!("captured {VERSION} listening on {endpoint} (api {API_VERSION})");

    let shutdown = state.shutdown.clone();
    let app = routes::router(state);
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await
        .map_err(|e| format!("serve: {e}"));

    // Best-effort: remove our discovery file on the way out (mirrors `run_daemon`'s finally).
    let _ = std::fs::remove_file(&disco_path);
    eprintln!("captured stopped");
    result
}

/// Resolve when SIGINT, SIGTERM, or an admin-shutdown notification arrives.
async fn shutdown_signal(notify: Arc<Notify>) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
        _ = notify.notified() => {},
    }
}
