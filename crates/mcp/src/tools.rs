//! The capture tool surface (a port of `capture_mcp/server.py`'s 12 `@mcp.tool`s).
//!
//! v3 is **daemon-first with no embedded engine** (capture/asr/platform land in #64/#65/#66): every
//! tool proxies to the running `captured` daemon's `/v1`. With no daemon the tool returns a clear
//! error; an engine route that isn't ported yet returns the daemon's `501` message. The READY tools
//! today are `capture_status` (session read) and `capture_index` (the real build) — the rest light up
//! automatically as the daemon's engine routes land. Tool descriptions are the agent's contract, so
//! they're ported faithfully from the Python docstrings.

use serde_json::{json, Map, Value};

use crate::client::{DaemonClient, DaemonError};

/// One MCP tool: its name, the description shown to the agent, and the JSON-Schema for its arguments.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

const PRUNE_PARTS: [&str; 3] = ["screenshots", "screenshots_halve", "audio"];

// ── tools/list ──────────────────────────────────────────────────────────────────────────────────

/// The full tool surface for `tools/list`.
pub fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "capture_start",
            description: r#"Start capturing a process. Returns a session summary including `session_id`.

Specify the target ONE of three ways:
  * `command` — a shell command to launch; its stdout/stderr are captured (the only mode that captures logs) and its window/audio are tracked once it appears.
  * `pid` — attach to an already-running process by PID.
  * `app_name` — attach by case-insensitive substring of the app's name (e.g. "Safari"); its main on-screen window is used.

Artifacts are written under `<output_dir>/capture-<id>/`: timestamped screenshots (every `screenshot_interval` s), stdout/stderr logs, raw `audio.s16le`, and `transcript.jsonl`/`.txt` with each speech segment stamped with the absolute time it was spoken."#,
            input_schema: obj(
                json!({
                    "output_dir": prop("string", "Base directory for the session folder (created if missing)."),
                    "command": prop("string", "Command line to launch and capture (mutually exclusive with pid/app_name)."),
                    "pid": prop("integer", "PID of a running process to attach to."),
                    "app_name": prop("string", "App name substring to attach to (e.g. \"Safari\")."),
                    "window_id": prop("integer", "Pin screenshots to this exact window (a window_id from list_windows) — needed when one process owns several windows. Audio stays per-process."),
                    "bundle_id": prop("string", "Bundle id for per-app audio (e.g. \"com.apple.Safari\"); optional."),
                    "preset": prop("string", "Capture preset (#54): \"meeting\", \"coding\", \"lecture\", \"auto\", \"general\", or \"custom\" — records intent + the index preset a later index defaults to. Leave unset for plain capture."),
                    "screenshot_interval": prop("number", "Seconds between screenshots (default 1.0)."),
                    "screenshot_format": prop("string", "Image format: png (default), jpg/jpeg, tiff, gif, bmp."),
                    "screenshot_resolution": prop("string", "Bounding box \"WxH\" (e.g. \"1280x720\"); shots scale to fit, never upscaled. May include the format, e.g. \"1280x720/jpg\"."),
                    "screenshot_jpeg_quality": prop("integer", "JPEG quality 0-100 (only when format is jpg)."),
                    "capture_screenshots": prop("boolean", "Capture window screenshots (default true)."),
                    "capture_audio": prop("boolean", "Capture + transcribe audio (default true)."),
                    "audio_source": prop("string", "\"auto\" (per-app helper, else mic), \"app\", or \"mic\"."),
                    "mic_device": prop("string", "Also record a microphone as a SEPARATE track (a device id from list_audio_devices, or \"default\"). Echo cancellation removes laptop-speaker bleed."),
                    "audio_chunk_seconds": prop("number", "Audio window size sent to ASR per pass (default 8.0)."),
                    "asr_backend": prop("string", "\"auto\", \"local\"/\"whisper\", or \"nemotron\"/\"riva\"."),
                    "cwd": prop("string", "Working directory for a launched command.")
                }),
                &["output_dir"],
            ),
        },
        ToolDef {
            name: "capture_stop",
            description: r#"Stop a capture and flush everything to disk. Returns the final session summary.

If `session_id` is omitted and exactly one capture is running, that one is stopped; if several are running, an error lists them (pass an explicit id). Use `capture_status` to see ids."#,
            input_schema: obj(
                json!({ "session_id": prop("string", "The session to stop. Omit to stop the single running capture.") }),
                &[],
            ),
        },
        ToolDef {
            name: "capture_status",
            description: r#"Report capture status.

With `session_id`, returns that session's summary; otherwise returns a list of all sessions — those created plus finished ones recovered from the on-disk index. Each summary carries counters (`screenshots`, `transcript_segments`) and capability flags (`has_screenshots`, `has_audio`, `has_mic`, `can_retranscribe`, `can_index`)."#,
            input_schema: obj(
                json!({ "session_id": prop("string", "If given, return that session's summary; otherwise list all sessions.") }),
                &[],
            ),
        },
        ToolDef {
            name: "capture_prune",
            description: r#"Free disk on a FINISHED capture by removing artifacts; returns freed bytes + the refreshed capability flags.

`parts`: any of "screenshots" (delete all screenshots), "screenshots_halve" (drop every other frame — half the cadence, full timeline), "audio" (remove the raw audio.s16le/mic.s16le — frees the most disk but disables `capture_retranscribe`)."#,
            input_schema: obj(
                json!({
                    "session_id": prop("string", "The session to prune (must be stopped)."),
                    "parts": json!({ "type": "array", "items": {"type": "string", "enum": PRUNE_PARTS}, "description": "Non-empty subset of screenshots / screenshots_halve / audio." })
                }),
                &["session_id", "parts"],
            ),
        },
        ToolDef {
            name: "capture_retranscribe",
            description: r#"Re-transcribe a saved capture's audio, replacing its transcript — e.g. to upgrade an old session with a stronger model, FIX a wrong-language transcript, or re-chunk it. Requires the raw audio still present (`can_retranscribe`). Runs in the background; watch `capture_status` `transcript_segments`."#,
            input_schema: obj(
                json!({
                    "session_id": prop("string", "The session to re-transcribe (must be stopped, with audio)."),
                    "asr_backend": prop("string", "\"auto\" (default), \"local\"/\"whisper\", or \"nemotron\"/\"riva\"."),
                    "model": prop("string", "Optional Whisper repo to switch to first (e.g. \"mlx-community/whisper-large-v3-turbo\")."),
                    "language": prop("string", "ISO code to pin (e.g. \"ru\", \"en\") to fix mis-detected speech; \"\"/\"auto\" = auto-detect."),
                    "chunk_seconds": prop("number", "Transcription window in seconds (default 30). ≥24 s avoids short-chunk hallucination.")
                }),
                &["session_id"],
            ),
        },
        ToolDef {
            name: "transcription_settings",
            description: r#"Get or set the persisted transcription settings shared by all new captures + re-transcribes.

Call with no args to read the current settings; pass a value to change it. A `language` change applies on a running capture's next chunk, so you can correct a live transcript without restarting. Returns the current `{language, chunk_seconds, active_model, backend_available}`."#,
            input_schema: obj(
                json!({
                    "language": prop("string", "ISO code to pin (e.g. \"ru\", \"en\"); \"\"/\"auto\" = auto-detect."),
                    "chunk_seconds": prop("number", "Transcription window in seconds (1–120; default 30).")
                }),
                &[],
            ),
        },
        ToolDef {
            name: "capture_import",
            description: r#"Import an existing audio or video file as a capture session.

Turns a recording you already have (a meeting capture, a screen recording, a voice memo) into a normal session — extracts its audio (and, for video, periodic frames), runs ASR, and registers it so it shows in `capture_status` and the GUI's playback scrubber. Runs in the background. Audio-only files become audio-only sessions."#,
            input_schema: obj(
                json!({
                    "path": prop("string", "Absolute path to a local audio/video file (anything AVFoundation decodes — .m4a/.mp3/.wav/.mov/.mp4/…)."),
                    "output_dir": prop("string", "Where to create the session dir (defaults to the daemon's runs dir)."),
                    "asr_backend": prop("string", "\"auto\" (default), \"local\"/\"whisper\", or \"nemotron\"/\"riva\".")
                }),
                &["path"],
            ),
        },
        ToolDef {
            name: "capture_index",
            description: r#"Build a multimodal index of a finished capture's screenshots with a remote vision LLM.

Captions the session's screenshots and summarizes the timeline as a binary tree (leaf captions → range summaries → a whole-session root), so the session becomes readable at any zoom level. Runs in the background; fetch the tree afterward from the daemon (`GET /v1/sessions/{id}/index`). Requires `can_index` (screenshots present) and a configured, reachable vision endpoint — disabled unless an LM Studio server is set (via `CAPTURE_INDEX_URL` or the `endpoint` arg).

You (a frontier model) can craft a `leaf_prompt` (+ optional `leaf_schema`) tailored to this session; the cheap local vision model executes it on every frame. The prompts you pass are saved to `<session>/index_prompts.json` so good ones can be folded into the built-in extractors."#,
            input_schema: obj(
                json!({
                    "session_id": prop("string", "The session to index (must be stopped, with screenshots)."),
                    "endpoint": prop("string", "Full chat URL override (e.g. http://host:1234/v1/chat/completions). Takes precedence over provider/host/port."),
                    "provider": prop("string", "Structured provider id: \"lmstudio\" (1234), \"ollama\" (11434), \"openai\" (cloud, needs a key), or \"custom\" (host = full base URL)."),
                    "host": prop("string", "Hostname/IP for the provider (or the full base URL for \"custom\")."),
                    "port": prop("integer", "Port for the provider (defaults to the provider's standard port)."),
                    "model": prop("string", "Model id override (e.g. \"qwen3.5-9b\")."),
                    "sample_rate": prop("number", "Leaf sampling rate in (0,1] (default 0.5 = caption every other frame)."),
                    "prompt_preset": prop("string", "Per-frame handling — \"auto\" (classify each frame) or a fixed type: \"meeting\", \"lecture\", \"coding\", \"browsing\", …"),
                    "leaf_prompt": prop("string", "A CUSTOM per-frame prompt. With leaf_schema it's a structured extractor; alone it's a free-text caption."),
                    "leaf_schema": json!({ "type": "object", "description": "A JSON Schema (object with a `summary` string + your fields) the local model returns per frame." }),
                    "classify_prompt": prop("string", "A CUSTOM classifier prompt (overrides the default content-type classifier)."),
                    "max_px": prop("integer", "Base longest-edge image downscale (default 1024). Code/terminal frames auto-bump to 2048; raise for a code-heavy session (study: 1024→2048 took UE code fidelity 0.42→0.88).")
                }),
                &["session_id"],
            ),
        },
        ToolDef {
            name: "index_models",
            description: r#"List the vision-LLM models a provider has available (to pick `model` for `capture_index`).

GETs the provider's `/v1/models`. Pass a structured config (`provider` = lmstudio/ollama/openai/custom + `host` + `port`) or a full `url`. Returns `{models, provider, reachable}` — `reachable: false` with an empty list if the endpoint can't be reached or needs a `key`."#,
            input_schema: obj(
                json!({
                    "provider": prop("string", "lmstudio/ollama/openai/custom."),
                    "host": prop("string", "Hostname/IP (or full base URL for custom)."),
                    "port": prop("integer", "Provider port."),
                    "key": prop("string", "API key (for cloud providers)."),
                    "url": prop("string", "A full base/chat URL instead of provider/host/port.")
                }),
                &[],
            ),
        },
        ToolDef {
            name: "list_windows",
            description: r#"List on-screen top-level windows (the picker for capture targets).

Each entry has `window_id`, `pid`, `app_name`, `title`, `width`, `height`, ordered largest-first (the first match is what `capture_start` would target)."#,
            input_schema: obj(
                json!({
                    "app_name": prop("string", "Optional case-insensitive substring filter (e.g. \"Safari\")."),
                    "pid": prop("integer", "Optional process id filter.")
                }),
                &[],
            ),
        },
        ToolDef {
            name: "list_audio_devices",
            description: r#"List microphone/input devices for `capture_start`'s `mic_device`.

Returns `{devices: [{id, name, default}]}`. Pass a device `id` (or "default") as `mic_device` to record that microphone as a separate track. macOS-only for now (other platforms return an empty list)."#,
            input_schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "capture_set_mic",
            description: r#"Switch the microphone on a RUNNING capture, live (no restart). The mic is recorded as a separate track (`mic.s16le` / `mic_transcript.*`); switching appends to it so the recording stays continuous.

`device`: an input-device id from `list_audio_devices` (or "default") turns the mic on / switches it; null / "" turns the mic OFF. Returns the updated session summary."#,
            input_schema: obj(
                json!({
                    "session_id": prop("string", "The running session to change."),
                    "device": prop("string", "Input-device id (or \"default\") to turn on/switch; null/\"\" to turn off.")
                }),
                &["session_id"],
            ),
        },
    ]
}

/// `{ "type": "object", "properties": props, "required": required }`.
fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// `{ "type": ty, "description": desc }` — one schema property.
fn prop(ty: &str, desc: &str) -> Value {
    json!({ "type": ty, "description": desc })
}

// ── tools/call dispatch ─────────────────────────────────────────────────────────────────────────

/// Run a tool by name. `Ok(value)` is the tool's JSON result; `Err(message)` becomes an
/// `isError: true` tool result the agent sees (mirrors FastMCP's ValueError handling).
pub fn dispatch(daemon: Option<&DaemonClient>, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "capture_start" => capture_start(daemon, args),
        "capture_stop" => capture_stop(daemon, args),
        "capture_status" => capture_status(daemon, args),
        "capture_prune" => capture_prune(daemon, args),
        "capture_retranscribe" => capture_retranscribe(daemon, args),
        "transcription_settings" => transcription_settings(daemon, args),
        "capture_import" => capture_import(daemon, args),
        "capture_index" => capture_index(daemon, args),
        "index_models" => index_models(daemon, args),
        "list_windows" => list_windows(daemon, args),
        "list_audio_devices" => list_audio_devices(daemon),
        "capture_set_mic" => capture_set_mic(daemon, args),
        other => Err(format!("unknown tool {other:?}")),
    }
}

// -- arg helpers ----------------------------------------------------------------------------------

fn s(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(String::from)
}
/// A non-blank string arg (mirrors the Python `_present` for string fields).
fn sne(args: &Value, key: &str) -> Option<String> {
    s(args, key).filter(|x| !x.trim().is_empty())
}
fn i(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64())
}
fn f(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}
fn bln(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}

fn require(daemon: Option<&DaemonClient>) -> Result<&DaemonClient, String> {
    daemon.ok_or_else(|| {
        "the capture daemon isn't running — start it (open the Capture app, or run `captured`)".to_string()
    })
}

/// Surface a daemon HTTP error as the agent-visible message (the daemon's `error` text).
fn de(e: DaemonError) -> String {
    e.message
}

// -- tools ----------------------------------------------------------------------------------------

fn capture_start(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    // Validate the request shape BEFORE touching the daemon (mirrors the Python order).
    let output_dir = sne(args, "output_dir").ok_or("output_dir is required")?;

    // Exactly one target: command / pid / app_name (pid=0 counts as provided, rejected by the daemon).
    let mut provided = Vec::new();
    if sne(args, "command").is_some() {
        provided.push("command");
    }
    if i(args, "pid").is_some() {
        provided.push("pid");
    }
    if sne(args, "app_name").is_some() {
        provided.push("app_name");
    }
    if provided.len() != 1 {
        return Err(format!(
            "specify exactly one target: command, pid, or app_name (got: {})",
            if provided.is_empty() { "none".to_string() } else { provided.join(", ") }
        ));
    }

    let d = require(daemon)?;
    let body = json!({
        "output_dir": output_dir,
        "command": args.get("command").cloned().unwrap_or(Value::Null),
        "pid": args.get("pid").cloned().unwrap_or(Value::Null),
        "window_id": args.get("window_id").cloned().unwrap_or(Value::Null),
        "app_name": args.get("app_name").cloned().unwrap_or(Value::Null),
        "bundle_id": args.get("bundle_id").cloned().unwrap_or(Value::Null),
        "screenshot_interval": f(args, "screenshot_interval").unwrap_or(1.0),
        "screenshot_format": s(args, "screenshot_format").unwrap_or_else(|| "png".into()),
        "screenshot_resolution": args.get("screenshot_resolution").cloned().unwrap_or(Value::Null),
        "screenshot_jpeg_quality": args.get("screenshot_jpeg_quality").cloned().unwrap_or(Value::Null),
        "capture_screenshots": bln(args, "capture_screenshots").unwrap_or(true),
        "capture_audio": bln(args, "capture_audio").unwrap_or(true),
        "audio_source": s(args, "audio_source").unwrap_or_else(|| "auto".into()),
        "mic_device": args.get("mic_device").cloned().unwrap_or(Value::Null),
        "audio_chunk_seconds": f(args, "audio_chunk_seconds").unwrap_or(8.0),
        "asr_backend": s(args, "asr_backend").unwrap_or_else(|| "auto".into()),
        "cwd": args.get("cwd").cloned().unwrap_or(Value::Null),
        "preset": args.get("preset").cloned().unwrap_or(Value::Null),
    });
    // start blocks on ASR model load on first use → long timeout (mirrors client.start).
    d.post_timeout("/v1/sessions", body, std::time::Duration::from_secs(600)).map_err(de)
}

fn capture_stop(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    let sid = match sne(args, "session_id") {
        Some(id) => id,
        None => {
            let sessions = d.get("/v1/sessions", &[]).map_err(de)?;
            let running: Vec<&Value> = sessions
                .get("sessions")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter(|s| s.get("state") == Some(&json!("running"))).collect())
                .unwrap_or_default();
            if running.is_empty() {
                return Ok(json!({ "stopped": [], "note": "no running captures" }));
            }
            if running.len() > 1 {
                let ids: Vec<&str> =
                    running.iter().filter_map(|s| s.get("session_id").and_then(|v| v.as_str())).collect();
                return Err(format!(
                    "multiple captures running; pass session_id. Running: {}",
                    ids.join(", ")
                ));
            }
            running[0].get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
    };
    d.post_timeout(&format!("/v1/sessions/{sid}/stop"), json!({}), std::time::Duration::from_secs(120))
        .map_err(de)
}

fn capture_status(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    match sne(args, "session_id") {
        Some(id) => d.get(&format!("/v1/sessions/{id}"), &[]).map_err(de),
        None => d.get("/v1/sessions", &[]).map_err(de),
    }
}

fn capture_prune(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let sid = sne(args, "session_id").ok_or("session_id is required")?;
    let parts: Vec<String> = args
        .get("parts")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let bad: Vec<&String> = parts.iter().filter(|p| !PRUNE_PARTS.contains(&p.as_str())).collect();
    if parts.is_empty() || !bad.is_empty() {
        return Err(format!(
            "parts must be a non-empty subset of {PRUNE_PARTS:?}; bad: {bad:?}"
        ));
    }
    let d = require(daemon)?;
    d.post(&format!("/v1/sessions/{sid}/prune"), json!({ "parts": parts })).map_err(de)
}

fn capture_retranscribe(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    let sid = sne(args, "session_id").ok_or("session_id is required")?;
    let mut body = Map::new();
    if let Some(v) = sne(args, "asr_backend") {
        body.insert("asr_backend".into(), json!(v));
    }
    if let Some(v) = sne(args, "model") {
        body.insert("model".into(), json!(v));
    }
    // language: include when present (even ""), to clear/auto-detect (mirrors `is not None`).
    if let Some(v) = args.get("language").filter(|v| v.is_string()) {
        body.insert("language".into(), v.clone());
    }
    if let Some(v) = f(args, "chunk_seconds") {
        body.insert("chunk_seconds".into(), json!(v));
    }
    d.post(&format!("/v1/sessions/{sid}/retranscribe"), Value::Object(body)).map_err(de)
}

fn transcription_settings(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    if let Some(lang) = args.get("language").filter(|v| v.is_string()) {
        d.post("/v1/asr/language", json!({ "language": lang.as_str().unwrap_or("") })).map_err(de)?;
    }
    if let Some(cs) = f(args, "chunk_seconds") {
        d.post("/v1/asr/chunk", json!({ "seconds": cs })).map_err(de)?;
    }
    let models = d.get("/v1/asr/models", &[]).map_err(de)?;
    Ok(json!({
        "language": models.get("language").cloned().unwrap_or(Value::Null),
        "chunk_seconds": models.get("chunk_seconds").cloned().unwrap_or(Value::Null),
        "active_model": models.get("active").cloned().unwrap_or(Value::Null),
        "backend_available": models.get("backend_available").cloned().unwrap_or(Value::Null),
    }))
}

fn capture_import(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    let path = sne(args, "path").ok_or("path is required")?;
    let mut body = Map::new();
    body.insert("path".into(), json!(path));
    if let Some(v) = sne(args, "output_dir") {
        body.insert("output_dir".into(), json!(v));
    }
    if let Some(v) = sne(args, "asr_backend") {
        body.insert("asr_backend".into(), json!(v));
    }
    d.post("/v1/sessions/import", Value::Object(body)).map_err(de)
}

fn capture_index(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    let sid = sne(args, "session_id").ok_or("session_id is required")?;
    let mut body = Map::new();
    if let Some(v) = sne(args, "provider") {
        body.insert("provider".into(), json!(v));
    }
    if let Some(v) = sne(args, "host") {
        body.insert("host".into(), json!(v));
    }
    if let Some(v) = i(args, "port") {
        body.insert("port".into(), json!(v));
    }
    if let Some(v) = sne(args, "endpoint") {
        body.insert("endpoint".into(), json!(v));
    }
    if let Some(v) = sne(args, "model") {
        body.insert("model".into(), json!(v));
    }
    if let Some(v) = f(args, "sample_rate") {
        body.insert("sample_rate".into(), json!(v));
    }
    if let Some(v) = sne(args, "prompt_preset") {
        body.insert("prompt_preset".into(), json!(v));
    }
    if let Some(v) = sne(args, "leaf_prompt") {
        body.insert("leaf_prompt".into(), json!(v));
    }
    if let Some(v) = args.get("leaf_schema").filter(|v| v.is_object()) {
        body.insert("leaf_schema".into(), v.clone());
    }
    if let Some(v) = sne(args, "classify_prompt") {
        body.insert("classify_prompt".into(), json!(v));
    }
    if let Some(v) = i(args, "max_px") {
        body.insert("max_px".into(), json!(v));
    }
    d.post(&format!("/v1/sessions/{sid}/index"), Value::Object(body)).map_err(de)
}

fn index_models(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    let params = [
        ("provider", sne(args, "provider")),
        ("host", sne(args, "host")),
        ("port", i(args, "port").map(|p| p.to_string())),
        ("key", sne(args, "key")),
        ("url", sne(args, "url")),
    ];
    d.get("/v1/index/models", &params).map_err(de)
}

fn list_windows(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    let params = [
        ("app_name", sne(args, "app_name")),
        ("pid", i(args, "pid").map(|p| p.to_string())),
    ];
    d.get("/v1/windows", &params).map_err(de)
}

fn list_audio_devices(daemon: Option<&DaemonClient>) -> Result<Value, String> {
    let d = require(daemon)?;
    d.get("/v1/audio/mics", &[]).map_err(de)
}

fn capture_set_mic(daemon: Option<&DaemonClient>, args: &Value) -> Result<Value, String> {
    let d = require(daemon)?;
    let sid = sne(args, "session_id").ok_or("session_id is required")?;
    // device: a string id / "default" = on, null / "" = off (passed through verbatim).
    let device = args.get("device").cloned().unwrap_or(Value::Null);
    d.post(&format!("/v1/sessions/{sid}/mic"), json!({ "device": device })).map_err(de)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_surface_is_the_twelve() {
        let names: Vec<&str> = tools().iter().map(|t| t.name).collect();
        assert_eq!(names.len(), 12);
        for expected in [
            "capture_start", "capture_stop", "capture_status", "capture_prune",
            "capture_retranscribe", "transcription_settings", "capture_import",
            "capture_index", "index_models", "list_windows", "list_audio_devices",
            "capture_set_mic",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
    }

    #[test]
    fn every_tool_has_an_object_schema_with_descriptions() {
        for t in tools() {
            assert_eq!(t.input_schema["type"], json!("object"), "{} schema", t.name);
            assert!(t.input_schema.get("properties").is_some(), "{} has properties", t.name);
            assert!(!t.description.is_empty(), "{} has a description", t.name);
        }
    }

    #[test]
    fn no_daemon_yields_a_clear_error() {
        // Every tool requires the daemon in v3 (no embedded engine yet).
        let err = dispatch(None, "capture_status", &json!({})).unwrap_err();
        assert!(err.contains("daemon isn't running"), "got: {err}");
    }

    #[test]
    fn capture_start_validates_target_before_the_daemon() {
        // No target → the request-shape error, even with no daemon (validation runs first).
        let err = dispatch(None, "capture_start", &json!({"output_dir": "/x"})).unwrap_err();
        assert!(err.contains("exactly one target"), "got: {err}");
        // Two targets → also rejected.
        let err = dispatch(None, "capture_start", &json!({"output_dir": "/x", "pid": 1, "command": "echo"})).unwrap_err();
        assert!(err.contains("exactly one target"), "got: {err}");
    }

    #[test]
    fn capture_prune_validates_parts_before_the_daemon() {
        let err = dispatch(None, "capture_prune", &json!({"session_id": "x", "parts": []})).unwrap_err();
        assert!(err.contains("non-empty subset"), "got: {err}");
        let err = dispatch(None, "capture_prune", &json!({"session_id": "x", "parts": ["bogus"]})).unwrap_err();
        assert!(err.contains("bogus"), "got: {err}");
    }

    #[test]
    fn unknown_tool_errors() {
        let err = dispatch(None, "nope", &json!({})).unwrap_err();
        assert!(err.contains("unknown tool"), "got: {err}");
    }
}
