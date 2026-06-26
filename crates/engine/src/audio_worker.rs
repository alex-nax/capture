//! The audio → ASR worker (runs on its own thread) plus the SCK-callback sink that feeds it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use capture_asr::{is_silent, resample_linear, silence_rms16, AsrRuntimeManager, TARGET_SAMPLE_RATE};
use capture_core::time::{iso, now};

use crate::helpers::{round3, sleep_interruptible, BYTES_PER_SAMPLE};
use crate::{Counters, EventSink};

/// An [`capture_platform::AudioCallback`] that appends each sample batch (+ its rate) into `buf` — the
/// lock-free hand-off from the SCK callback thread to a worker — and stamps `last` (epoch millis) with
/// the delivery time so the audio watchdog can tell a live stream from one that has silently stalled.
pub(crate) fn sink_into(buf: Arc<Mutex<(Vec<i16>, u32)>>, last: Arc<AtomicU64>) -> capture_platform::AudioCallback {
    Box::new(move |batch: &[i16], hz: u32| {
        last.store((now() * 1000.0) as u64, Ordering::Relaxed);
        let mut b = buf.lock().unwrap();
        b.1 = hz;
        b.0.extend_from_slice(batch);
    })
}

/// Round an empirically-measured sample rate to the nearest standard PCM rate. The audio worker
/// measures delivered-samples ÷ wall-clock to recover a mic's TRUE rate when SCK mislabels it
/// (Bluetooth-HFP delivers 8 kHz tagged as the requested 16 kHz); the measurement lands within a few
/// percent, so snapping to the nearest real rate removes timing jitter. (#87)
fn round_to_standard_rate(measured: f64) -> u32 {
    const STD: [u32; 5] = [8000, 16000, 32000, 44100, 48000];
    STD.into_iter()
        .min_by(|&a, &b| (measured - a as f64).abs().total_cmp(&(measured - b as f64).abs()))
        .unwrap_or(TARGET_SAMPLE_RATE)
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
    let mut rate = TARGET_SAMPLE_RATE;
    let mut tx16: Vec<f32> = Vec::new(); // resampled 16 kHz samples awaiting transcription
    let mut transcribed: u64 = 0; // 16 kHz samples already transcribed (the offset base)
    let mut epoch: Option<f64> = None;
    let chunk16 = (w.chunk_seconds * TARGET_SAMPLE_RATE as f64) as usize; // transcription window, in 16 kHz samples

    // Empirical source-rate detection (#87). SCK labels Bluetooth-HFP mic buffers with the *requested*
    // 16 kHz while delivering native 8 kHz, so `rate` (from the platform layer) is unreliable for the
    // mic. Measure the TRUE rate from delivered-samples ÷ wall-clock over the first PROBE_SECONDS,
    // buffering the opening audio so none is resampled at the wrong rate, then resample everything from
    // the measured rate. App/system audio (already 16 kHz) just measures ~16 kHz → unchanged.
    const PROBE_SECONDS: f64 = 1.5;
    let mut measured_rate: Option<u32> = None;
    let mut probe_raw: Vec<i16> = Vec::new();
    let mut probe_samples: u64 = 0;
    let mut probe_t0: Option<f64> = None;

    // Transcribe one 16 kHz window: silence-gate, ASR, then append + emit each segment. `offset` is the
    // window's start (seconds) on the 16 kHz timeline; segment stamps are `epoch + offset + seg`. ASR
    // wants a full chunk (≥~24 s) to avoid short-chunk hallucination — that's why TRANSCRIPTION is
    // windowed even though the raw audio is written continuously below.
    let mut transcribe = |pcm16: &[f32], offset: f64, epoch: f64| {
        let Ok(ref be) = backend else { return };
        if is_silent(pcm16, threshold) {
            return; // the caller still advances the offset, so the timeline holds through silence
        }
        match be.transcribe(pcm16, TARGET_SAMPLE_RATE) {
            Ok(segments) => {
                for seg in segments {
                    let start = iso(Some(epoch + offset + seg.start));
                    let end = iso(Some(epoch + offset + seg.end));
                    let rec = json!({
                        "start": start, "end": end,
                        "start_offset": round3(offset + seg.start),
                        "end_offset": round3(offset + seg.end),
                        "text": seg.text,
                    });
                    if let Some(f) = jsonl.as_mut() {
                        let _ = writeln!(f, "{rec}");
                    }
                    if let Some(f) = txt.as_mut() {
                        let _ = writeln!(f, "[{start}] {}", seg.text);
                    }
                    let counter = if w.track == "audio" {
                        &w.counters.transcript_segments
                    } else {
                        &w.counters.mic_segments
                    };
                    let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
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
        // Drain the SCK hand-off buffer.
        let batch: Vec<i16> = {
            let mut b = w.shared.lock().unwrap();
            if !b.0.is_empty() && epoch.is_none() {
                epoch = Some(now());
            }
            if b.1 != 0 {
                rate = b.1;
            }
            std::mem::take(&mut b.0)
        };
        let ep = epoch.unwrap_or(w.t0);
        // Decide what to process and at what source rate. While probing the true rate, buffer the audio
        // and write nothing; once measured (or on stop), flush the buffer at the measured rate. After
        // that, resample + write the raw PCM in REAL TIME each ~0.1 s drain (so audio.s16le tracks the
        // capture live and a crash/stall loses ~0.1 s, not a whole chunk). The same samples accumulate
        // for transcription below.
        let (proc_samples, src_rate): (Vec<i16>, u32) = if measured_rate.is_none() {
            if !batch.is_empty() {
                match probe_t0 {
                    // First delivery: start the clock but don't count its burst (it accreted in the
                    // shared buffer over an unknown prior interval and would bias the rate high).
                    None => probe_t0 = Some(now()),
                    Some(_) => probe_samples += batch.len() as u64,
                }
                probe_raw.extend_from_slice(&batch);
            }
            let elapsed = probe_t0.map(|t0| now() - t0).unwrap_or(0.0);
            let ready = probe_t0.is_some() && elapsed >= PROBE_SECONDS && probe_samples > 0;
            if ready || (stopping && !probe_raw.is_empty()) {
                let measured = if elapsed > 0.05 && probe_samples > 0 {
                    round_to_standard_rate(probe_samples as f64 / elapsed)
                } else {
                    rate // stopped before a full window — trust the reported rate
                };
                measured_rate = Some(measured);
                eprintln!(
                    "[audio:{}] measured source rate {measured} Hz (reported {rate}; {probe_samples} samples / {elapsed:.2}s)",
                    w.track
                );
                (w.emit)(json!({
                    "type": "audio_rate",
                    "session_id": w.id,
                    "track": w.track,
                    "measured_rate": measured,
                    "reported_rate": rate,
                }));
                (std::mem::take(&mut probe_raw), measured)
            } else {
                (Vec::new(), rate) // keep probing — nothing written yet
            }
        } else {
            (batch, measured_rate.unwrap_or(rate))
        };
        if !proc_samples.is_empty() {
            let f32_src: Vec<f32> = proc_samples.iter().map(|&s| s as f32 / 32768.0).collect();
            let pcm16 = if src_rate == TARGET_SAMPLE_RATE {
                f32_src
            } else {
                resample_linear(&f32_src, src_rate, TARGET_SAMPLE_RATE)
            };
            if let Some(f) = raw.as_mut() {
                let mut bytes = Vec::with_capacity(pcm16.len() * BYTES_PER_SAMPLE);
                for &s in &pcm16 {
                    bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                }
                let _ = f.write_all(&bytes);
            }
            tx16.extend_from_slice(&pcm16);
        }
        // Transcribe whenever a full `chunk_seconds` window of 16 kHz audio is available.
        while chunk16 > 0 && tx16.len() >= chunk16 {
            let chunk: Vec<f32> = tx16.drain(..chunk16).collect();
            let offset = transcribed as f64 / TARGET_SAMPLE_RATE as f64;
            transcribe(&chunk, offset, ep);
            transcribed += chunk16 as u64;
        }
        if stopping {
            // Final tail (>= 0.1 s), mirroring MIN_TAIL_BYTES.
            let min_tail = (0.1 * TARGET_SAMPLE_RATE as f64) as usize;
            if tx16.len() >= min_tail {
                let offset = transcribed as f64 / TARGET_SAMPLE_RATE as f64;
                transcribe(&tx16, offset, ep);
            }
            break;
        }
        sleep_interruptible(0.1, &w.stop);
    }
    // `transcribe` (holding &mut on the transcript files) is dropped here; the files (incl. raw) then
    // close on scope exit.
}

#[cfg(test)]
mod tests {
    use super::round_to_standard_rate;

    #[test]
    fn rounds_measured_rates_to_nearest_standard() {
        // Bluetooth-HFP narrowband, real measured values from live captures → 8 kHz.
        assert_eq!(round_to_standard_rate(7862.0), 8000);
        assert_eq!(round_to_standard_rate(8160.0), 8000);
        assert_eq!(round_to_standard_rate(8000.0), 8000);
        // Built-in / app audio (SCK-resampled), robust to ±jitter → 16 kHz.
        assert_eq!(round_to_standard_rate(16188.0), 16000);
        assert_eq!(round_to_standard_rate(15400.0), 16000);
        assert_eq!(round_to_standard_rate(17500.0), 16000);
        // Higher native rates snap correctly.
        assert_eq!(round_to_standard_rate(47000.0), 48000);
        assert_eq!(round_to_standard_rate(44100.0), 44100);
    }
}
