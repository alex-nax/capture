// sck-capture — v3 de-risk SPIKE A (#59).
//
// Question: can ScreenCaptureKit be driven from Rust well enough to replace the
// Swift `helper/audiocap.swift` (per-app audio + window screenshots)? This throwaway
// binary exercises three capabilities through the `screencapturekit` crate (v7):
//
//   1. list   — SCShareableContent → displays / windows / apps (title, owner, id)
//   2. shot   — SCScreenshotManager::capture_image → PNG at /tmp/sck_spike.png
//   3. audio  — per-app SCStream (capturesAudio) → ~3s s16le at /tmp/sck_spike.s16le
//
// A bare `cargo run` binary is NOT the TCC-granted Capture.app, so Screen Recording
// may prompt or silently deny — that's a FINDING, not a failure (see FINDINGS.md).
//
//   cargo run               # runs all three in sequence
//   cargo run -- list
//   cargo run -- shot [display|--pid <PID>|--bundle <id>]
//   cargo run -- audio [--pid <PID>|--bundle <id>] [--secs N]

use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

const SHOT_PATH: &str = "/tmp/sck_spike.png";
const AUDIO_PATH: &str = "/tmp/sck_spike.s16le";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("all");

    match cmd {
        "list" => {
            let _ = list_content(true);
        }
        "shot" => {
            run_shot(&args);
        }
        "audio" => {
            run_audio(&args);
        }
        "all" => {
            eprintln!("== capability 1: list shareable content ==");
            let _ = list_content(true);
            eprintln!("\n== capability 2: screenshot (display 0) ==");
            run_shot(&[]);
            eprintln!("\n== capability 3: per-app audio (first audible app, 3s) ==");
            run_audio(&[]);
        }
        other => {
            eprintln!("unknown command '{other}' (use: list | shot | audio | all)");
            std::process::exit(2);
        }
    }
}

// ---- capability 1: list shareable content -----------------------------------

fn list_content(print: bool) -> Option<SCShareableContent> {
    let content = match SCShareableContent::get() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SCShareableContent::get FAILED: {e:?}");
            eprintln!("(this is where a TCC denial usually surfaces)");
            return None;
        }
    };
    let displays = content.displays();
    let windows = content.windows();
    let apps = content.applications();
    eprintln!(
        "content: displays={} windows={} apps={}",
        displays.len(),
        windows.len(),
        apps.len()
    );
    if print {
        for d in displays.iter().take(3) {
            eprintln!(
                "  display id={} {}x{}",
                d.display_id(),
                d.width(),
                d.height()
            );
        }
        for w in windows.iter().filter(|w| w.title().is_some()).take(8) {
            let owner = w
                .owning_application()
                .map(|a| a.application_name())
                .unwrap_or_else(|| "?".into());
            eprintln!(
                "  window id={} owner='{}' title='{}'",
                w.window_id(),
                owner,
                w.title().unwrap_or_default()
            );
        }
    }
    Some(content)
}

// ---- target resolution (shared by shot/audio) -------------------------------

fn find_app(content: &SCShareableContent, args: &[String]) -> Option<SCRunningApplication> {
    if let Some(pid) = flag(args, "--pid").and_then(|v| v.parse::<i32>().ok()) {
        return content.applications().into_iter().find(|a| a.process_id() == pid);
    }
    if let Some(b) = flag(args, "--bundle") {
        return content
            .applications()
            .into_iter()
            .find(|a| a.bundle_identifier() == b);
    }
    None
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(String::as_str)
}

// ---- capability 2: screenshot -----------------------------------------------

fn run_shot(args: &[String]) {
    let Some(content) = list_content(false) else {
        return;
    };
    let Some(display) = content.displays().into_iter().next() else {
        eprintln!("no display");
        return;
    };

    // Default: whole display. --pid/--bundle scopes the screenshot to one app.
    let (filter, label) = if let Some(app) = find_app(&content, args) {
        let f = SCContentFilter::create()
            .with_display(&display)
            .with_including_applications(&[&app], &[])
            .build();
        (f, format!("app '{}'", app.application_name()))
    } else {
        let f = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build();
        (f, format!("display {}", display.display_id()))
    };

    let config = SCStreamConfiguration::new()
        .with_width(display.width())
        .with_height(display.height());

    eprintln!("capturing screenshot of {label}...");
    let img = match SCScreenshotManager::capture_image(&filter, &config) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("capture_image FAILED: {e:?} (likely TCC / Screen Recording)");
            return;
        }
    };
    let (w, h) = (img.width(), img.height());
    let bgra = match img.bgra_data() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("bgra_data FAILED: {e:?}");
            return;
        }
    };
    // BGRA -> RGBA for the png crate.
    let mut rgba = bgra.clone();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    match write_png(SHOT_PATH, w as u32, h as u32, &rgba) {
        Ok(()) => eprintln!("wrote {SHOT_PATH} ({w}x{h}, {} bytes BGRA)", bgra.len()),
        Err(e) => eprintln!("png write failed: {e}"),
    }
}

fn write_png(path: &str, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(to_io)?;
    writer.write_image_data(rgba).map_err(to_io)?;
    Ok(())
}

fn to_io(e: png::EncodingError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

// ---- capability 3: per-app audio --------------------------------------------

// Output handler. SCK dispatch queues invoke this from arbitrary threads, so the
// trait requires Send + Sync — we share an Arc<Mutex<File>> + an atomic byte count.
struct AudioSink {
    file: std::sync::Mutex<std::fs::File>,
    bytes: Arc<AtomicU64>,
}

impl SCStreamOutputTrait for AudioSink {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if of_type != SCStreamOutputType::Audio {
            return;
        }
        // SCK delivers app/system audio as deinterleaved Float32 (one AudioBuffer
        // per channel). The Swift helper hands this to AVAudioConverter -> s16le;
        // here we convert ch0 Float32 -> i16 inline to prove the bytes are real.
        let Some(list) = sample.audio_buffer_list() else {
            return;
        };
        let Some(buf) = list.get(0) else { return };
        let raw = buf.data();
        let mut out = Vec::with_capacity(raw.len() / 2);
        for f in raw.chunks_exact(4) {
            let s = f32::from_le_bytes([f[0], f[1], f[2], f[3]]);
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(&out);
        }
        self.bytes.fetch_add(out.len() as u64, Ordering::Relaxed);
    }
}

fn run_audio(args: &[String]) {
    let secs: u64 = flag(args, "--secs").and_then(|v| v.parse().ok()).unwrap_or(3);
    let Some(content) = list_content(false) else {
        return;
    };
    let Some(display) = content.displays().into_iter().next() else {
        eprintln!("no display");
        return;
    };

    // Pick a target app: explicit --pid/--bundle, else the first running app that
    // is plausibly audible (heuristic: first window-owning app that isn't us).
    let app = find_app(&content, args).or_else(|| {
        let me = std::process::id() as i32;
        content
            .windows()
            .into_iter()
            .filter_map(|w| w.owning_application())
            .find(|a| a.process_id() != me && !a.application_name().is_empty())
    });
    let Some(app) = app else {
        eprintln!("no target app resolved");
        return;
    };
    eprintln!(
        "audio target: '{}' pid={} (play sound in it now)",
        app.application_name(),
        app.process_id()
    );

    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_including_applications(&[&app], &[])
        .build();

    // Mirror audiocap.swift's stream config: 16 kHz mono, app audio only, with a
    // tiny but non-degenerate video config (SCK rejects very small frames).
    let config = SCStreamConfiguration::new()
        .with_captures_audio(true)
        .with_sample_rate(16_000)
        .with_channel_count(1)
        .with_excludes_current_process_audio(true)
        .with_width(128)
        .with_height(128);

    let file = match std::fs::File::create(AUDIO_PATH) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot create {AUDIO_PATH}: {e}");
            return;
        }
    };
    let bytes = Arc::new(AtomicU64::new(0));
    let sink = AudioSink {
        file: std::sync::Mutex::new(file),
        bytes: bytes.clone(),
    };

    let mut stream = SCStream::new(&filter, &config);
    stream.add_output_handler(sink, SCStreamOutputType::Audio);

    if let Err(e) = stream.start_capture() {
        eprintln!("start_capture FAILED: {e:?} (likely TCC / Screen Recording denial)");
        return;
    }
    eprintln!("capturing {secs}s of audio -> {AUDIO_PATH} ...");
    std::thread::sleep(std::time::Duration::from_secs(secs));
    let _ = stream.stop_capture();

    let total = bytes.load(Ordering::Relaxed);
    eprintln!("captured {total} bytes s16le ({} samples)", total / 2);
    if total == 0 {
        eprintln!("NOTE: 0 bytes — either no audio played, or the app produced silence.");
    }
}
