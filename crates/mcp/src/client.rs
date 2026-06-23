//! A thin blocking `/v1` client for the `captured` daemon — a port of `daemon/client.py`.
//!
//! Reads `~/.capture/daemon.json` (or `CAPTURE_DAEMON_JSON`) for the endpoint + bearer token, then
//! issues plain reqwest calls. `available()` is a cheap liveness probe so the MCP can report "no
//! daemon" cleanly. A non-2xx response is turned into a [`DaemonError`] carrying the daemon's
//! `{"error": …}` message, which the tool layer surfaces to the agent (mirrors `_as_value_error`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

/// A daemon HTTP error: the status + the daemon's `error` message (or the HTTP reason).
pub struct DaemonError {
    #[allow(dead_code)] // retained for diagnostics; the tool layer surfaces `message`
    pub status: u16,
    pub message: String,
}

impl DaemonError {
    fn transport(message: impl Into<String>) -> Self {
        // status 0 = the request never reached a daemon response (connect/timeout).
        DaemonError { status: 0, message: message.into() }
    }
}

pub struct DaemonClient {
    endpoint: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl DaemonClient {
    /// Build from `daemon.json` discovery, or `None` when the file is missing/unreadable.
    pub fn from_discovery() -> Option<DaemonClient> {
        let path = daemon_json_path();
        let text = std::fs::read_to_string(path).ok()?;
        let info: Value = serde_json::from_str(&text).ok()?;
        let endpoint = info.get("endpoint")?.as_str()?.trim_end_matches('/').to_string();
        let token = info.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .ok()?;
        Some(DaemonClient { endpoint, token, http })
    }

    /// Cheap liveness probe: `GET /v1/health` returns `ok:true` within 2s.
    pub fn available(&self) -> bool {
        match self.get_timeout("/v1/health", &[], Duration::from_secs(2)) {
            Ok(v) => v.get("ok").and_then(|b| b.as_bool()) == Some(true),
            Err(_) => false,
        }
    }

    // -- transport -------------------------------------------------------------

    pub fn get(&self, route: &str, params: &[(&str, Option<String>)]) -> Result<Value, DaemonError> {
        self.get_timeout(route, params, Duration::from_secs(30))
    }

    fn get_timeout(
        &self,
        route: &str,
        params: &[(&str, Option<String>)],
        timeout: Duration,
    ) -> Result<Value, DaemonError> {
        let mut req = self
            .http
            .get(format!("{}{route}", self.endpoint))
            .bearer_auth(&self.token)
            .timeout(timeout);
        let query: Vec<(&str, String)> =
            params.iter().filter_map(|(k, v)| v.clone().map(|v| (*k, v))).collect();
        if !query.is_empty() {
            req = req.query(&query);
        }
        send(req)
    }

    pub fn post(&self, route: &str, body: Value) -> Result<Value, DaemonError> {
        self.post_timeout(route, body, Duration::from_secs(120))
    }

    pub fn post_timeout(
        &self,
        route: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<Value, DaemonError> {
        let req = self
            .http
            .post(format!("{}{route}", self.endpoint))
            .bearer_auth(&self.token)
            .timeout(timeout)
            .json(&body);
        send(req)
    }
}

/// Send a request, mapping a non-2xx into a [`DaemonError`] with the daemon's `error` message.
fn send(req: reqwest::blocking::RequestBuilder) -> Result<Value, DaemonError> {
    let resp = req.send().map_err(|e| DaemonError::transport(format!("daemon unreachable: {e}")))?;
    let status = resp.status();
    let body: Value = resp.json().unwrap_or(Value::Null);
    if status.is_success() {
        Ok(body)
    } else {
        let message = body
            .get("error")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string());
        Err(DaemonError { status: status.as_u16(), message })
    }
}

// -- discovery path (mirrors daemon/server.py::daemon_json_path) ----------------------------------

fn home() -> PathBuf {
    // Windows: prefer %USERPROFILE% to match the daemon/GUI's home resolution (dirs::home_dir());
    // $HOME is unset when launched outside a shell, so the MCP must look where the daemon wrote
    // daemon.json or it can't discover it.
    #[cfg(windows)]
    if let Some(p) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// `CAPTURE_DAEMON_JSON` else `~/.capture/daemon.json`.
pub fn daemon_json_path() -> PathBuf {
    match std::env::var_os("CAPTURE_DAEMON_JSON") {
        Some(v) if !v.is_empty() => {
            let p = Path::new(&v);
            if let Some(rest) = p.to_string_lossy().strip_prefix("~/") {
                home().join(rest)
            } else {
                p.to_path_buf()
            }
        }
        _ => home().join(".capture").join("daemon.json"),
    }
}
