//! Per-session `events.jsonl` — the lifecycle log a client can tail (feature #26, port of
//! `core/events.py::EventsFileWriter`).
//!
//! It records the session **lifecycle**: every `state` transition + a counter `snapshot` line every
//! `CAPTURE_EVENTS_SNAPSHOT_SECONDS` (default 5 s) + one final snapshot on stop. High-volume events
//! (`log_line`, `screenshot_taken`, `transcript_segment`) are deliberately NOT persisted here — they
//! already live in `output.log` / `screenshots/` / `transcript.jsonl`; this file is the cheap thing a
//! client tails to follow progress without watching the artifact dirs.
//!
//! Lines: a state event is `{"t", "type": "state", "state", "session_id"}`; a snapshot is
//! `{"t", "type": "snapshot", "summary": <session summary>}`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;

use serde_json::{json, Value};

use capture_core::time::{iso, now};

/// The snapshot cadence in seconds: env `CAPTURE_EVENTS_SNAPSHOT_SECONDS`, default 5, floor 0.5.
pub(crate) fn snapshot_interval() -> f64 {
    std::env::var("CAPTURE_EVENTS_SNAPSHOT_SECONDS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(5.0)
        .max(0.5)
}

/// A session's append-only `events.jsonl`. Cheap to share (`Arc<EventsLog>`): `publish()` records
/// state transitions and the snapshot thread + `stop()` record snapshots, all behind one file lock.
pub(crate) struct EventsLog {
    file: Mutex<BufWriter<File>>,
}

impl EventsLog {
    /// Open (truncate) `<dir>/events.jsonl`. `None` if it can't be created — a broken events file must
    /// never break the capture, so the session just runs without one.
    pub(crate) fn open(dir: &Path) -> Option<EventsLog> {
        let f = File::create(dir.join("events.jsonl")).ok()?;
        Some(EventsLog { file: Mutex::new(BufWriter::new(f)) })
    }

    fn write(&self, ev: &Value) {
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{ev}");
            let _ = f.flush();
        }
    }

    /// Record a `state` transition (the event already carries `type`/`session_id`/`state`; we stamp `t`).
    pub(crate) fn record_state(&self, ev: &Value) {
        let mut line = ev.clone();
        if let Value::Object(ref mut m) = line {
            m.insert("t".into(), json!(iso(Some(now()))));
        }
        self.write(&line);
    }

    /// Record a counter `snapshot` carrying the current session summary.
    pub(crate) fn snapshot(&self, summary: &Value) {
        self.write(&json!({ "t": iso(Some(now())), "type": "snapshot", "summary": summary }));
    }
}
