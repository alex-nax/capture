//! macOS platform backend — ScreenCaptureKit (windows, audio devices) + CoreGraphics (the
//! Screen-Recording TCC check). Port of `core/platform/macos.py`'s read-only paths + `core/
//! {windows,permissions}.py`. No Swift helper: the `screencapturekit` crate covers it (spike A).

// The prelude brings SCShareableContent/SCWindow/SCRunningApplication/SCDisplay, the stream types
// (SCStream + the SCStreamOutput/Configuration/ContentFilter), AudioInputDevice, and the
// CMSampleBuffer ext traits (`audio_buffer_list`). The screenshot manager isn't in the prelude.
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::{
    AudioInputInfo, AudioTarget, ScreenshotOptions, WindowInfo, AUDIO_SAMPLE_RATE, DENIED, GRANTED,
    UNDETERMINED, UNKNOWN,
};

/// On-screen top-level windows, largest area first; filter by pid / case-insensitive app-name
/// substring. Mirrors `WindowFinder.find`: only normal-layer (layer 0) windows, on-screen preferred
/// with a fall back to *all* windows so an app on another Space/fullscreen is still found.
pub fn list_windows(pid: Option<i32>, app_name: Option<&str>) -> Result<Vec<WindowInfo>, String> {
    let needle = app_name.map(str::to_lowercase);
    let needle = needle.as_deref();
    // ON-SCREEN windows only for the primary list. `SCShareableContent::get()` defaults to ALL windows
    // (on + off screen), which surfaces an app's off-screen helper/renderer windows — e.g. Chrome lists
    // ~2 real windows but ~24 CGWindow objects, bloating the picker. `with_on_screen_windows_only(true)`
    // restricts it to windows actually on a display; the all-windows fallback below still finds an app
    // whose windows are on another Space / fullscreen / minimized.
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(true)
        .get()
        .map_err(|e| {
            format!("SCShareableContent::get failed: {e:?} (Screen Recording may not be granted)")
        })?;
    let primary = collect(content.windows(), pid, needle);
    // No filter → the on-screen list as-is. A pid/app filter that matched nothing on-screen retries
    // across all windows (other Spaces / fullscreen), mirroring the Python two-pass.
    if !primary.is_empty() || (pid.is_none() && needle.is_none()) {
        return Ok(primary);
    }
    match SCShareableContent::create().with_on_screen_windows_only(false).get() {
        Ok(all) => Ok(collect(all.windows(), pid, needle)),
        Err(_) => Ok(primary),
    }
}

/// Filter + sort a window set into the wire shape (mirrors `_match`).
fn collect(windows: Vec<SCWindow>, pid: Option<i32>, needle: Option<&str>) -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = windows
        .into_iter()
        .filter_map(|w| {
            if w.window_layer() != 0 {
                return None; // normal windows only (skip menubar/dock/overlays)
            }
            let frame = w.frame();
            let width = frame.size.width.round();
            let height = frame.size.height.round();
            if width < 1.0 || height < 1.0 {
                return None;
            }
            let app = w.owning_application()?;
            let owner_pid = app.process_id();
            let owner_name = app.application_name();
            if pid.is_some_and(|p| owner_pid != p) {
                return None;
            }
            if needle.is_some_and(|n| !owner_name.to_lowercase().contains(n)) {
                return None;
            }
            Some(WindowInfo {
                window_id: w.window_id(),
                pid: owner_pid,
                app_name: owner_name,
                title: w.title().unwrap_or_default(),
                width: width as u32,
                height: height as u32,
            })
        })
        .collect();
    // Largest area first (a u64 product avoids overflow on big displays).
    out.sort_by(|a, b| {
        let area = |w: &WindowInfo| w.width as u64 * w.height as u64;
        area(b).cmp(&area(a))
    });
    out
}

/// Force the process's CoreGraphics window-server connection to initialize (so the per-window capture
/// filter doesn't trip `CGS_REQUIRE_INIT` in a non-GUI process). `CGMainDisplayID` is a cheap public
/// CoreGraphics call that establishes the connection.
fn ensure_window_server_connection() {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
    }
    // SAFETY: a parameterless CoreGraphics accessor with no side effects beyond connection setup.
    unsafe {
        let _ = CGMainDisplayID();
    }
}

/// The Screen-Recording TCC status via `CGPreflightScreenCaptureAccess` — a pure check that never
/// prompts (the prompt would abort this headless daemon; the GUI triggers it). Mirrors
/// `permissions.screen_recording_status`.
pub fn screen_recording_status() -> &'static str {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    // SAFETY: a parameterless CoreGraphics predicate with no side effects.
    if unsafe { CGPreflightScreenCaptureAccess() } {
        GRANTED
    } else {
        DENIED
    }
}

/// Microphone TCC status via `+[AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]` —
/// a pure status read that NEVER prompts (a prompt would `SIGABRT` this headless daemon, like the
/// screen-recording case; the GUI triggers the dialog). The mic is only needed for the separate mic
/// track — per-app/system audio keys off Screen Recording instead. Mirrors `permissions.microphone_status`.
pub fn microphone_status() -> &'static str {
    use std::os::raw::{c_char, c_void};

    // The Obj-C runtime + the `AVMediaTypeAudio` NSString constant. Done with raw FFI (the idiom the
    // CoreGraphics calls above use) so this doesn't couple to an objc2 binding version.
    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *const c_void;
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        // `AVMediaType` is a typedef for `NSString *`; the symbol holds that NSString pointer.
        static AVMediaTypeAudio: *const c_void;
    }

    // SAFETY: standard Obj-C runtime usage. `objc_msgSend` is transmuted to the exact prototype of
    // `authorizationStatusForMediaType:` (a class method returning `AVAuthorizationStatus`, an
    // `NSInteger`). All inputs are framework-provided (the class, the selector, the media-type const).
    unsafe {
        let cls = objc_getClass(c"AVCaptureDevice".as_ptr());
        if cls.is_null() {
            return UNKNOWN;
        }
        let sel = sel_registerName(c"authorizationStatusForMediaType:".as_ptr());
        let send: extern "C" fn(*const c_void, *const c_void, *const c_void) -> isize =
            std::mem::transmute(objc_msgSend as *const c_void);
        // AVAuthorizationStatus: 0 notDetermined, 1 restricted, 2 denied, 3 authorized.
        match send(cls, sel, AVMediaTypeAudio) {
            0 => UNDETERMINED,
            1 | 2 => DENIED, // restricted | denied — both mean "can't use the mic"
            3 => GRANTED,
            _ => UNKNOWN,
        }
    }
}

/// Available microphones (the system default flagged). The crate enumerates them via AVFoundation
/// internally, so no extra dependency is needed. Mirrors `MacAudioSource.list_input_devices`.
pub fn audio_input_devices() -> Vec<AudioInputInfo> {
    AudioInputDevice::list()
        .into_iter()
        .map(|d| AudioInputInfo { id: d.id, name: d.name, default: d.is_default })
        .collect()
}

/// Capture a window (`Some(id)`, via a desktop-independent-window filter == `screencapture -l`) or the
/// whole main display (`None`), then hand the RGBA pixels to [`crate::encode_image`]. Replaces the
/// Python's `screencapture` + `sips`. Mirrors `MacScreenGrabber.capture`.
pub fn capture_screenshot(
    window_id: Option<u32>,
    opts: &ScreenshotOptions,
) -> Result<Vec<u8>, String> {
    // The desktop-independent-window filter touches the window-server (SkyLight) connection; a non-GUI
    // process (a CLI daemon) hasn't initialized it, which trips `CGS_REQUIRE_INIT`. A public
    // CoreGraphics call forces the connection up first. Harmless + idempotent.
    ensure_window_server_connection();
    let content = SCShareableContent::get().map_err(|e| {
        format!("SCShareableContent::get failed: {e:?} (Screen Recording may not be granted)")
    })?;
    // A window filter captures just that window regardless of what's in front of it (matching
    // `screencapture -l`); the requested size is the window/display size — SCK returns the actual
    // pixel dimensions on the CGImage, which is what we encode.
    let (filter, req_w, req_h) = match window_id {
        Some(wid) => {
            // Search the on-screen set first, then all windows (the target may be on another
            // Space / fullscreen), mirroring `list_windows`' two-pass.
            let win = content.windows().into_iter().find(|w| w.window_id() == wid).or_else(|| {
                SCShareableContent::create()
                    .with_on_screen_windows_only(false)
                    .get()
                    .ok()?
                    .windows()
                    .into_iter()
                    .find(|w| w.window_id() == wid)
            });
            let win =
                win.ok_or_else(|| format!("window id {wid} not found (it may have closed)"))?;
            let frame = win.frame();
            let w = (frame.size.width.round().max(1.0)) as u32;
            let h = (frame.size.height.round().max(1.0)) as u32;
            (SCContentFilter::create().with_window(&win).build(), w, h)
        }
        None => {
            let display =
                content.displays().into_iter().next().ok_or("no display available")?;
            let filter =
                SCContentFilter::create().with_display(&display).with_excluding_windows(&[]).build();
            (filter, display.width(), display.height())
        }
    };
    let config = SCStreamConfiguration::new().with_width(req_w).with_height(req_h);
    let img = SCScreenshotManager::capture_image(&filter, &config).map_err(|e| {
        format!("capture_image failed: {e:?} (Screen Recording may not be granted)")
    })?;
    let width = img.width() as u32;
    let height = img.height() as u32;
    let mut rgba = img.bgra_data().map_err(|e| format!("bgra_data failed: {e:?}"))?;
    // ScreenCaptureKit delivers BGRA; the encoder wants RGBA.
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    crate::encode_image(rgba, width, height, opts)
}

// ── Audio capture (SCStream → 16 kHz mono s16le) ────────────────────────────────────────────────

/// A running ScreenCaptureKit audio stream. Holds the `SCStream` (and its dedicated dispatch queue)
/// alive; stopping/dropping ends it.
pub struct MacAudioCapture {
    stream: SCStream,
    // Each stream/output gets its OWN serial queue, kept alive for the stream's lifetime.
    _queue: DispatchQueue,
    _mic_queue: Option<DispatchQueue>,
}

impl MacAudioCapture {
    pub fn stop(self) -> Result<(), String> {
        self.stream.stop_capture().map_err(|e| format!("stop_capture failed: {e:?}"))
    }
}

impl Drop for MacAudioCapture {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture(); // best-effort if stop() wasn't called
    }
}

/// Start an SCStream capturing app or mic audio, converting each Float32 buffer to s16le and handing
/// the batch to `on_samples`. Mirrors the spike's `AudioSink` path, generalized to app + mic.
pub fn start_audio_capture(
    target: &AudioTarget,
    on_samples: impl Fn(&[i16], u32) + Send + Sync + 'static,
) -> Result<MacAudioCapture, String> {
    // Force the window-server connection up first: a non-GUI process (a screenshots-off capture)
    // otherwise gets an EMPTY `displays()` (and the SCStream filter needs a display). CGMainDisplayID
    // is the same cheap init the screenshot path uses.
    ensure_window_server_connection();
    let content = SCShareableContent::get().map_err(|e| {
        format!("SCShareableContent::get failed: {e:?} (Screen Recording may not be granted)")
    })?;
    let display = content.displays().into_iter().next().ok_or("no display available")?;

    // A tiny non-degenerate video config (SCK rejects very small frames); we only consume audio, so
    // no Screen output handler is added and the frames are dropped. 16 kHz mono matches the contract.
    let base = SCStreamConfiguration::new()
        .with_width(128)
        .with_height(128)
        .with_sample_rate(AUDIO_SAMPLE_RATE as i32)
        .with_channel_count(1);

    let (filter, config, want) = match target {
        AudioTarget::App { pid, bundle_id } => {
            let app = find_running_app(&content, *pid, bundle_id.as_deref())
                .ok_or("target application not found (is it running?)")?;
            let filter = SCContentFilter::create()
                .with_display(&display)
                .with_including_applications(&[&app], &[])
                .build();
            let config = base.with_captures_audio(true).with_excludes_current_process_audio(true);
            (filter, config, SCStreamOutputType::Audio)
        }
        AudioTarget::Mic { device_id } => {
            // Mic capture still needs a stream/filter; the display video is produced but dropped.
            let filter =
                SCContentFilter::create().with_display(&display).with_excluding_windows(&[]).build();
            let mut config = base.with_captures_microphone(true);
            if let Some(id) = device_id {
                config = config.with_microphone_capture_device_id(id);
            }
            (filter, config, SCStreamOutputType::Microphone)
        }
    };

    // A per-stream serial queue (uniquely labelled) so concurrent app + mic streams don't share one.
    let label = match want {
        SCStreamOutputType::Microphone => "com.capture.audio.mic",
        _ => "com.capture.audio.app",
    };
    let queue = DispatchQueue::new(label, DispatchQoS::UserInteractive);
    let mut stream = SCStream::new(&filter, &config);
    stream.add_output_handler_with_queue(AudioSink { on_samples, want }, want, Some(&queue));
    stream
        .start_capture()
        .map_err(|e| format!("start_capture failed: {e:?} (a TCC permission may be denied)"))?;
    Ok(MacAudioCapture { stream, _queue: queue, _mic_queue: None })
}

/// Start ONE SCStream capturing app/system audio (`app`) and/or the microphone (`mic_device`),
/// delivering each output type to its callback. Single-stream because macOS won't run two concurrent
/// audio SCStreams in a process. See [`crate::start_audio_capture_dual`].
pub fn start_audio_capture_dual(
    app: Option<&AudioTarget>,
    mic_device: Option<&str>,
    on_audio: crate::AudioCallback,
    on_mic: crate::AudioCallback,
) -> Result<MacAudioCapture, String> {
    ensure_window_server_connection();
    let content = SCShareableContent::get().map_err(|e| {
        format!("SCShareableContent::get failed: {e:?} (Screen Recording may not be granted)")
    })?;
    let display = content.displays().into_iter().next().ok_or("no display available")?;

    // Scope the filter to the app when capturing its audio, else the whole display.
    let filter = match app {
        Some(AudioTarget::App { pid, bundle_id }) => {
            let a = find_running_app(&content, *pid, bundle_id.as_deref())
                .ok_or("target application not found (is it running?)")?;
            SCContentFilter::create().with_display(&display).with_including_applications(&[&a], &[]).build()
        }
        _ => SCContentFilter::create().with_display(&display).with_excluding_windows(&[]).build(),
    };

    let mut config = SCStreamConfiguration::new()
        .with_width(128)
        .with_height(128)
        .with_sample_rate(AUDIO_SAMPLE_RATE as i32)
        .with_channel_count(1);
    if app.is_some() {
        config = config.with_captures_audio(true).with_excludes_current_process_audio(true);
    }
    if let Some(dev) = mic_device {
        config = config.with_captures_microphone(true);
        let dev = dev.trim();
        if !dev.is_empty() && dev != "default" {
            config = config.with_microphone_capture_device_id(dev);
        }
    }

    // Separate serial queues per output so the two handlers don't starve each other.
    let queue = DispatchQueue::new("com.capture.audio.sys", DispatchQoS::UserInteractive);
    let mic_queue = DispatchQueue::new("com.capture.audio.mic", DispatchQoS::UserInteractive);
    let mut stream = SCStream::new(&filter, &config);
    if app.is_some() {
        stream.add_output_handler_with_queue(
            AudioSink { on_samples: on_audio, want: SCStreamOutputType::Audio },
            SCStreamOutputType::Audio,
            Some(&queue),
        );
    }
    if mic_device.is_some() {
        stream.add_output_handler_with_queue(
            AudioSink { on_samples: on_mic, want: SCStreamOutputType::Microphone },
            SCStreamOutputType::Microphone,
            Some(&mic_queue),
        );
    }
    stream
        .start_capture()
        .map_err(|e| format!("start_capture failed: {e:?} (a TCC permission may be denied)"))?;
    Ok(MacAudioCapture { stream, _queue: queue, _mic_queue: Some(mic_queue) })
}

/// Resolve a running application by pid (preferred) or bundle id.
fn find_running_app(
    content: &SCShareableContent,
    pid: Option<i32>,
    bundle_id: Option<&str>,
) -> Option<SCRunningApplication> {
    if let Some(p) = pid {
        return content.applications().into_iter().find(|a| a.process_id() == p);
    }
    let b = bundle_id?;
    content.applications().into_iter().find(|a| a.bundle_identifier() == b)
}

/// The SCStream output handler: Float32 → i16 (s16le) + the buffer's actual rate, then `on_samples`.
/// SCK dispatch calls this from arbitrary threads, so the closure must be `Send + Sync` (trait bound).
struct AudioSink<F: Fn(&[i16], u32) + Send + Sync> {
    on_samples: F,
    want: SCStreamOutputType,
}

impl<F: Fn(&[i16], u32) + Send + Sync> SCStreamOutputTrait for AudioSink<F> {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if of_type != self.want {
            return;
        }
        // SCK delivers deinterleaved Float32 (mono → one buffer). ch0 Float32 → i16.
        let Some(list) = sample.audio_buffer_list() else {
            return;
        };
        let Some(buf) = list.get(0) else {
            return;
        };
        let raw = buf.data();
        let mut out = Vec::with_capacity(raw.len() / 4);
        for f in raw.chunks_exact(4) {
            let s = f32::from_le_bytes([f[0], f[1], f[2], f[3]]);
            out.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        if !out.is_empty() {
            (self.on_samples)(&out, buffer_sample_rate(&sample, out.len()));
        }
    }
}

/// The buffer's actual sample rate (Hz): `num_samples / duration`. SCK resamples app/system audio to
/// the requested 16 kHz, but the mic comes at its native rate — this reports whichever it is.
/// Falls back to the requested rate if the timing is unavailable.
fn buffer_sample_rate(sample: &CMSampleBuffer, sample_count: usize) -> u32 {
    let n = sample.num_samples();
    let secs = sample.duration().as_seconds().unwrap_or(0.0);
    if n > 0 && secs > 0.0 {
        (n as f64 / secs).round() as u32
    } else if sample_count > 0 {
        // No timing info — assume the requested rate (true for the app/system path).
        AUDIO_SAMPLE_RATE
    } else {
        AUDIO_SAMPLE_RATE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These also prove the Swift-runtime rpath (build.rs): a test binary linking the SCK/AVFoundation
    // bridges won't load at all if `@rpath/libswift_Concurrency.dylib` can't resolve.

    #[test]
    fn screen_recording_status_is_a_known_state() {
        // A pure preflight — no prompt, no permission needed to *check*.
        assert!(matches!(screen_recording_status(), GRANTED | DENIED));
    }

    #[test]
    fn microphone_status_is_a_known_state() {
        // The AVFoundation authorization read must never prompt or panic, and must return one of the
        // four real TCC states (whichever this machine is actually in).
        assert!(matches!(
            microphone_status(),
            GRANTED | DENIED | UNDETERMINED | UNKNOWN
        ));
    }

    #[test]
    fn audio_input_devices_enumerates_without_panicking() {
        // May be empty (no mic / CI), but must not panic and each entry must carry an id + name.
        for d in audio_input_devices() {
            assert!(!d.id.is_empty(), "device id must be non-empty");
            assert!(!d.name.is_empty(), "device name must be non-empty");
        }
    }

    #[test]
    fn at_most_one_default_microphone() {
        let defaults = audio_input_devices().into_iter().filter(|d| d.default).count();
        assert!(defaults <= 1, "at most one system-default input device");
    }
}
