//! Standalone validation runner for the **dynamically-loaded** whisper engine: `dlopen` the engine
//! cdylib, load a GGML model, and transcribe a raw 16 kHz mono s16le clip — exercising the real
//! `DynamicEngine` → C ABI → whisper.cpp path the daemon will use.
//!
//! Usage:
//!   cargo run -p capture-asr --example transcribe -- <engine.dylib> <model.bin> <audio.s16le> [language]

use std::path::Path;

use capture_asr::{is_silent, silence_rms16, AsrBackend, DynamicEngine};

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: transcribe <engine.dylib> <model.bin> <audio.s16le> [language]";
    let engine = args.next().expect(usage);
    let model = args.next().expect(usage);
    let audio_path = args.next().expect(usage);
    let language = args.next();

    // s16le (16-bit little-endian mono) → float32 in [-1, 1].
    let bytes = std::fs::read(&audio_path).expect("read audio");
    let pcm: Vec<f32> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    let secs = pcm.len() as f64 / 16_000.0;
    eprintln!("audio: {} samples ({secs:.1}s) from {audio_path}", pcm.len());
    eprintln!("silence gate: is_silent={}", is_silent(&pcm, silence_rms16()));

    let t0 = std::time::Instant::now();
    let backend = DynamicEngine::load(Path::new(&engine), &model, language.as_deref())
        .expect("dlopen + load engine");
    eprintln!(
        "engine '{}' dlopen'd + model loaded in {:.2}s (language={})",
        backend.engine_name(),
        t0.elapsed().as_secs_f64(),
        language.as_deref().unwrap_or("auto"),
    );

    let t1 = std::time::Instant::now();
    let segs = backend.transcribe(&pcm, 16_000).expect("transcribe");
    let dt = t1.elapsed().as_secs_f64();
    eprintln!(
        "transcribed in {dt:.2}s ({:.1}x realtime); {} segments",
        secs / dt,
        segs.len()
    );
    for s in &segs {
        println!("[{:7.2} -> {:7.2}] {}", s.start, s.end, s.text);
    }
}
