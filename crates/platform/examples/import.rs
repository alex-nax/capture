//! Live validation of media import extraction.
//!
//!   cargo run -p capture-platform --example import -- audio <file> [rate]
//!   cargo run -p capture-platform --example import -- frames <file> [interval] [out_dir]
//!
//! `audio` decodes the file's audio track to s16le and reports the sample count + duration.
//! `frames` samples the video at `interval` seconds and writes each PNG to `out_dir`.

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("audio");
    let file = args.get(1).cloned().unwrap_or_default();
    if file.is_empty() {
        eprintln!("usage: import <audio|frames> <file> ...");
        std::process::exit(2);
    }

    match mode {
        "audio" => {
            let rate: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(16000);
            match capture_platform::extract_audio_s16le(Path::new(&file), rate) {
                Ok(Some(pcm)) => {
                    let samples = pcm.len() / 2;
                    eprintln!(
                        "audio: {} bytes ({} samples @ {} Hz ≈ {:.2}s)",
                        pcm.len(),
                        samples,
                        rate,
                        samples as f64 / rate as f64
                    );
                }
                Ok(None) => eprintln!("audio: no audio track"),
                Err(e) => {
                    eprintln!("audio extract failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        "frames" => {
            let interval: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            let out = args.get(3).cloned().unwrap_or_else(|| "/tmp/import-frames".into());
            let max_width = std::env::var("CAP_W").ok().and_then(|s| s.parse::<u32>().ok());
            std::fs::create_dir_all(&out).ok();
            match capture_platform::extract_frames(Path::new(&file), interval, max_width) {
                Ok(frames) => {
                    eprintln!("frames: {} sampled", frames.len());
                    for f in &frames {
                        let p = format!("{out}/{:08}.png", f.offset_ms);
                        std::fs::write(&p, &f.png).expect("write png");
                    }
                    if !frames.is_empty() {
                        eprintln!("  wrote PNGs to {out}/ (first {} bytes)", frames[0].png.len());
                    }
                }
                Err(e) => {
                    eprintln!("frame extract failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("unknown mode {mode:?}");
            std::process::exit(2);
        }
    }
}
