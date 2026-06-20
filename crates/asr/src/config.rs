//! The shared settings file `~/.capture/config.json` (read/write, preserving unrelated keys) — the
//! ASR settings the GUI and both daemons share. A 1:1 home for the keys the Python reads/writes
//! (`asr_runtime`, `whisper_model`, `whisper_language`). `CAPTURE_CONFIG` overrides the path (test).

use std::path::PathBuf;

use serde_json::{Map, Value};

/// Config key: the active ASR runtime id (`registry` id, or absent = ASR off).
pub const ASR_RUNTIME: &str = "asr_runtime";
/// Config key: the active model (a GGML model id for the whisper runtime).
pub const WHISPER_MODEL: &str = "whisper_model";
/// Config key: the pinned transcription language (ISO code, `""`/absent = auto).
pub const WHISPER_LANGUAGE: &str = "whisper_language";
/// Config key: the transcription chunk length in seconds (number; absent = the default).
pub const AUDIO_CHUNK_SECONDS: &str = "audio_chunk_seconds";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// `CAPTURE_CONFIG` else `~/.capture/config.json`.
pub fn config_path() -> PathBuf {
    match std::env::var_os("CAPTURE_CONFIG") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(".capture").join("config.json"),
    }
}

/// The whole config object (empty when absent/unparseable).
pub fn read() -> Map<String, Value> {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// A non-empty string setting, or `None`.
pub fn get_str(key: &str) -> Option<String> {
    read()
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// A numeric setting (a JSON number, or a numeric string), or `None`.
pub fn get_f64(key: &str) -> Option<f64> {
    let v = read();
    let v = v.get(key)?;
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

/// Set a string setting, preserving every other key in the file.
pub fn set_str(key: &str, value: &str) -> Result<(), String> {
    set_value(key, Value::String(value.to_string()))
}

/// Set a numeric setting (written as a JSON number, matching what the Python daemon reads).
pub fn set_f64(key: &str, value: f64) -> Result<(), String> {
    let n = serde_json::Number::from_f64(value)
        .ok_or_else(|| format!("non-finite value for {key}"))?;
    set_value(key, Value::Number(n))
}

/// Insert/replace one key, preserving every other key in the file.
fn set_value(key: &str, value: Value) -> Result<(), String> {
    let mut cfg = read();
    cfg.insert(key.to_string(), value);
    let path = config_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
    }
    let body = serde_json::to_vec_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))
}
