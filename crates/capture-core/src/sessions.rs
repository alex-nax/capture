//! On-disk SESSION READ layer — a 1:1 port of the read half of `core/registry.py`
//! (`_scan_runs_dir`, `_recover`, `summaries`/`summary`/`history_record`) plus the
//! capability derivation from `core/session.py` (`session_capabilities`).
//!
//! Pure file/JSON logic: read a session dir's `session.json` → a [`v1::Session`] summary,
//! enumerate the runs dir, and merge the append-only index (`sessions.jsonl`). NO capture /
//! process / live logic (that lands in #65) — every record produced here is a read-only
//! recovery from disk.
//!
//! ## Precedence (mirrors `SessionRegistry._recover`)
//!  1. Start from a default-filled template.
//!  2. Read `<dir>/session.json` → its `"summary"` block; on missing/unreadable, return the
//!     template with a "session.json missing or unreadable" note.
//!  3. Merge the recorded summary over the template; force `session_id`/`dir`.
//!  4. If the recorded `state` is a LIVE state (`starting`/`running`/`stopping`) — a process
//!     that is gone left it that way — rewrite it to `"interrupted"` and append a note.
//!  5. RE-derive the capability flags from the *current* on-disk artifacts (mirrors
//!     `_with_caps`/`summaries`), so a pruned session reports the truth, not the flags frozen
//!     into session.json. Counts (`screenshots`/`transcript_segments`) come from the recorded
//!     summary, falling back to an on-disk derivation when the summary omits them.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::transcript::load_transcript;
use crate::v1;

/// Live states: a session recorded in one of these by a process that has since exited was
/// interrupted. Copy of `registry._LIVE_STATES`.
const LIVE_STATES: [&str; 3] = ["starting", "running", "stopping"];

/// Newest-only retention bound — copy of `registry.MAX_SESSIONS`.
const MAX_SESSIONS: usize = 100;

/// The `capture-` prefix on a session dir name; the suffix is the session id.
const DIR_PREFIX: &str = "capture-";

/// The runs dir scanned for session folders: env `CAPTURE_RUNS_DIR` else `~/.capture/runs`.
/// Mirrors `registry._scan_runs_dir`'s path resolution.
pub fn runs_dir() -> PathBuf {
    match std::env::var_os("CAPTURE_RUNS_DIR") {
        Some(v) if !v.is_empty() => expanduser(Path::new(&v)),
        _ => home().join(".capture").join("runs"),
    }
}

/// The append-only session index: env `CAPTURE_SESSION_INDEX` else `~/.capture/sessions.jsonl`.
/// Mirrors `registry.default_index_path`.
pub fn sessions_index_path() -> PathBuf {
    match std::env::var_os("CAPTURE_SESSION_INDEX") {
        Some(v) if !v.is_empty() => expanduser(Path::new(&v)),
        _ => home().join(".capture").join("sessions.jsonl"),
    }
}

/// `$HOME` (`Path::home()` equivalent). Falls back to `.` so callers never panic.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Expand a leading `~` to `$HOME` (mirrors `Path(...).expanduser()`).
fn expanduser(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        return home().join(rest);
    }
    if s == "~" {
        return home();
    }
    p.to_path_buf()
}

/// Capabilities + counts derived from a session's on-disk artifacts — a port of
/// `session.session_capabilities` (the flags) extended with the artifact COUNTS the recovery
/// path falls back to. Recomputed on every read so pruning is reflected immediately.
///
/// Flags (exactly the Python `session_capabilities` dict):
/// - `has_screenshots`: `screenshots/` is a dir with at least one entry.
/// - `has_audio`: `audio.s16le` is a non-empty file.
/// - `has_mic`: `mic.s16le` is a non-empty file.
/// - `can_retranscribe` = `has_audio` (app audio still present).
/// - `can_index` = `has_screenshots` (the multimodal index needs frames to caption).
///
/// Counts (used only as a fallback when the recorded summary omits them):
/// - `screenshot_count`: number of files directly under `screenshots/`.
/// - `transcript_segments`: number of valid `transcript.jsonl` segments.
struct Caps {
    has_screenshots: bool,
    has_audio: bool,
    has_mic: bool,
    can_retranscribe: bool,
    can_index: bool,
    screenshot_count: i64,
    transcript_segments: i64,
}

fn session_capabilities(dir: &Path) -> Caps {
    // `nonempty(name)`: an existing regular file with size > 0.
    let nonempty = |name: &str| -> bool {
        std::fs::metadata(dir.join(name))
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
    };

    let shots = dir.join("screenshots");
    // `has_shots`: the dir exists and has at least one entry (`any(shots.iterdir())`).
    let has_shots = shots
        .is_dir()
        .then(|| {
            std::fs::read_dir(&shots)
                .map(|mut it| it.next().is_some())
                .unwrap_or(false)
        })
        .unwrap_or(false);

    // `screenshot_count`: files directly under `screenshots/` (mirrors `prune`'s recount).
    let screenshot_count = std::fs::read_dir(&shots)
        .map(|it| {
            it.flatten()
                .filter(|e| e.path().is_file())
                .count() as i64
        })
        .unwrap_or(0);

    let has_audio = nonempty("audio.s16le");
    Caps {
        has_screenshots: has_shots,
        has_audio,
        has_mic: nonempty("mic.s16le"),
        can_retranscribe: has_audio,
        can_index: has_shots,
        screenshot_count,
        transcript_segments: load_transcript(dir).len() as i64,
    }
}

/// A string field from the recorded summary block (None when absent or not a string).
fn s_str(summary: &Value, key: &str) -> Option<String> {
    summary.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// An i64 field from the recorded summary block (None when absent or not an integer).
fn s_i64(summary: &Value, key: &str) -> Option<i64> {
    summary.get(key).and_then(|v| v.as_i64())
}

/// Recover a read-only [`v1::Session`] for one session dir, by reading its `session.json` and
/// re-deriving the capability flags from the current on-disk artifacts. Port of
/// `SessionRegistry._recover` (the `dir`/`session_id` are taken from the path).
///
/// Returns `None` only when the path is not a session dir (no `session.json`) — mirroring
/// `_scan_runs_dir`'s `if not (d / "session.json").exists(): continue` guard. (`_recover`
/// itself tolerates a missing session.json by returning a template, but the scan never calls
/// it without one, so the no-session case maps to `None` here.)
pub fn recover_session(session_dir: &Path) -> Option<v1::Session> {
    let session_id = session_id_for(session_dir);
    let dir = session_dir.to_string_lossy().to_string();

    let meta_path = session_dir.join("session.json");
    if !meta_path.is_file() {
        return None;
    }

    // The capability flags are ALWAYS re-derived from disk (never trusted from the summary).
    let caps = session_capabilities(session_dir);

    // Parse session.json → its "summary" block. A missing/unreadable/`summary`-less file maps
    // to the template-only branch of `_recover` (a recovery note, default state "unknown").
    let summary: Option<Value> = std::fs::read_to_string(&meta_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|m| m.get("summary").cloned())
        .filter(|s| s.is_object());

    let Some(summary) = summary else {
        // `_recover`: "recovered from index; session.json missing or unreadable".
        return Some(template_session(session_id, dir, &caps));
    };

    // State precedence: recorded state → "interrupted" if it's a live state (process gone).
    let recorded_state = s_str(&summary, "state").unwrap_or_else(|| "unknown".into());
    let interrupted = LIVE_STATES.contains(&recorded_state.as_str());
    let state = if interrupted {
        "interrupted".to_string()
    } else {
        recorded_state
    };

    // Counts come from the recorded summary, falling back to the on-disk derivation when the
    // summary omits them (a partial/old session.json).
    let screenshots = s_i64(&summary, "screenshots").unwrap_or(caps.screenshot_count);
    let transcript_segments =
        s_i64(&summary, "transcript_segments").unwrap_or(caps.transcript_segments);

    Some(v1::Session {
        session_id,
        state,
        screenshots,
        transcript_segments,
        audio_status: s_str(&summary, "audio_status").unwrap_or_else(|| "unknown".into()),
        window_title: s_str(&summary, "window_title"),
        dir,
        // Capability flags: re-derived from disk (mirrors `_with_caps`), never the frozen ones.
        has_screenshots: caps.has_screenshots,
        has_audio: caps.has_audio,
        has_mic: caps.has_mic,
        mic_device: s_str(&summary, "mic_device"),
        can_retranscribe: caps.can_retranscribe,
        can_index: caps.can_index,
    })
}

/// A full-shaped record for a session whose `session.json` had no usable `summary` block.
/// Mirrors `_recover`'s template branch: state "unknown", counts 0 (in the recorded sense),
/// but capability flags still derived from whatever artifacts are on disk.
fn template_session(session_id: String, dir: String, caps: &Caps) -> v1::Session {
    v1::Session {
        session_id,
        state: "unknown".into(),
        screenshots: caps.screenshot_count,
        transcript_segments: caps.transcript_segments,
        audio_status: "unknown".into(),
        window_title: None,
        dir,
        has_screenshots: caps.has_screenshots,
        has_audio: caps.has_audio,
        has_mic: caps.has_mic,
        mic_device: None,
        can_retranscribe: caps.can_retranscribe,
        can_index: caps.can_index,
    }
}

/// The session id for a dir: its name with the `capture-` prefix stripped (mirrors
/// `_scan_runs_dir`'s `sid = d.name[len(prefix):] if d.name.startswith(prefix) else d.name`).
fn session_id_for(session_dir: &Path) -> String {
    let name = session_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    name.strip_prefix(DIR_PREFIX)
        .map(String::from)
        .unwrap_or(name)
}

/// Enumerate `runs`, recover each `capture-*` session dir holding a `session.json`, and return
/// the recovered [`v1::Session`]s sorted by session id (oldest first — ids are timestamp-prefixed,
/// so lexical order == chronological order, matching `summaries()`).
///
/// Port of `_scan_runs_dir` + `summaries()`: a dir without a `session.json` (or not a `capture-*`
/// folder) is skipped. The append-only `sessions.jsonl` index is folded in too (an indexed
/// session whose dir still has a `session.json` is recovered even if it's outside `runs`).
pub fn list_sessions(runs: &Path) -> Vec<v1::Session> {
    // dir -> recovered session, keyed by session id (later writes win, mirroring the index +
    // scan merge where index entries and on-disk scans converge on one record per id).
    let mut by_id: std::collections::BTreeMap<String, v1::Session> = std::collections::BTreeMap::new();

    // 1) Fold in the append-only index (`sessions.jsonl`): each entry points at a dir that may
    //    live outside `runs`. Mirrors `_load_history` reading the index before scanning.
    for (sid, dir) in read_index_entries() {
        if by_id.contains_key(&sid) {
            continue;
        }
        if let Some(sess) = recover_session(Path::new(&dir)) {
            by_id.insert(sid, sess);
        }
    }

    // 2) Scan the runs dir for `capture-*` folders (covers sessions the index lost).
    if let Ok(entries) = std::fs::read_dir(runs) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(DIR_PREFIX) {
                continue;
            }
            let sid = name[DIR_PREFIX.len()..].to_string();
            if sid.is_empty() || by_id.contains_key(&sid) {
                continue;
            }
            if let Some(sess) = recover_session(&path) {
                by_id.insert(sid, sess);
            }
        }
    }

    // Newest-only retention then oldest-first order (BTreeMap is already sorted by id).
    let total = by_id.len();
    by_id
        .into_iter()
        .skip(total.saturating_sub(MAX_SESSIONS))
        .map(|(_, s)| s)
        .collect()
}

/// Read `(id, dir)` pairs from the append-only `sessions.jsonl` index (later lines win for a
/// re-indexed id). Tolerates torn/corrupt lines. Empty when the index is absent. Mirrors the
/// `_load_history` parse loop.
fn read_index_entries() -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(sessions_index_path()) else {
        return Vec::new();
    };
    let mut entries: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for ln in text.lines() {
        let ln = ln.trim();
        if ln.is_empty() {
            continue;
        }
        let Ok(e) = serde_json::from_str::<Value>(ln) else {
            continue; // tolerate torn/corrupt lines
        };
        if let (Some(id), Some(dir)) = (
            e.get("id").and_then(|v| v.as_str()),
            e.get("dir").and_then(|v| v.as_str()),
        ) {
            entries.insert(id.to_string(), dir.to_string()); // later lines win
        }
    }
    entries.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Serializes the tests that mutate process-global env (`CAPTURE_SESSION_INDEX` /
    /// `CAPTURE_RUNS_DIR`), since cargo runs tests multithreaded — without this a parallel
    /// test could observe another's env mutation and flake.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Unique temp root under the OS temp dir (no tempfile dep).
    fn mk_tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("capture-core-sessions-{tag}-{nanos}"));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Write a session dir with a `session.json` carrying the given summary JSON, plus the
    /// requested artifacts. `screenshots` = how many `screenshots/*.jpg` to write (0 = no dir);
    /// `audio`/`transcript_lines` create `audio.s16le` / `transcript.jsonl`.
    fn write_session(
        runs: &Path,
        id: &str,
        summary: &str,
        screenshots: usize,
        audio: bool,
        transcript_lines: usize,
    ) -> PathBuf {
        let dir = runs.join(format!("capture-{id}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.json"),
            format!(r#"{{"config":{{}},"summary":{summary}}}"#),
        )
        .unwrap();
        if screenshots > 0 {
            let shots = dir.join("screenshots");
            fs::create_dir_all(&shots).unwrap();
            for i in 0..screenshots {
                fs::write(shots.join(format!("2026-06-16T22-01-1{i}.000Z.jpg")), b"x").unwrap();
            }
        }
        if audio {
            fs::write(dir.join("audio.s16le"), b"audiobytes").unwrap();
        }
        if transcript_lines > 0 {
            let mut lines = String::new();
            for i in 0..transcript_lines {
                lines.push_str(&format!(
                    r#"{{"start_offset":{i}.0,"end_offset":{}.0,"text":"seg{i}"}}"#,
                    i + 1
                ));
                lines.push('\n');
            }
            fs::write(dir.join("transcript.jsonl"), lines).unwrap();
        }
        dir
    }

    #[test]
    fn recover_complete_session() {
        let runs = mk_tmp("complete");
        // A clean, stopped session: full summary block + 2 screenshots + audio + 3 transcript lines.
        let dir = write_session(
            &runs,
            "20260616-aaa",
            r#"{"state":"stopped","started_at":"2026-06-16T22:01:13.146Z",
                "stopped_at":"2026-06-16T22:05:00.000Z","window_title":"Chrome",
                "audio_status":"recording","screenshots":2,"transcript_segments":3,
                "mic_device":null}"#,
            2,
            true,
            3,
        );
        let s = recover_session(&dir).expect("a session dir recovers");
        assert_eq!(s.session_id, "20260616-aaa");
        assert_eq!(s.state, "stopped");
        assert_eq!(s.window_title.as_deref(), Some("Chrome"));
        assert_eq!(s.audio_status, "recording");
        assert!(s.has_screenshots, "screenshots/ non-empty");
        assert!(s.has_audio, "audio.s16le present");
        assert_eq!(s.transcript_segments, 3);
        assert_eq!(s.screenshots, 2);
        assert!(s.can_index, "can_index == has_screenshots");
        assert!(s.can_retranscribe, "can_retranscribe == has_audio");
        assert_eq!(s.dir, dir.to_string_lossy());
        fs::remove_dir_all(&runs).ok();
    }

    #[test]
    fn recover_audio_only_session_has_no_screenshots() {
        let runs = mk_tmp("audioonly");
        // No screenshots/ dir at all → has_screenshots/can_index false; audio present.
        let dir = write_session(
            &runs,
            "20260616-bbb",
            r#"{"state":"stopped","audio_status":"recording","transcript_segments":1}"#,
            0,
            true,
            1,
        );
        let s = recover_session(&dir).expect("recovers");
        assert!(!s.has_screenshots, "no screenshots/ dir");
        assert!(!s.can_index, "can_index follows has_screenshots");
        assert!(s.has_audio);
        assert!(s.can_retranscribe);
        assert_eq!(s.transcript_segments, 1);
        fs::remove_dir_all(&runs).ok();
    }

    #[test]
    fn recover_interrupted_session() {
        let runs = mk_tmp("interrupted");
        // Recorded "running" by a process that is now gone → recovered as "interrupted".
        // Rule ported (registry._recover): a recorded state in
        // ("starting","running","stopping") becomes "interrupted" — the capturing process
        // exited while the session was live.
        let dir = write_session(
            &runs,
            "20260616-ccc",
            r#"{"state":"running","audio_status":"recording","screenshots":1}"#,
            1,
            false,
            0,
        );
        let s = recover_session(&dir).expect("recovers");
        assert_eq!(s.state, "interrupted", "live state with no clean stop → interrupted");
        fs::remove_dir_all(&runs).ok();
    }

    #[test]
    fn recover_missing_summary_is_unknown() {
        let runs = mk_tmp("nosummary");
        let dir = runs.join("capture-20260616-ddd");
        fs::create_dir_all(&dir).unwrap();
        // session.json with no "summary" block → template branch: state "unknown".
        fs::write(dir.join("session.json"), r#"{"config":{}}"#).unwrap();
        let s = recover_session(&dir).expect("recovers a template record");
        assert_eq!(s.state, "unknown");
        assert!(!s.has_screenshots);
        fs::remove_dir_all(&runs).ok();
    }

    #[test]
    fn recover_non_session_dir_is_none() {
        let runs = mk_tmp("nonsession");
        let dir = runs.join("capture-20260616-eee");
        fs::create_dir_all(&dir).unwrap();
        // No session.json → not a session dir.
        assert!(recover_session(&dir).is_none());
        fs::remove_dir_all(&runs).ok();
    }

    #[test]
    fn list_sessions_orders_and_skips_non_sessions() {
        let runs = mk_tmp("list");
        // Two sessions; ids are timestamp-prefixed so lexical == chronological order.
        write_session(
            &runs,
            "20260616-zzz",
            r#"{"state":"stopped","audio_status":"recording"}"#,
            1,
            true,
            2,
        );
        write_session(
            &runs,
            "20260615-aaa",
            r#"{"state":"stopped","audio_status":"off"}"#,
            0,
            true,
            0,
        );
        // A non-session dir (no session.json) and a non-capture dir — both skipped.
        fs::create_dir_all(runs.join("capture-20260616-empty")).unwrap();
        fs::create_dir_all(runs.join("not-a-capture")).unwrap();

        // Point the index env at a non-existent file so only the runs scan contributes.
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "CAPTURE_SESSION_INDEX",
            runs.join("nope.jsonl").to_string_lossy().to_string(),
        );
        let sessions = list_sessions(&runs);
        std::env::remove_var("CAPTURE_SESSION_INDEX");
        drop(_guard);

        assert_eq!(sessions.len(), 2, "two valid sessions, others skipped");
        // Oldest first: 20260615 before 20260616.
        assert_eq!(sessions[0].session_id, "20260615-aaa");
        assert_eq!(sessions[1].session_id, "20260616-zzz");
        fs::remove_dir_all(&runs).ok();
    }

    #[test]
    fn list_sessions_folds_in_the_index() {
        let runs = mk_tmp("index");
        // A session whose dir is OUTSIDE the runs dir, reachable only via the index.
        let other = mk_tmp("indexother");
        write_session(&other, "20260616-fff", r#"{"state":"stopped"}"#, 1, true, 1);
        let idx = runs.join("sessions.jsonl");
        fs::write(
            &idx,
            format!(
                r#"{{"id":"20260616-fff","dir":"{}","created_at":"t"}}"#,
                other.join("capture-20260616-fff").to_string_lossy()
            ),
        )
        .unwrap();
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("CAPTURE_SESSION_INDEX", idx.to_string_lossy().to_string());
        let sessions = list_sessions(&runs);
        std::env::remove_var("CAPTURE_SESSION_INDEX");
        drop(_guard);

        assert_eq!(sessions.len(), 1, "the indexed out-of-runs session is recovered");
        assert_eq!(sessions[0].session_id, "20260616-fff");
        fs::remove_dir_all(&runs).ok();
        fs::remove_dir_all(&other).ok();
    }

    #[test]
    fn runs_dir_respects_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("CAPTURE_RUNS_DIR", "/tmp/custom-runs");
        assert_eq!(runs_dir(), PathBuf::from("/tmp/custom-runs"));
        std::env::remove_var("CAPTURE_RUNS_DIR");
    }
}
