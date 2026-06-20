//! Live screenshot validation (needs a display + Screen Recording). Captures one window or the whole
//! main display through `capture-platform` and writes the encoded bytes to a file.
//!
//!   cargo run -p capture-platform --example screenshot                 # whole display → /tmp/capture_shot.png
//!   cargo run -p capture-platform --example screenshot -- <window_id>  # one window
//!   cargo run -p capture-platform --example screenshot -- <window_id> jpg /tmp/w.jpg

use capture_platform::{capture_screenshot, list_windows, ImageFormat, ScreenshotOptions};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `first` lists windows and captures the largest one in this same process (no staleness).
    let window_id = match args.first().map(String::as_str) {
        Some("first") => {
            let wins = list_windows(None, None).expect("list windows");
            let w = wins.first().expect("at least one window");
            eprintln!("first window: id={} {:?} {}x{}", w.window_id, w.app_name, w.width, w.height);
            Some(w.window_id)
        }
        other => other.and_then(|s| s.parse::<u32>().ok()),
    };
    let format = args.get(1).map(|s| ImageFormat::parse(s)).unwrap_or(ImageFormat::Png);
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
    };
    let out = args.get(2).cloned().unwrap_or_else(|| format!("/tmp/capture_shot.{ext}"));

    let opts = ScreenshotOptions { format, resolution: Some((1600, 1600)), jpeg_quality: Some(85) };
    let target = window_id.map(|w| format!("window {w}")).unwrap_or_else(|| "whole display".into());
    eprintln!("capturing {target} → {out} ...");
    match capture_screenshot(window_id, &opts) {
        Ok(bytes) => {
            std::fs::write(&out, &bytes).expect("write file");
            eprintln!("wrote {out} ({} bytes)", bytes.len());
        }
        Err(e) => {
            eprintln!("capture failed: {e}");
            std::process::exit(1);
        }
    }
}
