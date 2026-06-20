//! Live audio-capture validation. Captures N seconds of app or mic audio through `capture-platform`,
//! writes s16le, and reports the sample count + RMS (proving the SCStream → Float32 → s16le pipeline).
//!
//!   cargo run -p capture-platform --example audio -- app <pid> [secs] [out.s16le]
//!   cargo run -p capture-platform --example audio -- mic [secs] [out.s16le]
//!
//! App audio keys off Screen Recording; even a silent app yields ~16 kHz of (zero) samples, so a
//! plausible sample count validates the mechanism regardless of whether sound is playing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use capture_platform::{
    start_audio_capture, start_audio_capture_dual, AudioTarget, AUDIO_SAMPLE_RATE,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("mic");

    // `dual <pid> [secs]` — the de-risk: capture app audio AND the mic from ONE SCStream and report
    // both sample counts (the single-stream fix for the two-concurrent-streams limitation).
    if mode == "dual" {
        let pid = args.get(1).and_then(|s| s.parse::<i32>().ok());
        let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
        let app_buf = Arc::new(Mutex::new((0usize, 0u32)));
        let mic_buf = Arc::new(Mutex::new((0usize, 0u32)));
        let a = app_buf.clone();
        let m = mic_buf.clone();
        eprintln!("dual: app(pid {pid:?}) + mic from one stream for {secs}s ...");
        let cap = start_audio_capture_dual(
            Some(&AudioTarget::App { pid, bundle_id: None }),
            Some("default"),
            Box::new(move |b, hz| {
                let mut g = a.lock().unwrap();
                g.0 += b.len();
                g.1 = hz;
            }),
            Box::new(move |b, hz| {
                let mut g = m.lock().unwrap();
                g.0 += b.len();
                g.1 = hz;
            }),
        )
        .unwrap_or_else(|e| {
            eprintln!("dual start failed: {e}");
            std::process::exit(1);
        });
        std::thread::sleep(Duration::from_secs(secs));
        cap.stop().ok();
        let (an, ar) = *app_buf.lock().unwrap();
        let (mn, mr) = *mic_buf.lock().unwrap();
        eprintln!("DUAL RESULT: app={an} samples @ {ar} Hz | mic={mn} samples @ {mr} Hz");
        if an > 0 && mn > 0 {
            eprintln!("  ✓ BOTH streams delivered — single-stream app+mic works");
        } else {
            eprintln!("  ✗ a stream was silent (app={an}, mic={mn})");
        }
        return;
    }
    let (target, rest) = match mode {
        "app" => {
            let pid = args.get(1).and_then(|s| s.parse::<i32>().ok());
            (AudioTarget::App { pid, bundle_id: None }, 2)
        }
        _ => (AudioTarget::Mic { device_id: None }, 1),
    };
    let secs: u64 = args.get(rest).and_then(|s| s.parse().ok()).unwrap_or(3);
    let out = args.get(rest + 1).cloned().unwrap_or_else(|| format!("/tmp/capture_audio_{mode}.s16le"));

    let samples: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
    let rate: Arc<Mutex<u32>> = Arc::new(Mutex::new(AUDIO_SAMPLE_RATE));
    let sink = samples.clone();
    let rate_sink = rate.clone();
    eprintln!("capturing {mode} audio for {secs}s ...");
    let cap = match start_audio_capture(&target, move |batch, hz| {
        sink.lock().unwrap().extend_from_slice(batch);
        *rate_sink.lock().unwrap() = hz;
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("start failed: {e}");
            std::process::exit(1);
        }
    };
    std::thread::sleep(std::time::Duration::from_secs(secs));
    cap.stop().ok();

    let buf = samples.lock().unwrap();
    let hz = *rate.lock().unwrap();
    let n = buf.len();
    let rms = if n > 0 {
        (buf.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / n as f64).sqrt()
    } else {
        0.0
    };
    let mut bytes = Vec::with_capacity(n * 2);
    for &s in buf.iter() {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(&out, &bytes).expect("write s16le");
    let dur = if hz > 0 { n as f64 / hz as f64 } else { 0.0 };
    eprintln!("captured {n} samples @ {hz} Hz (~{dur:.1}s), RMS={rms:.0} → {out} ({} bytes)", bytes.len());
    if n == 0 {
        eprintln!("NOTE: 0 samples — no audio flowed (silent source, or a TCC denial).");
    }
}
