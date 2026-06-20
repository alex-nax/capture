//! Pure helpers shared across the engine submodules: id/path/format utilities, screenshot-option
//! resolution, the preset → index-preset mapping, on-disk capability probing, and the interruptible
//! sleep used by the worker loops.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use capture_platform::{ImageFormat, ScreenshotOptions};

use crate::CaptureConfig;

pub(crate) const BYTES_PER_SAMPLE: usize = 2;

/// 6 hex chars of OS randomness (the session-id suffix; mirrors `secrets.token_hex(3)`).
pub(crate) fn rand_hex6() -> String {
    let mut b = [0u8; 3];
    let _ = getrandom::getrandom(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn expand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

pub(crate) fn non_empty(title: &str, app_name: &str) -> String {
    if title.is_empty() {
        app_name.to_string()
    } else {
        title.to_string()
    }
}

pub(crate) fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// The window to screenshot this tick: an explicit picked id, else the target pid/app's largest
/// window, else `None` (whole-screen fallback). Mirrors `Screenshotter._resolve_window_id`.
pub(crate) fn resolve_shot_window(window_id: Option<i64>, pid: Option<i32>, app_name: Option<&str>) -> Option<u32> {
    if let Some(w) = window_id {
        return Some(w as u32);
    }
    if pid.is_some() || app_name.is_some() {
        if let Ok(wins) = capture_platform::list_windows(pid, app_name) {
            return wins.first().map(|w| w.window_id);
        }
    }
    None
}

/// Resolve `(ScreenshotOptions, file-extension, resolved-format-string)` from the config. A
/// `"WxH/fmt"` resolution spec can override the format. Mirrors `parse_resolution` + the session's
/// format resolution.
pub(crate) fn screenshot_opts(c: &CaptureConfig) -> (ScreenshotOptions, &'static str, String) {
    let (resolution, fmt_override) = parse_resolution(c.screenshot_resolution.as_deref());
    let fmt_str = fmt_override.unwrap_or_else(|| c.screenshot_format.to_ascii_lowercase());
    let format = ImageFormat::parse(&fmt_str);
    let ext = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
    };
    let opts = ScreenshotOptions {
        format,
        resolution,
        jpeg_quality: c.screenshot_jpeg_quality.map(|q| q.clamp(1, 100) as u8),
    };
    (opts, ext, fmt_str)
}

/// Parse `"WxH"` or `"WxH/fmt"` → `(Some((w,h)), Some(fmt))`. Lenient: bad specs yield `(None, None)`.
/// Mirrors `screenshots.parse_resolution`.
pub(crate) fn parse_resolution(spec: Option<&str>) -> (Option<(u32, u32)>, Option<String>) {
    let Some(spec) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        return (None, None);
    };
    let (dims, fmt) = match spec.split_once('/') {
        Some((d, f)) => (d.trim(), Some(f.trim().to_ascii_lowercase())),
        None => (spec, None),
    };
    let dims = dims.to_ascii_lowercase().replace('×', "x");
    let parsed = dims
        .split_once('x')
        .and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?)))
        .filter(|&(w, h)| w >= 1 && h >= 1);
    (parsed, fmt)
}

/// The index preset a capture preset resolves to (#54). Mirrors `presets.index_preset_for`.
pub(crate) fn index_preset_for(preset: Option<&str>) -> String {
    match preset.unwrap_or("auto") {
        "meeting" => "meeting",
        "coding" => "coding",
        "lecture" => "lecture",
        "custom" => "custom",
        _ => "auto", // auto / general / unknown
    }
    .to_string()
}

/// `(has_screenshots, has_audio, has_mic)` from the on-disk artifacts (recomputed each summary so
/// pruning is reflected). `can_retranscribe`/`can_index` derive from these. Mirrors
/// `session_capabilities`.
pub(crate) fn session_capabilities(dir: &Path) -> (bool, bool, bool) {
    let has_shots = std::fs::read_dir(dir.join("screenshots"))
        .map(|mut it| it.any(|e| e.is_ok()))
        .unwrap_or(false);
    let nonempty = |name: &str| {
        std::fs::metadata(dir.join(name)).map(|m| m.is_file() && m.len() > 0).unwrap_or(false)
    };
    (has_shots, nonempty("audio.s16le"), nonempty("mic.s16le"))
}

/// Sleep up to `secs`, waking early (in ~50 ms steps) if `stop` is set, so teardown is prompt.
pub(crate) fn sleep_interruptible(secs: f64, stop: &AtomicBool) {
    let mut left = secs;
    while left > 0.0 && !stop.load(Ordering::Relaxed) {
        let step = left.min(0.05);
        std::thread::sleep(std::time::Duration::from_secs_f64(step));
        left -= step;
    }
}
