//! capture-engine — the v3 capture session orchestration (a port of `core/session.py` +
//! `screenshots.py` + `audio.py`). A [`CaptureSession`] owns the lifecycle (created → starting →
//! running → stopping → stopped/error), the on-disk session directory (`session.json` +
//! `screenshots/` + `audio.s16le` + `transcript.{jsonl,txt}`), a screenshot timer thread, and the
//! audio→chunk→resample→silence-gate→ASR pump wired to [`capture_asr::AsrRuntimeManager::backend`].
//!
//! The on-disk layout is byte-compatible with the Python so the v3 read layer (`capture_core::sessions`)
//! and the GUI keep working. Modes: **attach** (pid / app / window) and **launch** (`command` → spawn a
//! child, tee its stdout/stderr to `stdout.log`/`stderr.log`/`output.log`, capture its window + audio).
//! Import is a follow-up.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::{json, Value};

use capture_asr::TARGET_SAMPLE_RATE;
use capture_core::time::{fs_stamp, iso, now};
use capture_core::v1;
use capture_platform::AudioTarget;

mod audio_worker;
mod events;
mod helpers;
mod import;
mod process;
mod retranscribe;

pub use import::import_media;
pub use retranscribe::retranscribe_session;

use audio_worker::{audio_worker, sink_into, AudioWorker};
use events::EventsLog;
use helpers::{
    expand, index_preset_for, non_empty, rand_hex6, resolve_shot_window, screenshot_opts,
    session_capabilities, sleep_interruptible,
};
use process::ProcessCapture;

/// A sink the session + its threads publish events to (the daemon forwards them to `/v1/events`).
/// Each value already carries `type` + `session_id`.
pub type EventSink = Arc<dyn Fn(Value) + Send + Sync>;

/// The resolved capture inputs (a `StartSessionRequest`).
#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub output_dir: String,
    /// Launch mode: a shell-style command to spawn (its pid becomes the capture target). When set,
    /// `pid`/`app_name`/`window_id` are ignored. `None` ⇒ attach mode.
    pub command: Option<String>,
    /// Working directory for a launched `command` (launch mode only; `None` ⇒ inherit the daemon's).
    pub cwd: Option<String>,
    pub pid: Option<i64>,
    pub window_id: Option<i64>,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub screenshot_interval: f64,
    pub screenshot_format: String,
    pub screenshot_resolution: Option<String>,
    pub screenshot_jpeg_quality: Option<u32>,
    pub capture_screenshots: bool,
    pub capture_audio: bool,
    pub audio_source: String,
    pub mic_device: Option<String>,
    pub audio_chunk_seconds: Option<f64>,
    pub asr_backend: String,
    pub preset: Option<String>,
}

#[derive(Default)]
struct Counters {
    screenshots: AtomicI64,
    screenshot_errors: AtomicI64,
    transcript_segments: AtomicI64,
    asr_errors: AtomicI64,
    mic_segments: AtomicI64,
}

struct Inner {
    state: String,
    t0: Option<f64>,
    t1: Option<f64>,
    pid: Option<i64>,
    window_title: Option<String>,
    mic_device: Option<String>, // the live mic device (switchable mid-capture)
    notes: Vec<String>,
    // The launched child (launch mode only) tee-ing its stdout/stderr to disk. Interior-mutable, so it
    // stays in Inner across stop() and the final summary still reports log_lines + process_running.
    proc: Option<Arc<ProcessCapture>>,
    shot_thread: Option<JoinHandle<()>>,
    audio_thread: Option<JoinHandle<()>>,
    // The SCStream capturing APP/SYSTEM audio only (the `Audio` output). `None` when the main track is
    // the mic (source=="mic"): SCK has nothing to capture then, since the mic comes via AVFoundation.
    audio_capture: Option<capture_platform::AudioCapture>,
    // The microphone, captured via AVFoundation (#88) for wideband 16 kHz, NOT via SCK's narrowband
    // captureMicrophone. Present whenever a mic is wanted (source=="mic" OR a separate mic track is on);
    // a mic switch / watchdog rebuild drops + replaces it. Independent of the SCK app stream above —
    // macOS runs an SCStream + an AVCaptureSession concurrently in one process.
    mic_capture: Option<capture_platform::MicCapture>,
    // The separate mic track's worker + its own stop flag (so it can be switched without stopping the
    // app worker). The mic feeds `mic_buf` from the AVFoundation capture above.
    mic_stop: Arc<AtomicBool>,
    mic_thread: Option<JoinHandle<()>>,
    // The periodic events.jsonl snapshot timer.
    events_thread: Option<JoinHandle<()>>,
}

/// One live capture. Cheap to share (`Arc<CaptureSession>`); the daemon keeps it in its live registry.
pub struct CaptureSession {
    id: String,
    dir: PathBuf,
    config: CaptureConfig,
    emit: EventSink,
    counters: Arc<Counters>,
    audio: Arc<Mutex<(String, String)>>, // (audio_mode, audio_status)
    mic: Arc<Mutex<(String, String)>>,   // (mic_mode, mic_status) — the separate mic track
    // The two worker input buffers (samples, source_rate). The single dual SCStream feeds these; the
    // workers drain them. They persist across stream rebuilds (a live mic switch), so the workers keep
    // running seamlessly. Mirrors the Python's two AudioCapture tracks, but from ONE stream.
    main_buf: Arc<Mutex<(Vec<i16>, u32)>>,
    mic_buf: Arc<Mutex<(Vec<i16>, u32)>>,
    // Last time (epoch millis) each output delivered audio — stamped by its sink. The audio watchdog
    // (#86) compares against `now()` to detect a SILENTLY stalled SCStream and rebuild it; the buffers
    // persist across the rebuild so the workers keep draining.
    main_last_audio: Arc<AtomicU64>,
    mic_last_audio: Arc<AtomicU64>,
    stop_flag: Arc<AtomicBool>,
    // The session's events.jsonl writer (the lifecycle log). `None` until `start()` opens it.
    events: Mutex<Option<Arc<EventsLog>>>,
    inner: Mutex<Inner>,
}

impl CaptureSession {
    /// Create a session (mints the id + dir; nothing is started or written yet).
    pub fn new(config: CaptureConfig, emit: EventSink) -> Self {
        let id = format!("{}-{}", fs_stamp(None), rand_hex6());
        let dir = expand(&config.output_dir).join(format!("capture-{id}"));
        CaptureSession {
            id,
            dir,
            config,
            emit,
            counters: Arc::new(Counters::default()),
            audio: Arc::new(Mutex::new(("off".into(), "off".into()))),
            mic: Arc::new(Mutex::new(("off".into(), "off".into()))),
            main_buf: Arc::new(Mutex::new((Vec::new(), TARGET_SAMPLE_RATE))),
            mic_buf: Arc::new(Mutex::new((Vec::new(), TARGET_SAMPLE_RATE))),
            main_last_audio: Arc::new(AtomicU64::new(0)),
            mic_last_audio: Arc::new(AtomicU64::new(0)),
            stop_flag: Arc::new(AtomicBool::new(false)),
            events: Mutex::new(None),
            inner: Mutex::new(Inner {
                state: "created".into(),
                t0: None,
                t1: None,
                pid: None,
                window_title: None,
                mic_device: None,
                notes: Vec::new(),
                proc: None,
                shot_thread: None,
                audio_thread: None,
                audio_capture: None,
                mic_capture: None,
                mic_stop: Arc::new(AtomicBool::new(false)),
                mic_thread: None,
                events_thread: None,
            }),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn state(&self) -> String {
        self.inner.lock().unwrap().state.clone()
    }

    /// Start the capture: create the dir, write `session.json`, resolve the target, and spawn the
    /// screenshot timer + the audio→ASR pump. Returns the summary. Mirrors `CaptureSession.start`.
    /// Takes `&Arc<Self>` so the `events.jsonl` snapshot thread can hold a `Weak<Self>` for `summary()`.
    pub fn start(self: &Arc<Self>) -> Result<Value, String> {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.state != "created" {
                return Err(format!("session already {}", inner.state));
            }
            std::fs::create_dir_all(&self.dir)
                .map_err(|e| format!("create {}: {e}", self.dir.display()))?;
            inner.t0 = Some(now());
            inner.state = "starting".into();
            self.write_metadata(&inner);
        }
        self.start_events_log(); // opens events.jsonl + the snapshot timer, before the first state event
        self.publish("state", json!({ "state": "starting" }));

        // Resolve the capture target (pid + a window title to label the session) outside the lock.
        // Launch mode spawns `command` and uses ITS pid; attach mode resolves pid/app/window.
        let (pid, title) = if let Some(command) = self.config.command.clone() {
            match ProcessCapture::start(
                &command,
                &self.dir,
                self.config.cwd.as_deref(),
                self.emit.clone(),
                self.id.clone(),
            ) {
                Ok(proc) => {
                    let pid = proc.pid();
                    self.inner.lock().unwrap().proc = Some(Arc::new(proc));
                    (pid, None)
                }
                Err(e) => {
                    // A launch-mode session whose command never started has captured nothing — fail
                    // loudly (state "error") rather than report a phantom 'running'. Mirrors session.py.
                    let mut inner = self.inner.lock().unwrap();
                    inner.notes.push(format!("launch failed: {e}"));
                    inner.state = "error".into();
                    self.write_metadata(&inner);
                    drop(inner);
                    self.publish("state", json!({ "state": "error" }));
                    self.stop_flag.store(true, Ordering::Relaxed); // let the events thread exit
                    return Err(format!("could not launch command: {e}"));
                }
            }
        } else {
            self.resolve_target()
        };
        {
            let mut inner = self.inner.lock().unwrap();
            inner.pid = pid;
            inner.window_title = title;
        }

        if self.config.capture_screenshots {
            let handle = self.spawn_screenshots(pid);
            self.inner.lock().unwrap().shot_thread = Some(handle);
        }
        if self.config.capture_audio {
            // The main track's worker (audio.s16le / transcript.*), draining main_buf.
            let handle = self.spawn_worker(
                "audio",
                false,
                self.audio.clone(),
                self.stop_flag.clone(),
                self.main_buf.clone(),
            );
            self.inner.lock().unwrap().audio_thread = Some(handle);
        }
        // A separate mic track (chosen at start) writes mic.s16le / mic_transcript.* alongside the app
        // audio — fed by the SAME dual stream's Microphone output. (When audio_source == "mic" the mic
        // IS the main track, so no separate worker.)
        if self.config.mic_device.is_some() && self.config.audio_source != "mic" {
            self.inner.lock().unwrap().mic_device = self.config.mic_device.clone();
            let mic_stop = self.inner.lock().unwrap().mic_stop.clone();
            let handle =
                self.spawn_worker("mic", false, self.mic.clone(), mic_stop, self.mic_buf.clone());
            self.inner.lock().unwrap().mic_thread = Some(handle);
        } else if self.config.audio_source == "mic" {
            self.inner.lock().unwrap().mic_device = self.config.mic_device.clone();
        }
        // Build the audio captures feeding the worker buffers: the SCK app/system-audio stream and/or
        // the AVFoundation mic capture (#88) — independent handles, run concurrently.
        if self.config.capture_audio || self.config.audio_source == "mic" {
            // Start the delivery clocks NOW so the watchdog gives the fresh streams a grace window.
            let nowms = (now() * 1000.0) as u64;
            self.main_last_audio.store(nowms, Ordering::Relaxed);
            self.mic_last_audio.store(nowms, Ordering::Relaxed);
            match self.build_audio_stream() {
                Ok(cap) => self.inner.lock().unwrap().audio_capture = cap,
                Err(e) => {
                    *self.audio.lock().unwrap() = ("none".into(), format!("audio-start-failed: {e}"));
                    self.inner.lock().unwrap().notes.push(format!("audio: {e}"));
                }
            }
            match self.build_mic_capture() {
                Ok(mic) => self.inner.lock().unwrap().mic_capture = mic,
                Err(e) => {
                    // A separate mic track failing shouldn't sink the app audio; surface it on the mic
                    // (or main, when the mic IS the main track) status + as a note.
                    if self.config.audio_source == "mic" {
                        *self.audio.lock().unwrap() = ("none".into(), format!("mic-start-failed: {e}"));
                    } else {
                        *self.mic.lock().unwrap() = ("none".into(), format!("mic-start-failed: {e}"));
                    }
                    self.inner.lock().unwrap().notes.push(format!("mic: {e}"));
                }
            }
            self.spawn_audio_watchdog(); // self-heal a silently stalled app stream / mic capture (#86)
        }

        {
            let mut inner = self.inner.lock().unwrap();
            inner.state = "running".into();
            self.write_metadata(&inner);
        }
        self.publish("state", json!({ "state": "running" }));
        Ok(self.summary())
    }

    /// Stop the capture: tear down the threads/stream, flush the final audio chunk, write the final
    /// `session.json`. Idempotent-ish — a no-op summary if not running. Mirrors `CaptureSession.stop`.
    pub fn stop(&self) -> Value {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.state != "running" {
                return self.summary_locked(&inner);
            }
            inner.state = "stopping".into();
        }
        self.publish("state", json!({ "state": "stopping" }));

        // Stop both audio captures FIRST (no more samples) — the SCK app stream and the AVFoundation mic
        // capture (#88) — then signal the workers + screenshot loop, then join them. Dropping/stopping a
        // capture ends its callbacks.
        let (acap, mcap, mic_stop) = {
            let mut inner = self.inner.lock().unwrap();
            (inner.audio_capture.take(), inner.mic_capture.take(), inner.mic_stop.clone())
        };
        if let Some(cap) = acap {
            let _ = cap.stop();
        }
        if let Some(cap) = mcap {
            let _ = cap.stop();
        }
        self.stop_flag.store(true, Ordering::Relaxed);
        mic_stop.store(true, Ordering::Relaxed);
        let (shot, audio, mic, events_t) = {
            let mut inner = self.inner.lock().unwrap();
            (
                inner.shot_thread.take(),
                inner.audio_thread.take(),
                inner.mic_thread.take(),
                inner.events_thread.take(),
            )
        };
        for h in [shot, audio, mic, events_t].into_iter().flatten() {
            let _ = h.join();
        }

        // Terminate the launched child (launch mode) OUTSIDE the lock — SIGTERM waits up to 5s. The
        // proc stays in Inner (interior-mutable) so the final summary still reports log_lines +
        // process_running=false. `None` exit code renders as "None" (killed by signal).
        let proc = self.inner.lock().unwrap().proc.clone();
        let exit_code = proc.map(|p| p.stop());

        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(code) = exit_code {
                let rc = code.map(|c| c.to_string()).unwrap_or_else(|| "None".into());
                inner.notes.push(format!("process exit code: {rc}"));
            }
            inner.t1 = Some(now());
            inner.state = "stopped".into();
            self.write_metadata(&inner);
        }
        // A healthy audio run becomes "stopped (asr-errors=N)" if any chunk failed.
        {
            let mut a = self.audio.lock().unwrap();
            if !["failed", "unavailable", "no-audio-source", "off"].iter().any(|k| a.1.contains(k)) {
                let errs = self.counters.asr_errors.load(Ordering::Relaxed);
                a.1 = if errs > 0 { format!("stopped (asr-errors={errs})") } else { "stopped".into() };
            }
        }
        self.publish("state", json!({ "state": "stopped" }));
        // Final snapshot — the last line of events.jsonl, with the terminal counters.
        if let Some(log) = self.events.lock().unwrap().clone() {
            log.snapshot(&self.summary());
        }
        self.summary()
    }

    /// Switch the microphone on a RUNNING capture. `device` = an input-device id (`"default"` = the
    /// system default) turns the mic on / switches it; `None`/`""` turns it off. The mic track APPENDS
    /// to `mic.s16le` so the recording stays continuous. Errors if the session isn't running. Mirrors
    /// `CaptureSession.set_mic_device`.
    ///
    /// The mic is captured via AVFoundation (#88), independent of the SCK app-audio stream — switching
    /// drops + rebuilds ONLY the mic's AVCaptureSession (the app stream keeps feeding `main_buf`
    /// untouched). Turning off drops the mic capture entirely.
    pub fn set_mic_device(&self, device: Option<&str>) -> Result<Value, String> {
        if self.state() != "running" {
            return Err(format!(
                "can only switch the mic on a running capture (state={})",
                self.state()
            ));
        }
        let device = device.map(str::to_string).filter(|s| !s.is_empty());
        if device == self.inner.lock().unwrap().mic_device {
            return Ok(self.summary());
        }
        let turning_on = device.is_some();
        self.inner.lock().unwrap().mic_device = device.clone();

        // Drop the old AVFoundation mic capture FIRST, clear stale buffered samples, then build the new
        // one (or none, when turning off). The SCK app stream is left alone — they're separate handles.
        let old = self.inner.lock().unwrap().mic_capture.take();
        if let Some(cap) = old {
            let _ = cap.stop();
        }
        self.mic_buf.lock().unwrap().0.clear(); // discard any stale pre-switch samples
        match self.build_mic_capture() {
            Ok(mic) => {
                self.inner.lock().unwrap().mic_capture = mic;
                // Refresh the mic delivery clock so the watchdog grants the fresh capture a grace window.
                self.mic_last_audio.store((now() * 1000.0) as u64, Ordering::Relaxed);
            }
            Err(e) => {
                *self.mic.lock().unwrap() = ("none".into(), format!("mic-start-failed: {e}"));
                self.inner.lock().unwrap().notes.push(format!("mic switch failed: {e}"));
            }
        }

        if turning_on {
            // Spawn the mic worker if not already running (append to keep mic.s16le continuous). The
            // mic_buf was already cleared above before the new AVF capture started.
            if self.inner.lock().unwrap().mic_thread.is_none() {
                let append = self.dir.join("mic.s16le").exists();
                let mic_stop = self.inner.lock().unwrap().mic_stop.clone();
                let handle =
                    self.spawn_worker("mic", append, self.mic.clone(), mic_stop, self.mic_buf.clone());
                self.inner.lock().unwrap().mic_thread = Some(handle);
            }
        } else {
            // Turning off: stop + join the mic worker, then reset its flag for a future on.
            let (stop, thread) = {
                let mut inner = self.inner.lock().unwrap();
                (inner.mic_stop.clone(), inner.mic_thread.take())
            };
            stop.store(true, Ordering::Relaxed);
            if let Some(h) = thread {
                let _ = h.join();
            }
            self.inner.lock().unwrap().mic_stop = Arc::new(AtomicBool::new(false));
            *self.mic.lock().unwrap() = ("off".into(), "off".into());
        }

        {
            let inner = self.inner.lock().unwrap();
            self.write_metadata(&inner);
        }
        self.publish("mic_device", json!({ "device": device }));
        Ok(self.summary())
    }

    // -- target resolution ---------------------------------------------------

    fn resolve_target(&self) -> (Option<i64>, Option<String>) {
        let mut pid = self.config.pid;
        let mut title: Option<String> = None;
        if pid.is_none() {
            if let Some(app) = self.config.app_name.as_deref() {
                if let Ok(wins) = capture_platform::list_windows(None, Some(app)) {
                    if let Some(w) = wins.first() {
                        pid = Some(w.pid as i64);
                        title = Some(non_empty(&w.title, &w.app_name));
                    }
                }
            }
        }
        // Label with the picked window's own title (its two windows share a pid otherwise).
        if let Some(wid) = self.config.window_id {
            let p = pid.map(|p| p as i32);
            if let Ok(wins) = capture_platform::list_windows(p, self.config.app_name.as_deref()) {
                if let Some(w) = wins.into_iter().find(|w| w.window_id as i64 == wid) {
                    title = Some(non_empty(&w.title, &w.app_name));
                }
            }
        }
        (pid, title)
    }

    // -- screenshots ---------------------------------------------------------

    fn spawn_screenshots(&self, pid: Option<i64>) -> JoinHandle<()> {
        let (opts, ext, _) = screenshot_opts(&self.config);
        let dir = self.dir.join("screenshots");
        let interval = self.config.screenshot_interval.max(0.05);
        let window_id = self.config.window_id;
        let app_name = if pid.is_some() { None } else { self.config.app_name.clone() };
        let pid32 = pid.map(|p| p as i32);
        let counters = self.counters.clone();
        let emit = self.emit.clone();
        let stop = self.stop_flag.clone();
        let id = self.id.clone();

        std::thread::Builder::new()
            .name("screenshotter".into())
            .spawn(move || {
                let _ = std::fs::create_dir_all(&dir);
                let mut next = now();
                while !stop.load(Ordering::Relaxed) {
                    let ts = now();
                    let wid = resolve_shot_window(window_id, pid32, app_name.as_deref());
                    match capture_platform::capture_screenshot(wid, &opts) {
                        Ok(bytes) => {
                            let path = dir.join(format!("{}.{ext}", fs_stamp(Some(ts))));
                            if std::fs::write(&path, &bytes).is_ok() {
                                let n = counters.screenshots.fetch_add(1, Ordering::Relaxed) + 1;
                                emit(json!({ "type": "screenshot_taken", "session_id": id,
                                    "path": path.to_string_lossy(), "count": n }));
                            } else {
                                let n = counters.screenshot_errors.fetch_add(1, Ordering::Relaxed) + 1;
                                emit(json!({ "type": "screenshot_error", "session_id": id, "errors": n }));
                            }
                        }
                        Err(_) => {
                            let n = counters.screenshot_errors.fetch_add(1, Ordering::Relaxed) + 1;
                            emit(json!({ "type": "screenshot_error", "session_id": id, "errors": n }));
                        }
                    }
                    // Stay on an absolute grid even if a capture took a fraction of the interval.
                    next += interval;
                    let mut sleep_for = next - now();
                    if sleep_for < 0.0 {
                        let missed = ((-sleep_for) / interval) as i64 + 1;
                        next += missed as f64 * interval;
                        sleep_for = (next - now()).max(0.0);
                    }
                    sleep_interruptible(sleep_for, &stop);
                }
            })
            .expect("spawn screenshotter")
    }

    // -- audio → ASR ---------------------------------------------------------

    /// Spawn one track's worker thread, draining `buf` (which the dual SCStream feeds). `track`
    /// ("audio"/"mic") selects the output file names + the SSE label; `append` continues an existing
    /// track (a live mic switch keeps `mic.s16le` going); `status`/`stop` are the per-track state + flag.
    fn spawn_worker(
        &self,
        track: &'static str,
        append: bool,
        status: Arc<Mutex<(String, String)>>,
        stop: Arc<AtomicBool>,
        buf: Arc<Mutex<(Vec<i16>, u32)>>,
    ) -> JoinHandle<()> {
        let mode = if track == "audio" && self.config.audio_source != "mic" { "app" } else { "mic" };
        *status.lock().unwrap() = (mode.into(), "running".into());
        let dir = self.dir.clone();
        let chunk_seconds = self
            .config
            .audio_chunk_seconds
            .unwrap_or_else(capture_asr::models::active_chunk_seconds)
            .max(1.0);
        let t0 = self.inner.lock().unwrap().t0.unwrap_or_else(now);
        let counters = self.counters.clone();
        let emit = self.emit.clone();
        let id = self.id.clone();
        std::thread::Builder::new()
            .name(format!("{track}-worker"))
            .spawn(move || {
                audio_worker(AudioWorker {
                    dir, shared: buf, chunk_seconds, t0, counters, status, emit, stop, id, track, append,
                });
            })
            .expect("spawn audio worker")
    }

    /// (Re)build the SCStream that captures **app/system audio only** (the `Audio` output → `main_buf`).
    /// `Ok(None)` when there's no app audio to capture — i.e. the main track IS the mic
    /// (source=="mic"): SCK then has nothing to do, because the mic is captured via AVFoundation in
    /// [`Self::build_mic_capture`] instead (#88). The mic is NEVER routed through SCK here, so the
    /// 8 kHz narrowband captureMicrophone path is gone. macOS runs this SCStream and the mic's
    /// AVCaptureSession concurrently in one process (the no-two-SCStreams limit doesn't cross frameworks).
    fn build_audio_stream(&self) -> Result<Option<capture_platform::AudioCapture>, String> {
        let source_is_mic = self.config.audio_source == "mic";
        let pid = self.inner.lock().unwrap().pid;

        // Audio output = the app's audio, only when the main track is app audio.
        let app_target = (self.config.capture_audio && !source_is_mic).then(|| AudioTarget::App {
            pid: pid.map(|p| p as i32),
            bundle_id: self.config.bundle_id.clone(),
        });
        // No app audio to capture (the mic IS the main track) → no SCStream; the mic comes from AVF.
        let Some(_) = app_target.as_ref() else {
            return Ok(None);
        };

        // Build the SCStream with NO mic output (the mic now comes from AVFoundation). `on_mic` is a
        // no-op sink that's never invoked, kept only to satisfy the dual API's signature.
        let on_audio = sink_into(self.main_buf.clone(), self.main_last_audio.clone());
        let on_mic: capture_platform::AudioCallback = Box::new(|_: &[i16], _: u32| {});

        capture_platform::start_audio_capture_dual(app_target.as_ref(), None, on_audio, on_mic)
            .map(Some)
    }

    /// (Re)build the AVFoundation microphone capture (#88) when a mic is wanted — the main track is the
    /// mic (source=="mic") OR a separate mic track is on. Captures the selected device at its native
    /// WIDEBAND rate (16 kHz mSBC for a BT headset), feeding the right worker buffer: `main_buf` when the
    /// mic is the main track, else the separate `mic_buf`. `Ok(None)` when no mic is wanted. The worker's
    /// #87 empirical rate detection still runs as the safety net.
    fn build_mic_capture(&self) -> Result<Option<capture_platform::MicCapture>, String> {
        let source_is_mic = self.config.audio_source == "mic";
        let mic_dev = self.inner.lock().unwrap().mic_device.clone();
        let want_mic = source_is_mic || mic_dev.is_some();
        if !want_mic {
            return Ok(None);
        }
        // Route to the main buffer when the mic IS the main track, else the separate mic buffer.
        let on_mic = sink_into(
            if source_is_mic { self.main_buf.clone() } else { self.mic_buf.clone() },
            if source_is_mic { self.main_last_audio.clone() } else { self.mic_last_audio.clone() },
        );
        let dev = mic_dev.as_deref().unwrap_or("default");
        capture_platform::start_mic_capture(dev, on_mic).map(Some)
    }

    /// Audio watchdog (#86): a macOS audio capture can SILENTLY stop delivering sample buffers (no
    /// error), which starves the ASR worker and freezes the live transcript. Every couple of seconds,
    /// check whether each ACTIVE source has gone quiet longer than a live stream ever would; if so,
    /// rebuild JUST that source (the worker buffers persist, so transcription resumes) and note it. The
    /// stalled handle is dropped FIRST. Since #88 there are two independent handles: the SCK app stream
    /// (`audio_capture`, rebuilt via `build_audio_stream`) and the AVFoundation mic (`mic_capture`,
    /// rebuilt via `build_mic_capture`). Exits on `stop_flag`; rebuilds are rate-limited per source.
    fn spawn_audio_watchdog(self: &Arc<Self>) {
        const STALL_SECS: f64 = 8.0;
        let sess = self.clone();
        let stop = self.stop_flag.clone();
        std::thread::Builder::new()
            .name("audio-watchdog".into())
            .spawn(move || {
                let mut last_app_rebuild_ms: u64 = 0;
                let mut last_mic_rebuild_ms: u64 = 0;
                loop {
                    sleep_interruptible(2.0, &stop);
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let now_ms = (now() * 1000.0) as u64;
                    let app_active = sess.config.capture_audio && sess.config.audio_source != "mic";
                    let mic_active =
                        sess.config.audio_source == "mic" || sess.inner.lock().unwrap().mic_device.is_some();
                    let stale = |last: &AtomicU64| {
                        now_ms.saturating_sub(last.load(Ordering::Relaxed)) as f64 / 1000.0 > STALL_SECS
                    };
                    let throttled = |last_rebuild: u64| {
                        now_ms.saturating_sub(last_rebuild) as f64 / 1000.0 < STALL_SECS
                    };
                    let main_stale = app_active && stale(&sess.main_last_audio);
                    let mic_stale = mic_active && stale(&sess.mic_last_audio);

                    // Rebuild the SCK app stream if it stalled (drop FIRST, refresh its clock only).
                    if main_stale && !throttled(last_app_rebuild_ms) {
                        last_app_rebuild_ms = now_ms;
                        {
                            let mut inner = sess.inner.lock().unwrap();
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            inner.audio_capture = None;
                        }
                        match sess.build_audio_stream() {
                            Ok(cap) => {
                                let mut inner = sess.inner.lock().unwrap();
                                if stop.load(Ordering::Relaxed) {
                                    drop(cap);
                                    break;
                                }
                                inner.audio_capture = cap;
                                inner.notes.push("audio reconnected (app audio stream had stalled)".into());
                                drop(inner);
                                sess.main_last_audio.store((now() * 1000.0) as u64, Ordering::Relaxed);
                                (sess.emit)(json!({ "type": "audio_reconnect", "session_id": sess.id, "track": "app audio" }));
                            }
                            Err(e) => eprintln!("captured: audio watchdog app-stream rebuild failed: {e}"),
                        }
                    }

                    // Rebuild the AVFoundation mic if it stalled (drop FIRST, refresh its clock only).
                    if mic_stale && !throttled(last_mic_rebuild_ms) {
                        last_mic_rebuild_ms = now_ms;
                        {
                            let mut inner = sess.inner.lock().unwrap();
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            inner.mic_capture = None;
                        }
                        match sess.build_mic_capture() {
                            Ok(cap) => {
                                let mut inner = sess.inner.lock().unwrap();
                                if stop.load(Ordering::Relaxed) {
                                    drop(cap);
                                    break;
                                }
                                inner.mic_capture = cap;
                                inner.notes.push("audio reconnected (mic capture had stalled)".into());
                                drop(inner);
                                sess.mic_last_audio.store((now() * 1000.0) as u64, Ordering::Relaxed);
                                (sess.emit)(json!({ "type": "audio_reconnect", "session_id": sess.id, "track": "mic" }));
                            }
                            Err(e) => eprintln!("captured: audio watchdog mic rebuild failed: {e}"),
                        }
                    }
                }
            })
            .expect("spawn audio watchdog");
    }

    // -- reporting -----------------------------------------------------------

    /// The session summary (the `/v1/sessions/{id}` + `session.json` shape), built as a
    /// [`v1::Session`] so it can't drift from the read layer.
    pub fn summary(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        self.summary_locked(&inner)
    }

    fn summary_locked(&self, inner: &Inner) -> Value {
        let (audio_mode, audio_status) = {
            let a = self.audio.lock().unwrap();
            (a.0.clone(), a.1.clone())
        };
        // mic_status: the live track's status when a mic device is active, else "off".
        let mic_status = match &inner.mic_device {
            Some(_) => self.mic.lock().unwrap().1.clone(),
            None => "off".to_string(),
        };
        // log_lines / process_running come from the launched child (launch mode); None in attach mode.
        let (log_lines, process_running) = match &inner.proc {
            Some(p) => (p.lines(), Some(p.is_running())),
            None => (0, None),
        };
        let caps = session_capabilities(&self.dir);
        let s = v1::Session {
            session_id: self.id.clone(),
            state: inner.state.clone(),
            dir: self.dir.to_string_lossy().into_owned(),
            pid: inner.pid,
            window_title: inner.window_title.clone(),
            started_at: inner.t0.map(|t| iso(Some(t))),
            stopped_at: inner.t1.map(|t| iso(Some(t))),
            screenshots: self.counters.screenshots.load(Ordering::Relaxed),
            screenshot_errors: self.counters.screenshot_errors.load(Ordering::Relaxed),
            log_lines,
            process_running,
            audio_mode,
            audio_status,
            transcript_segments: self.counters.transcript_segments.load(Ordering::Relaxed),
            asr_errors: self.counters.asr_errors.load(Ordering::Relaxed),
            mic_status,
            mic_segments: self.counters.mic_segments.load(Ordering::Relaxed),
            mic_device: inner.mic_device.clone(),
            capture_preset: self.config.preset.clone(),
            index_preset: Some(index_preset_for(self.config.preset.as_deref())),
            has_screenshots: caps.0,
            has_audio: caps.1,
            has_mic: caps.2,
            can_retranscribe: caps.1,
            can_index: caps.0,
            notes: inner.notes.clone(),
        };
        serde_json::to_value(s).unwrap_or(Value::Null)
    }

    fn write_metadata(&self, inner: &Inner) {
        let c = &self.config;
        let meta = json!({
            "config": {
                "command": c.command,
                "pid": c.pid,
                "app_name": c.app_name,
                "bundle_id": c.bundle_id,
                "screenshot_interval": c.screenshot_interval,
                "screenshot_format": screenshot_opts(c).2,
                "screenshot_resolution": c.screenshot_resolution,
                "screenshot_jpeg_quality": c.screenshot_jpeg_quality,
                "capture_screenshots": c.capture_screenshots,
                "capture_audio": c.capture_audio,
                "audio_source": c.audio_source,
                "mic_device": c.mic_device,
                "audio_chunk_seconds": c.audio_chunk_seconds
                    .unwrap_or_else(capture_asr::models::active_chunk_seconds),
                "capture_preset": c.preset,
                "index_preset": index_preset_for(c.preset.as_deref()),
                "asr_backend": c.asr_backend,
                "cwd": c.cwd,
            },
            "summary": self.summary_locked(inner),
        });
        if let Ok(body) = serde_json::to_string_pretty(&meta) {
            let _ = std::fs::write(self.dir.join("session.json"), body);
        }
    }

    /// Open `events.jsonl` + spawn the snapshot timer. The thread holds a `Weak<Self>` (so it never
    /// keeps the session alive) and writes a counter snapshot every `snapshot_interval()` until the
    /// session stops or is dropped. No-op if the file can't be created. Called once from `start()`.
    fn start_events_log(self: &Arc<Self>) {
        let Some(log) = EventsLog::open(&self.dir) else { return };
        let log = Arc::new(log);
        *self.events.lock().unwrap() = Some(log.clone());

        let weak = Arc::downgrade(self);
        let stop = self.stop_flag.clone();
        let interval = events::snapshot_interval();
        let handle = std::thread::Builder::new()
            .name("events-writer".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    sleep_interruptible(interval, &stop);
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    match weak.upgrade() {
                        Some(s) => log.snapshot(&s.summary()),
                        None => break, // session dropped — nothing left to snapshot
                    }
                }
            })
            .expect("spawn events writer");
        self.inner.lock().unwrap().events_thread = Some(handle);
    }

    fn publish(&self, type_: &str, mut fields: Value) {
        if let Value::Object(ref mut m) = fields {
            m.insert("type".into(), Value::String(type_.into()));
            m.insert("session_id".into(), Value::String(self.id.clone()));
        }
        // events.jsonl persists only state transitions (the lifecycle); high-volume events (log_line,
        // screenshot_taken, transcript_segment) already live in their own files.
        if type_ == "state" {
            if let Some(log) = self.events.lock().unwrap().clone() {
                log.record_state(&fields);
            }
        }
        (self.emit)(fields);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::{parse_resolution, round3};

    #[test]
    fn parse_resolution_variants() {
        assert_eq!(parse_resolution(Some("1280x720")), (Some((1280, 720)), None));
        assert_eq!(parse_resolution(Some("1280x720/jpg")), (Some((1280, 720)), Some("jpg".into())));
        assert_eq!(parse_resolution(Some(" 800X600 ")), (Some((800, 600)), None));
        assert_eq!(parse_resolution(None), (None, None));
        assert_eq!(parse_resolution(Some("garbage")), (None, None));
        assert_eq!(parse_resolution(Some("0x10")), (None, None)); // dims must be >= 1
    }

    #[test]
    fn index_preset_mapping() {
        assert_eq!(index_preset_for(Some("meeting")), "meeting");
        assert_eq!(index_preset_for(Some("general")), "auto");
        assert_eq!(index_preset_for(None), "auto");
        assert_eq!(index_preset_for(Some("whatever")), "auto");
    }

    #[test]
    fn round3_rounds() {
        assert_eq!(round3(1.23456), 1.235);
        assert_eq!(round3(2.0), 2.0);
    }

    #[test]
    fn id_and_dir_shape() {
        let cfg = CaptureConfig {
            output_dir: "/tmp/cap-test".into(),
            command: None,
            cwd: None,
            pid: Some(123),
            window_id: None,
            app_name: None,
            bundle_id: None,
            screenshot_interval: 1.0,
            screenshot_format: "png".into(),
            screenshot_resolution: None,
            screenshot_jpeg_quality: None,
            capture_screenshots: true,
            capture_audio: false,
            audio_source: "auto".into(),
            mic_device: None,
            audio_chunk_seconds: Some(30.0),
            asr_backend: "auto".into(),
            preset: None,
        };
        let sink: EventSink = Arc::new(|_| {});
        let s = CaptureSession::new(cfg, sink);
        assert!(s.dir().to_string_lossy().contains("/tmp/cap-test/capture-"));
        assert_eq!(s.state(), "created");
        // id = <fs_stamp>-<6 hex>
        let suffix = s.id().rsplit('-').next().unwrap();
        assert_eq!(suffix.len(), 6);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Launch mode tees a command's stdout/stderr to the three log files and reports the exit code +
    /// line count. Hermetic (no TCC/window-server) — exercises the `core/proc.py` contract directly.
    #[test]
    fn process_capture_tees_streams_and_reaps() {
        let dir = std::env::temp_dir().join(format!("cap-proc-{}", rand_hex6()));
        std::fs::create_dir_all(&dir).unwrap();
        let sink: EventSink = Arc::new(|_| {});
        let proc = ProcessCapture::start(
            "sh -c 'echo hello; echo oops 1>&2; exit 7'",
            &dir,
            None,
            sink,
            "test-session".into(),
        )
        .expect("spawn");
        assert!(proc.pid().is_some());

        // Let the child finish on its own (it exits 7 immediately), then stop() reaps the real code
        // and joins the pumps (flushing the logs). A stop() before exit would SIGTERM it instead.
        for _ in 0..300 {
            if !proc.is_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let code = proc.stop();
        assert_eq!(code, Some(7), "exit code should propagate");
        assert!(!proc.is_running());

        let stdout = std::fs::read_to_string(dir.join("stdout.log")).unwrap();
        let stderr = std::fs::read_to_string(dir.join("stderr.log")).unwrap();
        let merged = std::fs::read_to_string(dir.join("output.log")).unwrap();
        assert_eq!(stdout, "hello\n");
        assert_eq!(stderr, "oops\n");
        // Merged lines are timestamped + stream-tagged: `<iso> [out] hello` / `<iso> [err] oops`.
        assert!(merged.contains("[out] hello\n"), "merged out: {merged:?}");
        assert!(merged.contains("[err] oops\n"), "merged err: {merged:?}");
        assert_eq!(proc.lines(), 2, "one merged line per source line");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
