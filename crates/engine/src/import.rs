//! Import an existing audio/video file as a capture session (#43) — a port of `core/import_media.py`.
//!
//! Extract the file's audio to `audio.s16le`, sample its video frames into `screenshots/` on the SAME
//! wall-clock timeline as the audio, write `session.json`, then run ASR (reusing
//! [`crate::retranscribe_session`], which recovers the epoch from `started_at` so subtitles line up
//! with the frames). The result lists + plays back exactly like a live capture; only
//! `audio_source="import"` in its config marks the origin. Extraction is in-process via
//! `capture_platform` (AVFoundation) — no Swift helper, no ffmpeg.

use std::path::Path;

use serde_json::{json, Value};

use capture_core::time::{fs_stamp, iso, now};
use capture_core::v1;

use crate::helpers::{expand, index_preset_for, rand_hex6, session_capabilities};
use crate::retranscribe::retranscribe_session;
use crate::EventSink;

/// The s16le rate the audio is decoded to (the live capture's ASR input rate).
const SAMPLE_RATE: u32 = 16000;
/// ASR chunk length for an import's transcription (matches the Python's import path).
const IMPORT_CHUNK_SECONDS: f64 = 8.0;

/// Import `src` into a new session dir under `output_dir`; returns the session summary (the same shape
/// `/v1/sessions/{id}` serves). `emit` receives `import` progress events (`phase` + `fraction`, keyed
/// by the new session id) and a terminal `import_done`. Errors if the file is missing or yields neither
/// audio nor frames. Mirrors `import_file`.
pub fn import_media(
    output_dir: &str,
    src: &Path,
    asr_backend: &str,
    screenshot_interval: f64,
    emit: &EventSink,
) -> Result<Value, String> {
    if !src.is_file() {
        return Err(format!("file not found: {}", src.display()));
    }
    let file_name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

    // Anchor the timeline at import time: ids are timestamp-prefixed (this sorts newest), and the
    // frames + transcript share this epoch.
    let base = now();
    let id = format!("{}-{}", fs_stamp(Some(base)), rand_hex6());
    let dir = expand(output_dir).join(format!("capture-{id}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    let progress = |phase: &str, frac: f64| {
        emit(json!({
            "type": "import", "session_id": id, "phase": phase,
            "fraction": (frac.clamp(0.0, 1.0) * 10000.0).round() / 10000.0,
        }));
    };

    // 1. Audio → audio.s16le. `None`/empty ⇒ no audio track (a frames-only import); a real decode
    //    error aborts.
    progress("extract-audio", 0.0);
    let has_audio = match capture_platform::extract_audio_s16le(src, SAMPLE_RATE) {
        Ok(Some(pcm)) if !pcm.is_empty() => {
            std::fs::write(dir.join("audio.s16le"), &pcm).map_err(|e| format!("write audio.s16le: {e}"))?;
            true
        }
        Ok(_) => false,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("could not extract audio from {file_name}: {e}"));
        }
    };
    progress("extract-audio", 1.0);

    // 2. Frames → screenshots/<fs_stamp(base + offset)>.png (same epoch as the transcript, so subtitles
    //    align in playback). Audio-only files yield none.
    progress("extract-frames", 0.0);
    let frames = match capture_platform::extract_frames(src, screenshot_interval, None) {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("could not extract frames from {file_name}: {e}"));
        }
    };
    let shot_count = frames.len() as i64;
    if !frames.is_empty() {
        let shots = dir.join("screenshots");
        std::fs::create_dir_all(&shots).map_err(|e| format!("create screenshots/: {e}"))?;
        for f in &frames {
            let stamp = fs_stamp(Some(base + f.offset_ms as f64 / 1000.0));
            std::fs::write(shots.join(format!("{stamp}.png")), &f.png)
                .map_err(|e| format!("write frame: {e}"))?;
        }
    }
    let has_frames = shot_count > 0;
    progress("extract-frames", 1.0);

    if !has_audio && !has_frames {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("{file_name} has no audio or video track to import"));
    }

    // 3. session.json (config + summary) — written BEFORE ASR so `retranscribe_session` recovers the
    //    epoch from `started_at` and aligns segments with the frames.
    let started_at = iso(Some(base));
    let config = build_config(src, asr_backend, screenshot_interval, has_audio, has_frames);
    let mut notes = vec![format!("imported from {file_name}")];
    write_meta(&dir, &config, &build_summary(&id, &dir, &file_name, &started_at, has_audio, shot_count, 0, &notes));

    // 4. ASR over the extracted audio (reuses retranscribe; epoch from `started_at`). A backend failure
    //    leaves a valid (frames + audio) session rather than aborting the import.
    progress("transcribe", 0.0);
    let mut segments = 0i64;
    if has_audio {
        let on_progress = |done: u64, total: u64, _segs: i64| {
            progress("transcribe", if total > 0 { done as f64 / total as f64 } else { 1.0 });
        };
        match retranscribe_session(&dir, IMPORT_CHUNK_SECONDS, on_progress) {
            Ok(n) => segments = n,
            Err(e) => notes.push(format!("transcription failed: {e}")),
        }
    }
    progress("transcribe", 1.0);

    // Final summary with the segment count + refreshed capabilities/notes.
    let summary = build_summary(&id, &dir, &file_name, &started_at, has_audio, shot_count, segments, &notes);
    write_meta(&dir, &config, &summary);
    emit(json!({ "type": "import_done", "session_id": id, "segments": segments, "screenshots": shot_count }));
    Ok(summary)
}

/// The `config` block of an import's `session.json` — `audio_source="import"` + the `source_file`
/// distinguish it from a live capture.
fn build_config(src: &Path, asr_backend: &str, interval: f64, has_audio: bool, has_frames: bool) -> Value {
    json!({
        "command": Value::Null,
        "pid": Value::Null,
        "app_name": Value::Null,
        "bundle_id": Value::Null,
        "audio_source": "import",
        "source_file": src.to_string_lossy(),
        "capture_screenshots": has_frames,
        "capture_audio": has_audio,
        "screenshot_interval": interval,
        "asr_backend": asr_backend,
        "cwd": Value::Null,
    })
}

/// The import summary, built as a [`v1::Session`] (state `stopped`) so it can't drift from the read
/// layer. Capabilities are disk-computed from `dir`.
#[allow(clippy::too_many_arguments)]
fn build_summary(
    id: &str,
    dir: &Path,
    file_name: &str,
    started_at: &str,
    has_audio: bool,
    shots: i64,
    segments: i64,
    notes: &[String],
) -> Value {
    let caps = session_capabilities(dir);
    let s = v1::Session {
        session_id: id.to_string(),
        state: "stopped".into(),
        dir: dir.to_string_lossy().into_owned(),
        pid: None,
        window_title: Some(file_name.to_string()),
        started_at: Some(started_at.to_string()),
        stopped_at: Some(started_at.to_string()),
        screenshots: shots,
        screenshot_errors: 0,
        log_lines: 0,
        process_running: Some(false),
        audio_mode: if has_audio { "import".into() } else { "off".into() },
        audio_status: if has_audio { "imported".into() } else { "no-audio".into() },
        transcript_segments: segments,
        asr_errors: 0,
        mic_status: "off".into(),
        mic_segments: 0,
        mic_device: None,
        capture_preset: None,
        index_preset: Some(index_preset_for(None)),
        has_screenshots: caps.0,
        has_audio: caps.1,
        has_mic: caps.2,
        can_retranscribe: caps.1,
        can_index: caps.0,
        notes: notes.to_vec(),
    };
    serde_json::to_value(s).unwrap_or(Value::Null)
}

/// Write `{config, summary}` to `session.json` (pretty, like the live engine's `write_metadata`).
fn write_meta(dir: &Path, config: &Value, summary: &Value) {
    let meta = json!({ "config": config, "summary": summary });
    if let Ok(body) = serde_json::to_string_pretty(&meta) {
        let _ = std::fs::write(dir.join("session.json"), body);
    }
}
