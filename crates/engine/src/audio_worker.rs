//! The audio → ASR worker (runs on its own thread) plus the SCK-callback sink that feeds it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use capture_asr::{is_silent, resample_linear, silence_rms16, AsrRuntimeManager, TARGET_SAMPLE_RATE};
use capture_core::time::{iso, now};

use crate::helpers::{round3, sleep_interruptible, BYTES_PER_SAMPLE};
use crate::{Counters, EventSink};

/// An [`capture_platform::AudioCallback`] that appends each sample batch (+ its rate) into `buf` — the
/// lock-free hand-off from the SCK callback thread to a worker.
pub(crate) fn sink_into(buf: Arc<Mutex<(Vec<i16>, u32)>>) -> capture_platform::AudioCallback {
    Box::new(move |batch: &[i16], hz: u32| {
        let mut b = buf.lock().unwrap();
        b.1 = hz;
        b.0.extend_from_slice(batch);
    })
}

pub(crate) struct AudioWorker {
    pub(crate) dir: PathBuf,
    pub(crate) shared: Arc<Mutex<(Vec<i16>, u32)>>,
    pub(crate) chunk_seconds: f64,
    pub(crate) t0: f64,
    pub(crate) counters: Arc<Counters>,
    pub(crate) status: Arc<Mutex<(String, String)>>, // (mode, status) for this track
    pub(crate) emit: EventSink,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) id: String,
    pub(crate) track: &'static str, // "audio" | "mic" — the file-name prefix + the SSE `track` field
    pub(crate) append: bool,        // append to existing files (a live mic switch continues mic.s16le)
}

/// Drain the shared buffer, chunk by `chunk_seconds`, resample each chunk to 16 kHz, write the raw
/// PCM, silence-gate, transcribe, and append the track's transcript. `track="audio"` writes
/// `audio.s16le`/`transcript.*`; any other track writes `<track>.s16le`/`<track>_transcript.*`.
/// Mirrors `audio.AudioCapture._read_loop` + `_transcribe`.
pub(crate) fn audio_worker(w: AudioWorker) {
    use std::io::Write;

    let (raw_name, jsonl_name, txt_name) = if w.track == "audio" {
        ("audio.s16le".to_string(), "transcript.jsonl".to_string(), "transcript.txt".to_string())
    } else {
        (format!("{}.s16le", w.track), format!("{}_transcript.jsonl", w.track), format!("{}_transcript.txt", w.track))
    };
    let open = |name: &str| -> Option<std::fs::File> {
        let path = w.dir.join(name);
        if w.append {
            std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
        } else {
            std::fs::File::create(path).ok()
        }
    };

    let backend = AsrRuntimeManager::new().backend();
    if let Err(ref e) = backend {
        w.status.lock().unwrap().1 = format!("running (asr-unavailable: {e})");
    }
    let mut raw = open(&raw_name);
    let mut jsonl = open(&jsonl_name);
    let mut txt = open(&txt_name);

    let threshold = silence_rms16();
    let mut local: Vec<i16> = Vec::new();
    let mut rate = TARGET_SAMPLE_RATE;
    let mut samples_out: u64 = 0; // total 16 kHz samples handed downstream (for offsets)
    let mut epoch: Option<f64> = None;

    let mut process = |chunk: &[i16], rate: u32, samples_out: &mut u64, epoch: f64| {
        // Resample to 16 kHz (identity when already 16 kHz, e.g. app audio).
        let f32_src: Vec<f32> = chunk.iter().map(|&s| s as f32 / 32768.0).collect();
        let pcm16 = if rate == TARGET_SAMPLE_RATE {
            f32_src
        } else {
            resample_linear(&f32_src, rate, TARGET_SAMPLE_RATE)
        };
        let n_out = pcm16.len();
        if n_out == 0 {
            return;
        }
        if let Some(f) = raw.as_mut() {
            let mut bytes = Vec::with_capacity(n_out * BYTES_PER_SAMPLE);
            for &s in &pcm16 {
                bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            }
            let _ = f.write_all(&bytes);
        }
        let chunk_offset = *samples_out as f64 / TARGET_SAMPLE_RATE as f64;
        *samples_out += n_out as u64;

        let Ok(ref be) = backend else { return };
        if is_silent(&pcm16, threshold) {
            return; // offset already advanced — the timeline still holds
        }
        match be.transcribe(&pcm16, TARGET_SAMPLE_RATE) {
            Ok(segments) => {
                for seg in segments {
                    let start = iso(Some(epoch + chunk_offset + seg.start));
                    let end = iso(Some(epoch + chunk_offset + seg.end));
                    let rec = json!({
                        "start": start, "end": end,
                        "start_offset": round3(chunk_offset + seg.start),
                        "end_offset": round3(chunk_offset + seg.end),
                        "text": seg.text,
                    });
                    if let Some(f) = jsonl.as_mut() {
                        let _ = writeln!(f, "{rec}");
                    }
                    if let Some(f) = txt.as_mut() {
                        let _ = writeln!(f, "[{start}] {}", seg.text);
                    }
                    let seg = if w.track == "audio" {
                        &w.counters.transcript_segments
                    } else {
                        &w.counters.mic_segments
                    };
                    let n = seg.fetch_add(1, Ordering::Relaxed) + 1;
                    let mut ev = rec.clone();
                    if let Value::Object(ref mut m) = ev {
                        m.insert("type".into(), json!("transcript_segment"));
                        m.insert("session_id".into(), json!(w.id));
                        m.insert("count".into(), json!(n));
                        m.insert("track".into(), json!(w.track));
                    }
                    (w.emit)(ev);
                }
            }
            Err(_) => {
                w.counters.asr_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    };

    loop {
        let stopping = w.stop.load(Ordering::Relaxed);
        {
            let mut b = w.shared.lock().unwrap();
            if !b.0.is_empty() {
                if epoch.is_none() {
                    epoch = Some(now());
                }
                local.append(&mut b.0);
            }
            if b.1 != 0 {
                rate = b.1;
            }
        }
        let ep = epoch.unwrap_or(w.t0);
        let chunk_src = (w.chunk_seconds * rate as f64) as usize;
        while chunk_src > 0 && local.len() >= chunk_src {
            let chunk: Vec<i16> = local.drain(..chunk_src).collect();
            process(&chunk, rate, &mut samples_out, ep);
        }
        if stopping {
            // Final tail (>= 0.1 s of audio), mirroring MIN_TAIL_BYTES.
            let min_tail = (0.1 * rate as f64) as usize;
            if local.len() >= min_tail {
                process(&local, rate, &mut samples_out, ep);
            }
            break;
        }
        sleep_interruptible(0.1, &w.stop);
    }
    // `process` (holding &mut on the files) is dropped here; the files then close on scope exit.
}
