//! `capture-mcp` — the v3 MCP stdio server (a port of `capture_mcp/server.py`).
//!
//! Speaks line-delimited JSON-RPC 2.0 over stdin/stdout (the MCP stdio transport): `initialize`,
//! `tools/list`, `tools/call`, `ping`, and notifications. Each tool proxies to the running `captured`
//! daemon's `/v1` (daemon-first; no embedded engine in v3 yet — see [`tools`]). **stdout is the
//! transport — all logs go to stderr.**

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

mod client;
mod tools;

fn main() {
    eprintln!("capture-mcp {} — stdio JSON-RPC; proxies to the captured daemon", env!("CARGO_PKG_VERSION"));
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            // Unparseable line: we can't recover an id, so we cannot respond per JSON-RPC. Skip.
            eprintln!("capture-mcp: ignoring unparseable line");
            continue;
        };
        if let Some(resp) = handle(&msg) {
            // One JSON object per line; never anything else on stdout.
            let _ = writeln!(out, "{resp}");
            let _ = out.flush();
        }
    }
}

/// Dispatch one JSON-RPC message. Returns the response to write, or `None` for a notification
/// (no `id` ⇒ no response, per JSON-RPC).
fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

    // Notifications (no id) — `notifications/initialized`, `notifications/cancelled`, … — are
    // fire-and-forget; we have no side effects to run, so just acknowledge by doing nothing.
    let id = id?;

    let response = match method {
        "initialize" => result(id, initialize_result(&params)),
        "tools/list" => result(id, tools_list_result()),
        "tools/call" => result(id, tools_call_result(&params)),
        "ping" => result(id, json!({})),
        other => error(id, -32601, &format!("method not found: {other}")),
    };
    Some(response)
}

fn result(id: Value, value: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": value })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// `initialize` — advertise the tools capability; echo the client's protocol version when given.
fn initialize_result(params: &Value) -> Value {
    let proto = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("2024-11-05");
    json!({
        "protocolVersion": proto,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "capture-mcp", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tools_list_result() -> Value {
    let list: Vec<Value> = tools::tools()
        .into_iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
        .collect();
    json!({ "tools": list })
}

/// `tools/call` — run the tool. A tool error is reported as an `isError: true` RESULT (so the model
/// sees the message), NOT a JSON-RPC protocol error (mirrors FastMCP).
fn tools_call_result(params: &Value) -> Value {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let daemon = current_daemon();

    match tools::dispatch(daemon.as_ref(), name, &args) {
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
            let mut res = json!({ "content": [ { "type": "text", "text": text } ], "isError": false });
            // structuredContent must be an object — include it only when the result is one.
            if value.is_object() {
                res["structuredContent"] = value;
            }
            res
        }
        Err(message) => {
            json!({ "content": [ { "type": "text", "text": message } ], "isError": true })
        }
    }
}

/// The live daemon for this call, or `None`. Re-checked per call (cheap `/v1/health` probe) so a
/// daemon started/stopped mid-session is picked up. `CAPTURE_MCP_EMBEDDED` forces "no daemon"
/// (v3 has no embedded engine yet, so tools then report the daemon is required).
fn current_daemon() -> Option<client::DaemonClient> {
    if std::env::var_os("CAPTURE_MCP_EMBEDDED").is_some() {
        return None;
    }
    let c = client::DaemonClient::from_discovery()?;
    c.available().then_some(c)
}
