//! Offline re-transcription of a finished session's `audio.s16le`.

use std::path::Path;

use serde_json::{json, Value};

use capture_asr::{is_silent, silence_rms16, AsrRuntimeManager, TARGET_SAMPLE_RATE};
use capture_core::time::iso;

use crate::helpers::{round3, BYTES_PER_SAMPLE};

/// Re-run ASR over a finished session's `audio.s16le` (16 kHz mono — no resampling) with the active
/// runtime, replacing `transcript.{jsonl,txt}` (the prior kept as `transcript.prev.*`). New segment
/// timestamps anchor to the SAME audio epoch (recovered from the old transcript's first record, else
/// `started_at`) so subtitles still line up with the screenshots; chunking mirrors the live pump so
/// offsets match. `on_progress` is `(done_bytes, total_bytes, segments)`. Returns the new segment
/// count. Mirrors `retranscribe.retranscribe_session`.
pub fn retranscribe_session(
    dir: &Path,
    chunk_seconds: f64,
    mut on_progress: impl FnMut(u64, u64, i64),
) -> Result<i64, String> {
    use std::io::Write;

    let raw_path = dir.join("audio.s16le");
    let meta = std::fs::metadata(&raw_path)
        .map_err(|_| "no audio to re-transcribe (audio.s16le missing or empty)".to_string())?;
    if !meta.is_file() || meta.len() == 0 {
        return Err("no audio to re-transcribe (audio.s16le missing or empty)".into());
    }
    let pcm_bytes = std::fs::read(&raw_path).map_err(|e| format!("read audio.s16le: {e}"))?;
    let epoch = recover_epoch(dir); // BEFORE the rename below
    let backend = AsrRuntimeManager::new().backend()?;
    let chunk_seconds = chunk_seconds.max(1.0);

    // Preserve the prior transcript before overwriting.
    for name in ["transcript.jsonl", "transcript.txt"] {
        let f = dir.join(name);
        if f.exists() {
            let _ = std::fs::rename(&f, dir.join(name.replace("transcript", "transcript.prev")));
        }
    }

    let chunk_bytes = (chunk_seconds * TARGET_SAMPLE_RATE as f64) as usize * BYTES_PER_SAMPLE;
    let min_tail = BYTES_PER_SAMPLE * TARGET_SAMPLE_RATE as usize / 10; // transcribe tails >= 0.1 s
    let total = pcm_bytes.len();
    let threshold = silence_rms16();
    let mut jsonl = std::fs::File::create(dir.join("transcript.jsonl"))
        .map_err(|e| format!("create transcript.jsonl: {e}"))?;
    let mut txt = std::fs::File::create(dir.join("transcript.txt"))
        .map_err(|e| format!("create transcript.txt: {e}"))?;

    let mut segments: i64 = 0;
    let mut consumed: usize = 0; // samples handed downstream (for offsets)
    let mut pos = 0;
    while pos < total {
        let end = (pos + chunk_bytes).min(total);
        let chunk = &pcm_bytes[pos..end];
        pos = end;
        if chunk.len() < min_tail {
            break;
        }
        let chunk_offset = consumed as f64 / TARGET_SAMPLE_RATE as f64;
        let pcm: Vec<f32> = chunk
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect();
        if !is_silent(&pcm, threshold) {
            if let Ok(segs) = backend.transcribe(&pcm, TARGET_SAMPLE_RATE) {
                for seg in segs {
                    let start = iso(Some(epoch + chunk_offset + seg.start));
                    let rec = json!({
                        "start": start,
                        "end": iso(Some(epoch + chunk_offset + seg.end)),
                        "start_offset": round3(chunk_offset + seg.start),
                        "end_offset": round3(chunk_offset + seg.end),
                        "text": seg.text,
                    });
                    let _ = writeln!(jsonl, "{rec}");
                    let _ = writeln!(txt, "[{start}] {}", seg.text);
                    segments += 1;
                }
            }
        }
        consumed += chunk.len() / BYTES_PER_SAMPLE;
        on_progress(pos.min(total) as u64, total as u64, segments);
    }
    Ok(segments)
}

/// The audio first-byte wall-clock epoch (so re-transcribed segments align with the screenshots):
/// the old transcript's first record (`start - start_offset`), else `session.json` `started_at`,
/// else 0. Mirrors `retranscribe._recover_epoch`.
fn recover_epoch(dir: &Path) -> f64 {
    if let Ok(text) = std::fs::read_to_string(dir.join("transcript.jsonl")) {
        for ln in text.lines() {
            if let Ok(rec) = serde_json::from_str::<Value>(ln) {
                let st = rec.get("start").and_then(|v| v.as_str()).and_then(parse_iso);
                let off = rec.get("start_offset").and_then(|v| v.as_f64());
                if let (Some(st), Some(off)) = (st, off) {
                    return st - off;
                }
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("session.json")) {
        if let Ok(meta) = serde_json::from_str::<Value>(&text) {
            if let Some(st) = meta
                .get("summary")
                .and_then(|s| s.get("started_at"))
                .and_then(|v| v.as_str())
                .and_then(parse_iso)
            {
                return st;
            }
        }
    }
    0.0
}

/// Parse an ISO `…T HH:MM:SS.sssZ` timestamp to unix seconds by reusing `parse_fs_stamp` (the only
/// difference is `:`→`-` in the time half; the date has no colons).
fn parse_iso(s: &str) -> Option<f64> {
    capture_core::time::parse_fs_stamp(&s.replace(':', "-"))
}
