// SPIKE B (#60): whisper-rs (whisper.cpp Rust bindings) feasibility on Apple silicon.
// Loads ggml-base.en.bin, transcribes 60s of real 16kHz mono s16le speech,
// benchmarks model-load vs transcribe wall-clock, prints realtime factor.

use std::time::Instant;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
};

const MODEL_PATH: &str = "ggml-base.en.bin";
const AUDIO_PATH: &str =
    "/Users/alex/.capture/runs/capture-2026-06-17T10-10-36.393Z-5806dc/audio.s16le";

// 16kHz mono. 60s = 960000 samples = 1_920_000 bytes (i16 little-endian).
const SAMPLE_RATE: usize = 16_000;
const CLIP_SECONDS: usize = 60;
const N_SAMPLES: usize = SAMPLE_RATE * CLIP_SECONDS; // 960_000
const SKIP_SECONDS: usize = 2; // skip possibly-silent first ~2s

fn load_audio_f32() -> std::io::Result<Vec<f32>> {
    let bytes = std::fs::read(AUDIO_PATH)?;
    let skip_bytes = SKIP_SECONDS * SAMPLE_RATE * 2;
    let need_bytes = N_SAMPLES * 2;
    let end = (skip_bytes + need_bytes).min(bytes.len());
    let slice = &bytes[skip_bytes..end];
    let mut out = Vec::with_capacity(slice.len() / 2);
    for chunk in slice.chunks_exact(2) {
        let s = i16::from_le_bytes([chunk[0], chunk[1]]);
        out.push(s as f32 / 32768.0);
    }
    Ok(out)
}

fn main() {
    println!("=== whisper-rs spike (#60) ===");

    let audio = load_audio_f32().expect("read audio");
    let audio_secs = audio.len() as f64 / SAMPLE_RATE as f64;
    println!(
        "audio: {} samples ({:.2}s) loaded from s16le",
        audio.len(),
        audio_secs
    );

    // --- (a) model load ---
    let t0 = Instant::now();
    let ctx = WhisperContext::new_with_params(MODEL_PATH, WhisperContextParameters::default())
        .expect("load model");
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("[timing] model load: {:.1} ms", load_ms);

    let mut state = ctx.create_state().expect("create state");

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(8);
    params.set_translate(false);
    params.set_language(Some("en"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    // --- (b) transcribe ---
    let t1 = Instant::now();
    state.full(params, &audio).expect("run full");
    let transcribe_s = t1.elapsed().as_secs_f64();

    // Collect segment text. 0.16: full_n_segments()->c_int, get_segment(i)->Option<WhisperSegment>.
    let n = state.full_n_segments();
    let mut text = String::new();
    for i in 0..n {
        if let Some(seg) = state.get_segment(i) {
            text.push_str(seg.to_str().expect("segment utf8"));
        }
    }

    println!("[timing] transcribe: {:.3} s", transcribe_s);
    println!(
        "[timing] realtime factor (transcribe/{:.0}s clip): {:.4}",
        audio_secs,
        transcribe_s / audio_secs
    );
    println!("[timing] xRT speedup: {:.2}x", audio_secs / transcribe_s);
    println!("\n=== TRANSCRIPTION ({} segments) ===", n);
    println!("{}", text.trim());
}
