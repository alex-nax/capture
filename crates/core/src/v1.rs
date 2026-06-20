//! The `/v1` HTTP API contract types (serde) — the v3 source of truth.
//!
//! These replace the v2 pydantic `daemon/models.py` + `v1_schema` golden. Two halves:
//!
//! - **Response/data types** were MOVED verbatim from `gui/src/daemon.rs` (the proven
//!   deserializers that already decode the live v2 daemon). They stay LENIENT — `#[serde(default)]`
//!   on optional fields, NO `deny_unknown_fields` — so the GUI tolerates benign daemon additions.
//!   They now ALSO derive `Serialize` because capture-core is the contract source (the future Rust
//!   daemon serializes them). Names match the GUI's call sites (`daemon::Session`, …) and stay there
//!   via the glob re-export in `gui/src/daemon.rs`.
//! - **Request types** are PORTED from `models.py`. Each uses `#[serde(deny_unknown_fields)]`
//!   (mirrors pydantic `extra="forbid"`); pydantic defaults become serde defaults via helper fns.
//!
//! Wire-name map (GUI name ≈ pydantic name; the JSON is identical, kept under the GUI name to avoid
//! churn): `Session` ≈ `SessionSummary`, `AsrModels` ≈ `AsrModelsResponse`, `AsrModel` ≈ `AsrModelInfo`.

use serde::{Deserialize, Serialize};

// ── Response / data types (moved verbatim from gui/src/daemon.rs) ──────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub api_version: String,
    pub pid: u32,
}

/// The session summary — the full `SessionSummary` wire shape (mirrors pydantic
/// `models.py::SessionSummary` + `session.py`). The Rust daemon SERIALIZES the complete shape
/// (serde emits every field), while the GUI DESERIALIZES it leniently: every optional field carries
/// a `#[serde(default)]` so a partial body (or a benign daemon addition) still decodes. The GUI only
/// renders a subset; the rest ride along so the daemon's `/v1/sessions` matches the v2 contract.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[allow(dead_code)] // the GUI renders a subset; the rest exist for the /v1 wire shape
pub struct Session {
    pub session_id: String,
    pub state: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub pid: Option<i64>,
    #[serde(default)]
    pub window_title: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub stopped_at: Option<String>,
    #[serde(default)]
    pub screenshots: i64,
    #[serde(default)]
    pub screenshot_errors: i64,
    #[serde(default)]
    pub log_lines: i64,
    #[serde(default)]
    pub process_running: Option<bool>,
    #[serde(default)]
    pub audio_mode: String,
    #[serde(default)]
    pub audio_status: String,
    #[serde(default)]
    pub transcript_segments: i64,
    #[serde(default)]
    pub asr_errors: i64,
    #[serde(default = "default_off")]
    pub mic_status: String,
    #[serde(default)]
    pub mic_segments: i64,
    #[serde(default)]
    pub mic_device: Option<String>, // active mic input id (None = off), for the live switcher
    #[serde(default)]
    pub capture_preset: Option<String>, // the #54 capture preset recorded at start
    #[serde(default)]
    pub index_preset: Option<String>, // the index preset a later build defaults to
    // Capability flags (what the session can still do, disk-computed by the daemon).
    #[serde(default = "default_true")]
    pub has_screenshots: bool,
    #[serde(default = "default_true")]
    pub has_audio: bool,
    #[serde(default)]
    pub has_mic: bool,
    #[serde(default = "default_true")]
    pub can_retranscribe: bool,
    #[serde(default = "default_true")]
    pub can_index: bool,
    #[serde(default)]
    pub notes: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_off() -> String {
    "off".into()
}

/// Availability of the multimodal-index endpoint (GET /v1/index/status). Extra response
/// fields (url/model) are ignored — the GUI only needs the gate.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct IndexStatus {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub configured: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[allow(dead_code)] // mirrors the /v1 WindowInfo wire shape (window_id/width/height for slice 2)
pub struct WindowInfo {
    pub window_id: i64,
    pub pid: i64,
    pub app_name: String,
    pub title: String,
    pub width: i64,
    pub height: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TranscriptSeg {
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AsrModel {
    pub repo: String,
    pub name: String,
    pub size_label: String,
    pub downloaded: bool,
    pub active: bool,
    #[serde(default)]
    pub downloading: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Permissions {
    #[serde(default)]
    #[allow(dead_code)] // wire shape; UI keys off the per-permission fields
    pub platform: String,
    #[serde(default)]
    pub screen_recording: String,
    #[serde(default)]
    pub microphone: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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

/// A selectable ASR runtime (GET /v1/asr/runtimes) — engine + hardware requirement + state.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[allow(dead_code)] // mirrors the wire shape; the UI reads a subset
pub struct AsrRuntime {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub requires: String,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub active: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AsrGpu {
    #[serde(default)]
    pub nvidia: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AsrRuntimes {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub gpu: AsrGpu,
    #[serde(default)]
    pub runtimes: Vec<AsrRuntime>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub default: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AudioDevices {
    #[serde(default)]
    pub devices: Vec<AudioDevice>,
}

// ── Request types (ported from src/capture_mcp/daemon/models.py) ───────────────────────────────
//
// Each derives Serialize + Deserialize and uses `deny_unknown_fields` (pydantic `extra="forbid"`).
// Pydantic field defaults are reproduced as serde defaults via the `d_*` helpers below.

fn d_png() -> String {
    "png".into()
}
fn d_auto() -> String {
    "auto".into()
}
fn d_true() -> bool {
    true
}
fn d_1() -> f64 {
    1.0
}
fn d_2() -> f64 {
    2.0
}
fn d_05() -> f64 {
    0.5
}
fn d_512() -> u32 {
    512
}

/// Body of POST /v1/sessions (mirrors the MCP `capture_start` args).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct StartSessionRequest {
    pub output_dir: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub pid: Option<i64>,
    #[serde(default)]
    pub window_id: Option<i64>,
    #[serde(default)]
    pub app_name: Option<String>,
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default = "d_1")]
    pub screenshot_interval: f64,
    #[serde(default = "d_png")]
    pub screenshot_format: String,
    #[serde(default)]
    pub screenshot_resolution: Option<String>,
    #[serde(default)]
    pub screenshot_jpeg_quality: Option<u32>,
    #[serde(default = "d_true")]
    pub capture_screenshots: bool,
    #[serde(default = "d_true")]
    pub capture_audio: bool,
    #[serde(default = "d_auto")]
    pub audio_source: String,
    #[serde(default)]
    pub mic_device: Option<String>,
    #[serde(default)]
    pub audio_chunk_seconds: Option<f64>,
    #[serde(default = "d_auto")]
    pub asr_backend: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
}

/// Body of POST /v1/asr/model and POST /v1/asr/models/download.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AsrModelRequest {
    pub repo: String,
}

/// Body of POST /v1/sessions/import — turn an existing audio/video file into a session.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ImportMediaRequest {
    pub path: String,
    #[serde(default)]
    pub output_dir: Option<String>,
    #[serde(default = "d_auto")]
    pub asr_backend: String,
    #[serde(default = "d_2")]
    pub screenshot_interval: f64,
}

/// Body of POST /v1/sessions/{id}/index — build the multimodal index (#44).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct IndexRequest {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<i64>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "d_05")]
    pub sample_rate: f64,
    #[serde(default = "d_512")]
    pub max_leaves: u32,
    #[serde(default = "d_true")]
    pub fuse_transcript: bool,
    #[serde(default)]
    pub prompt_preset: Option<String>,
    #[serde(default)]
    pub leaf_prompt: Option<String>,
    #[serde(default)]
    pub leaf_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub classify_prompt: Option<String>,
    #[serde(default)]
    pub max_px: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Response types deserialize realistic live-daemon JSON; key fields land.
    #[test]
    fn deserialize_health() {
        // HealthResponse carries extra fields (platform, sessions) the lenient GUI type ignores.
        let j = r#"{"ok":true,"version":"0.2.6","api_version":"1.0","pid":4242,
                    "platform":"darwin","sessions":{"live":1,"history":3}}"#;
        let h: Health = serde_json::from_str(j).unwrap();
        assert!(h.ok);
        assert_eq!(h.version, "0.2.6");
        assert_eq!(h.api_version, "1.0");
        assert_eq!(h.pid, 4242);
    }

    #[test]
    fn deserialize_session() {
        // SessionSummary wire shape; the GUI type ignores the fields it doesn't render.
        let j = r#"{"session_id":"5806dc","state":"running","dir":"/runs/5806dc",
                    "pid":900,"window_title":"Chrome","started_at":"t","stopped_at":null,
                    "screenshots":12,"screenshot_errors":0,"log_lines":4,"process_running":true,
                    "audio_mode":"app","audio_status":"recording","transcript_segments":7,
                    "asr_errors":0,"mic_status":"off","mic_segments":0,"mic_device":null,
                    "capture_preset":"meeting","index_preset":"meeting",
                    "has_screenshots":true,"has_audio":true,"has_mic":false,
                    "can_retranscribe":true,"can_index":true,"notes":[]}"#;
        let s: Session = serde_json::from_str(j).unwrap();
        assert_eq!(s.session_id, "5806dc");
        assert_eq!(s.state, "running");
        assert_eq!(s.screenshots, 12);
        assert_eq!(s.transcript_segments, 7);
        assert!(s.can_index);
        // The expanded SessionSummary fields land too.
        assert_eq!(s.pid, Some(900));
        assert_eq!(s.audio_mode, "app");
        assert_eq!(s.capture_preset.as_deref(), Some("meeting"));
        assert_eq!(s.mic_status, "off");
        assert_eq!(s.asr_errors, 0);
        assert!(s.notes.is_empty());

        // A minimal body (only the required pair) still decodes — the GUI's lenient read.
        let min: Session = serde_json::from_str(r#"{"session_id":"x","state":"stopped"}"#).unwrap();
        assert_eq!(min.audio_mode, ""); // absent string → serde default
        assert_eq!(min.mic_status, "off"); // pydantic default preserved
        assert!(min.has_screenshots && min.can_index); // capability flags default true
        assert!(min.notes.is_empty());
    }

    #[test]
    fn deserialize_asr_models() {
        let j = r#"{"backend_available":true,"active":"mlx-community/whisper-large-v3",
                    "language":null,"chunk_seconds":30.0,
                    "models":[{"repo":"mlx-community/whisper-large-v3","name":"large-v3",
                               "size_label":"1.5 GB","downloaded":true,"active":true}]}"#;
        let m: AsrModels = serde_json::from_str(j).unwrap();
        assert!(m.backend_available);
        assert_eq!(m.chunk_seconds, 30.0);
        assert_eq!(m.models.len(), 1);
        let model = &m.models[0];
        assert_eq!(model.name, "large-v3");
        assert!(model.active);
        assert!(!model.downloading); // defaulted (absent in JSON)
    }

    /// A request serializes the expected field names, and omitted optionals take their defaults
    /// on the round trip back.
    #[test]
    fn start_session_request_roundtrip_defaults() {
        let req = StartSessionRequest {
            output_dir: "/runs".into(),
            command: Some("echo hi".into()),
            pid: None,
            window_id: None,
            app_name: None,
            bundle_id: None,
            screenshot_interval: d_1(),
            screenshot_format: d_png(),
            screenshot_resolution: None,
            screenshot_jpeg_quality: None,
            capture_screenshots: true,
            capture_audio: true,
            audio_source: d_auto(),
            mic_device: None,
            audio_chunk_seconds: None,
            asr_backend: d_auto(),
            cwd: None,
            preset: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"output_dir\":\"/runs\""));
        assert!(json.contains("\"command\":\"echo hi\""));
        assert!(json.contains("\"screenshot_format\":\"png\""));

        // A minimal body (only required + target) deserializes with pydantic defaults.
        let minimal = r#"{"output_dir":"/runs","command":"echo hi"}"#;
        let back: StartSessionRequest = serde_json::from_str(minimal).unwrap();
        assert_eq!(back.screenshot_interval, 1.0);
        assert_eq!(back.screenshot_format, "png");
        assert!(back.capture_screenshots);
        assert!(back.capture_audio);
        assert_eq!(back.audio_source, "auto");
        assert_eq!(back.asr_backend, "auto");
    }

    #[test]
    fn index_request_defaults() {
        let back: IndexRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(back.sample_rate, 0.5);
        assert_eq!(back.max_leaves, 512);
        assert!(back.fuse_transcript);
        assert!(back.provider.is_none());
    }

    #[test]
    fn import_media_request_defaults() {
        let back: ImportMediaRequest = serde_json::from_str(r#"{"path":"/a.mp4"}"#).unwrap();
        assert_eq!(back.asr_backend, "auto");
        assert_eq!(back.screenshot_interval, 2.0);
        assert!(back.output_dir.is_none());
    }

    /// `deny_unknown_fields` (pydantic `extra="forbid"`): an unexpected field is a contract breach.
    #[test]
    fn request_rejects_unknown_field() {
        let bad = r#"{"output_dir":"/runs","command":"echo","bogus":1}"#;
        let r: Result<StartSessionRequest, _> = serde_json::from_str(bad);
        assert!(r.is_err());
    }
}
