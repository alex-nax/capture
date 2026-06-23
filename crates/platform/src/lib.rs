//! capture-platform — the v3 OS capture backends (a port of `core/platform/` + `core/{windows,
//! permissions,screenshots,audio}.py`). Window discovery, screenshots, per-app + mic audio, and TCC
//! permission status, behind a small platform-neutral surface the daemon's engine routes call.
//!
//! macOS uses **ScreenCaptureKit** (the `screencapturekit` crate — no Swift helper; spike A) for
//! windows/screenshots/audio and CoreGraphics for the Screen-Recording TCC check. Windows backends
//! land with #66. Built incrementally (#65): **[A this slice] window listing + screen-recording
//! status** → [B] screenshots → [C] per-app + mic audio (+ device list, mic TCC) → [D] the capture
//! session loop wired to `capture-asr`.

use serde::Serialize;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod import;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
mod windows_audio;

/// One on-screen top-level window, in the `/v1/windows` wire shape (mirrors `core.list_windows`).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub window_id: u32,
    pub pid: i32,
    pub app_name: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
}

/// One audio input device, in the `/v1/audio/mics` wire shape (mirrors `AudioSource.list_input_devices`
/// + `v1::AudioDevice`).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AudioInputInfo {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// The capture sample rate (16 kHz mono s16le — the contract every ASR path expects).
pub const AUDIO_SAMPLE_RATE: u32 = 16_000;

/// What to capture audio from (mirrors the `audio_source` capture setting).
#[derive(Clone, Debug)]
pub enum AudioTarget {
    /// A running application's audio, per-app via a ScreenCaptureKit content filter.
    App { pid: Option<i32>, bundle_id: Option<String> },
    /// The microphone (macOS-15 SCK mic capture). `device_id` from [`audio_input_devices`], or `None`
    /// for the system default.
    Mic { device_id: Option<String> },
}

/// A running audio capture. Mono `i16` (s16le) sample batches flow to the `on_samples` callback —
/// `(&[i16], source_rate_hz)` — invoked from a capture thread until [`AudioCapture::stop`] or drop.
/// The rate is the buffer's actual rate: SCK resamples **app/system** audio to 16 kHz, but the
/// **microphone** arrives at its native hardware rate (typically 48 kHz), so the session loop must
/// resample each chunk to 16 kHz (via `capture-asr::resample_linear`) before ASR + `audio.s16le`.
pub struct AudioCapture {
    #[cfg(target_os = "macos")]
    inner: macos::MacAudioCapture,
    #[cfg(target_os = "windows")]
    inner: windows_audio::WinAudioCapture,
}

impl AudioCapture {
    /// Stop the capture (idempotent with drop).
    pub fn stop(self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.stop()
        }
        #[cfg(target_os = "windows")]
        {
            self.inner.stop()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Ok(())
        }
    }
}

/// Start capturing audio from `target`, delivering mono s16le sample batches + their source rate to
/// `on_samples` until the returned [`AudioCapture`] is stopped/dropped. `Err` if the target can't be
/// resolved or the OS refuses the stream (on macOS, usually a TCC denial).
pub fn start_audio_capture(
    target: &AudioTarget,
    on_samples: impl Fn(&[i16], u32) + Send + Sync + 'static,
) -> Result<AudioCapture, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(AudioCapture { inner: macos::start_audio_capture(target, on_samples)? })
    }
    #[cfg(target_os = "windows")]
    {
        Ok(AudioCapture { inner: windows_audio::start_audio_capture(target, on_samples)? })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (target, on_samples);
        Err("audio capture is not supported on this platform yet".to_string())
    }
}

/// A boxed audio sink — `(&[i16], source_rate)` — for [`start_audio_capture_dual`].
pub type AudioCallback = Box<dyn Fn(&[i16], u32) + Send + Sync>;

/// Start ONE SCStream capturing app/system audio (`app` = an [`AudioTarget::App`]) **and/or** the
/// microphone (`mic_device` = `Some`), delivering each to its callback. This exists because macOS
/// won't deliver audio to **two concurrent SCStreams in one process** — a session that wants both app
/// audio and a mic track must capture them from a single stream (two output types). `on_audio` gets
/// the app/system audio; `on_mic` gets the microphone. `mic_device` of `Some("")`/`Some("default")`
/// uses the system default input.
pub fn start_audio_capture_dual(
    app: Option<&AudioTarget>,
    mic_device: Option<&str>,
    on_audio: AudioCallback,
    on_mic: AudioCallback,
) -> Result<AudioCapture, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(AudioCapture { inner: macos::start_audio_capture_dual(app, mic_device, on_audio, on_mic)? })
    }
    #[cfg(target_os = "windows")]
    {
        Ok(AudioCapture {
            inner: windows_audio::start_audio_capture_dual(app, mic_device, on_audio, on_mic)?,
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (app, mic_device, on_audio, on_mic);
        Err("audio capture is not supported on this platform yet".to_string())
    }
}

/// A screenshot output format (mirrors the `screenshot_format` capture setting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    /// Parse the wire/CLI format string (`png` | `jpg`/`jpeg`); anything else falls back to PNG.
    pub fn parse(fmt: &str) -> ImageFormat {
        match fmt.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            _ => ImageFormat::Png,
        }
    }
}

/// How to encode a captured screenshot (mirrors the `screenshot_{format,resolution,jpeg_quality}`
/// capture settings).
#[derive(Clone, Debug)]
pub struct ScreenshotOptions {
    pub format: ImageFormat,
    /// Fit within this `(w, h)` box, aspect-preserving, never upscaling. `None` = native size.
    pub resolution: Option<(u32, u32)>,
    /// JPEG quality `1..=100` (JPEG only); `None` → a sensible default.
    pub jpeg_quality: Option<u8>,
}

impl Default for ScreenshotOptions {
    fn default() -> Self {
        ScreenshotOptions { format: ImageFormat::Png, resolution: None, jpeg_quality: None }
    }
}

/// Capture a screenshot of one window (`Some(window_id)`) or the whole main display (`None`), encoded
/// to bytes per `opts`. The session loop writes the bytes to `<session>/screenshots/<stamp>.<ext>`.
/// `Err` if the OS capture fails (on macOS, usually Screen Recording not granted, or the window closed).
pub fn capture_screenshot(window_id: Option<u32>, opts: &ScreenshotOptions) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "macos")]
    {
        macos::capture_screenshot(window_id, opts)
    }
    #[cfg(target_os = "windows")]
    {
        windows::capture_screenshot(window_id, opts)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (window_id, opts);
        Err("screenshots are not supported on this platform yet".to_string())
    }
}

/// Largest `(w, h)` fitting inside box `(bw, bh)`, aspect-preserving, never upscaled. Port of
/// `platform.base.fit_box`.
fn fit_box(sw: u32, sh: u32, bw: u32, bh: u32) -> (u32, u32) {
    let scale = (bw as f64 / sw as f64).min(bh as f64 / sh as f64).min(1.0);
    (((sw as f64 * scale).round() as u32).max(1), ((sh as f64 * scale).round() as u32).max(1))
}

/// Encode raw RGBA8 pixels to PNG/JPEG bytes, optionally downscaling to fit `opts.resolution` first
/// (the Rust replacement for `screencapture` + `sips`). Pure image processing — platform-agnostic.
pub(crate) fn encode_image(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    opts: &ScreenshotOptions,
) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;

    let mut img = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or("screenshot buffer size doesn't match its dimensions")?;
    if let Some((bw, bh)) = opts.resolution {
        let (tw, th) = fit_box(width, height, bw, bh);
        if (tw, th) != (width, height) {
            img = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
        }
    }
    let mut out = Vec::new();
    match opts.format {
        ImageFormat::Png => image::codecs::png::PngEncoder::new(&mut out)
            .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
            .map_err(|e| format!("png encode: {e}"))?,
        ImageFormat::Jpeg => {
            // JPEG has no alpha channel — flatten to RGB.
            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
            let quality = opts.jpeg_quality.unwrap_or(80).clamp(1, 100);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
                .map_err(|e| format!("jpeg encode: {e}"))?;
        }
    }
    Ok(out)
}

// ── TCC permission states (mirror `core/permissions.py`) ────────────────────────────────────────
/// Screen Recording / Microphone are granted.
pub const GRANTED: &str = "granted";
/// The right is denied.
pub const DENIED: &str = "denied";
/// The user hasn't been asked yet (microphone only).
pub const UNDETERMINED: &str = "undetermined";
/// Not a macOS build — the concept doesn't apply.
pub const NOT_APPLICABLE: &str = "not_applicable";
/// Couldn't determine the status.
pub const UNKNOWN: &str = "unknown";

/// On-screen top-level windows, largest area first; filter by `pid` and/or a case-insensitive
/// app-name substring (mirrors `WindowFinder.find` / `core.list_windows`). `Err` if the OS query
/// fails (on macOS that usually means Screen Recording isn't granted).
pub fn list_windows(pid: Option<i32>, app_name: Option<&str>) -> Result<Vec<WindowInfo>, String> {
    #[cfg(target_os = "macos")]
    {
        macos::list_windows(pid, app_name)
    }
    #[cfg(target_os = "windows")]
    {
        windows::list_windows(pid, app_name)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (pid, app_name);
        Ok(Vec::new())
    }
}

/// The Screen-Recording TCC status (`CGPreflightScreenCaptureAccess`, no prompt). `not_applicable`
/// off macOS. The daemon needs this right for screenshots, window titles, and SCK per-app audio.
pub fn screen_recording_status() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        macos::screen_recording_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        NOT_APPLICABLE
    }
}

/// The Microphone TCC status (`granted`/`denied`/`undetermined` via the AVFoundation authorization
/// check, no prompt). `not_applicable` off macOS. Needed for the separate mic track (per-app audio
/// keys off Screen Recording instead).
pub fn microphone_status() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        macos::microphone_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        NOT_APPLICABLE
    }
}

/// One sampled video frame from an import: its millisecond offset from the start + the PNG bytes.
pub struct ImportedFrame {
    pub offset_ms: u64,
    pub png: Vec<u8>,
}

/// Decode an audio/video file's first audio track to 16-bit little-endian mono PCM at `rate` Hz
/// (`Ok(None)` ⇒ the file has no audio track — a video-only import). The in-process AVFoundation
/// replacement for the Swift helper's `--extract-audio`. `Err` off macOS.
pub fn extract_audio_s16le(src: &std::path::Path, rate: u32) -> Result<Option<Vec<u8>>, String> {
    #[cfg(target_os = "macos")]
    {
        import::extract_audio_s16le(src, rate)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (src, rate);
        Err("media import is macOS-only for now (AVFoundation); a cross-platform path is planned".into())
    }
}

/// Sample frames from a video file at `interval` seconds (`0, interval, … ≤ duration`), each encoded
/// PNG with its millisecond offset; `max_width` optionally caps width (aspect-preserved). Empty for an
/// audio-only file. The AVFoundation replacement for the helper's `--extract-frames`. `Err` off macOS.
pub fn extract_frames(
    src: &std::path::Path,
    interval: f64,
    max_width: Option<u32>,
) -> Result<Vec<ImportedFrame>, String> {
    #[cfg(target_os = "macos")]
    {
        import::extract_frames(src, interval, max_width)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (src, interval, max_width);
        Err("media import is macOS-only for now (AVFoundation); a cross-platform path is planned".into())
    }
}

/// Available audio input devices (microphones), the system default flagged. Empty off macOS / on
/// error. Mirrors `AudioSource.list_input_devices`.
pub fn audio_input_devices() -> Vec<AudioInputInfo> {
    #[cfg(target_os = "macos")]
    {
        macos::audio_input_devices()
    }
    #[cfg(target_os = "windows")]
    {
        windows_audio::audio_input_devices()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_box_preserves_aspect_and_never_upscales() {
        assert_eq!(fit_box(100, 50, 200, 200), (100, 50)); // already inside the box → unchanged
        assert_eq!(fit_box(1000, 500, 100, 100), (100, 50)); // width-bound, aspect kept
        assert_eq!(fit_box(500, 1000, 100, 100), (50, 100)); // height-bound
        assert_eq!(fit_box(16, 8, 4, 4), (4, 2));
    }

    fn blank_rgba(w: u32, h: u32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    #[test]
    fn encode_png_is_valid_and_keeps_dimensions() {
        let png = encode_image(blank_rgba(8, 4), 8, 4, &ScreenshotOptions::default()).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n"); // PNG signature
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (8, 4));
    }

    #[test]
    fn encode_jpeg_downscales_to_fit_resolution() {
        let opts = ScreenshotOptions {
            format: ImageFormat::Jpeg,
            resolution: Some((4, 4)),
            jpeg_quality: Some(70),
        };
        let jpg = encode_image(blank_rgba(16, 8), 16, 8, &opts).unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]); // JPEG SOI marker
        let img = image::load_from_memory(&jpg).unwrap();
        assert_eq!((img.width(), img.height()), (4, 2)); // (16x8) fit into (4x4)
    }

    #[test]
    fn encode_rejects_a_mismatched_buffer() {
        assert!(encode_image(vec![0u8; 10], 8, 4, &ScreenshotOptions::default()).is_err());
    }

    #[test]
    fn image_format_parse() {
        assert_eq!(ImageFormat::parse("JPG"), ImageFormat::Jpeg);
        assert_eq!(ImageFormat::parse("jpeg"), ImageFormat::Jpeg);
        assert_eq!(ImageFormat::parse("png"), ImageFormat::Png);
        assert_eq!(ImageFormat::parse("weird"), ImageFormat::Png);
    }
}
