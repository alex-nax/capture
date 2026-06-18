//! Screenshot enumeration + leaf selection for the multimodal index — a 1:1 port of
//! `core/frames.py` (and the `_recover_epoch` it borrows from `core/retranscribe.py`).
//!
//! Lists a session's `screenshots/<fs_stamp>.png` in time order, maps each to its offset
//! on the session timeline (so frames line up with the transcript), and picks the **leaf
//! frames** to caption by a tunable sampling rate (decimation) with a hard cap.
//!
//! `select_leaves` is LOAD-BEARING: the index eval relies on its exact determinism, so the
//! rounding here matches Python's `round()` (banker's / round-half-to-even) rather than
//! Rust's default round-half-away-from-zero. See `round_half_even`.

use std::path::{Path, PathBuf};

use crate::time::{display_iso, parse_fs_stamp};

/// Screenshot extensions the indexer accepts (capture format is png/jpg-configurable).
/// Copy of `frames._IMAGE_EXTS`.
const IMAGE_EXTS: [&str; 3] = ["png", "jpg", "jpeg"];

#[derive(Clone, Debug)]
pub struct Frame {
    pub path: PathBuf,
    /// unix epoch seconds (parsed from the filename).
    pub stamp: f64,
    /// seconds since the session epoch (aligns with transcript offsets).
    pub offset: f64,
    /// ISO-8601 stamp for display.
    pub iso: String,
}

/// Round half-to-even (banker's rounding) to a whole number — matches Python's `round(x)`.
///
/// Rust's `f64::round()` rounds half AWAY from zero (`round(0.5) == 1`, `round(2.5) == 3`),
/// but Python rounds half to the nearest EVEN integer (`round(0.5) == 0`, `round(2.5) == 2`).
/// `select_leaves` feeds `round()` exact `.5` values (e.g. `i*(n-1)/(max-1)`), so this
/// difference is observable and the eval determinism depends on matching Python.
fn round_half_even(x: f64) -> f64 {
    let r = x.round(); // half away from zero
    if (x - x.trunc()).abs() == 0.5 {
        // Exactly halfway: pick the even neighbor instead of the away-from-zero one.
        let floor = x.floor();
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        r
    }
}

/// Round to `ndigits` decimal places using banker's rounding — matches Python's `round(x, n)`.
fn round_dp(x: f64, ndigits: i32) -> f64 {
    let scale = 10f64.powi(ndigits);
    round_half_even(x * scale) / scale
}

/// The audio first-byte wall-clock, so frames align with the transcript:
/// from the existing transcript's first record (`start - start_offset`), else the
/// session's `started_at`, else 0. Port of `retranscribe._recover_epoch`.
pub fn recover_epoch(session_dir: &Path) -> f64 {
    // 1) transcript.jsonl first record: start (ISO) - start_offset.
    if let Ok(text) = std::fs::read_to_string(session_dir.join("transcript.jsonl")) {
        for ln in text.lines() {
            let Ok(rec) = serde_json::from_str::<serde_json::Value>(ln) else {
                continue;
            };
            let st = rec.get("start").and_then(|v| v.as_str()).and_then(parse_iso);
            let off = rec.get("start_offset").and_then(|v| v.as_f64());
            if let (Some(st), Some(off)) = (st, off) {
                return st - off;
            }
        }
    }
    // 2) session.json summary.started_at.
    if let Ok(text) = std::fs::read_to_string(session_dir.join("session.json")) {
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(st) = meta
                .get("summary")
                .and_then(|s| s.get("started_at"))
                .and_then(|v| v.as_str())
                .and_then(parse_iso)
            {
                return st;
            }
        }
    }
    // 3) fallback.
    0.0
}

/// Parse an ISO-8601 timestamp (`...Z` form written by `iso()`) to a unix timestamp.
/// Mirrors `retranscribe._parse_iso` (`fromisoformat(s.replace("Z", "+00:00"))`); None on failure.
fn parse_iso(s: &str) -> Option<f64> {
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9)
}

/// All screenshots in `session_dir`, oldest first, with timeline offsets. Frames whose
/// name doesn't parse are skipped. Empty if there are no screenshots. Port of `list_frames`.
pub fn list_frames(session_dir: &Path) -> Vec<Frame> {
    let shots = session_dir.join("screenshots");
    if !shots.is_dir() {
        return Vec::new();
    }
    let epoch = recover_epoch(session_dir);
    let mut out: Vec<Frame> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&shots) else {
        return out;
    };
    for entry in entries.flatten() {
        let f = entry.path();
        if !f.is_file() {
            continue;
        }
        // Extension must be in the image set (case-insensitive, like suffix.lower()).
        let ext_ok = f
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        // The stem is the fs_stamp regardless of extension.
        let Some(stem) = f.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(ts) = parse_fs_stamp(stem) else {
            continue;
        };
        out.push(Frame {
            path: f.clone(),
            stamp: ts,
            offset: round_dp(ts - epoch, 3),
            iso: display_iso(stem),
        });
    }
    // fs_stamp already sorts chronologically; be explicit (sort ascending by stamp).
    out.sort_by(|a, b| a.stamp.partial_cmp(&b.stamp).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Pick the leaf frames to caption: keep every `round(1/rate)`-th frame (rate in `(0,1]`;
/// 1.0 = all, 0.5 = every other), then uniformly decimate to `max_leaves` if still over.
/// Always keeps at least the first frame (and the last, when >1 survives). Port of
/// `select_leaves` — EXACT determinism (banker's rounding) is required by the eval.
pub fn select_leaves(frames: &[Frame], sample_rate: f64, max_leaves: usize) -> Vec<Frame> {
    if frames.is_empty() {
        return Vec::new();
    }
    let rate = 1.0_f64.min(1e-3_f64.max(sample_rate));
    let step = (round_half_even(1.0 / rate) as usize).max(1);

    // frames[::step] — the kept indices into `frames`.
    let mut kept_idx: Vec<usize> = (0..frames.len()).step_by(step).collect();

    // Always anchor the end of the timeline: if the last kept isn't the last frame and
    // there's >1 frame, append the last frame. (Python: kept[-1] is not frames[-1].)
    let last = frames.len() - 1;
    if let Some(&last_kept) = kept_idx.last() {
        if last_kept != last && frames.len() > 1 {
            kept_idx.push(last);
        }
    }

    if max_leaves > 0 && kept_idx.len() > max_leaves {
        // Uniformly sample max_leaves indices across the kept list (endpoints included).
        let n = kept_idx.len();
        let mut picks: Vec<usize> = (0..max_leaves)
            .map(|i| round_half_even(i as f64 * (n - 1) as f64 / (max_leaves - 1) as f64) as usize)
            .collect();
        picks.sort_unstable();
        picks.dedup();
        kept_idx = picks.into_iter().map(|i| kept_idx[i]).collect();
    }

    kept_idx.into_iter().map(|i| frames[i].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Build N synthetic frames whose offset == index, so kept-index == kept-offset.
    fn synth(n: usize) -> Vec<Frame> {
        (0..n)
            .map(|i| Frame {
                path: PathBuf::from(format!("{i}.png")),
                stamp: i as f64,
                offset: i as f64,
                iso: String::new(),
            })
            .collect()
    }

    fn offsets(frames: &[Frame]) -> Vec<usize> {
        frames.iter().map(|f| f.offset as usize).collect()
    }

    #[test]
    fn round_half_even_matches_python() {
        // Python: round(0.5)==0, round(1.5)==2, round(2.5)==2, round(3.5)==4.
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(2.5), 2.0);
        assert_eq!(round_half_even(3.5), 4.0);
        assert_eq!(round_half_even(2.4), 2.0);
        assert_eq!(round_half_even(2.6), 3.0);
    }

    #[test]
    fn select_leaves_rate_one_keeps_all() {
        let f = synth(5);
        let kept = select_leaves(&f, 1.0, 0);
        assert_eq!(offsets(&kept), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn select_leaves_rate_half_every_other_plus_last_anchor() {
        // rate 0.5 → step 2 → indices 0,2,4 (last==4 is already the final frame, no extra).
        let f = synth(5);
        let kept = select_leaves(&f, 0.5, 0);
        assert_eq!(offsets(&kept), vec![0, 2, 4]);

        // 6 frames: step 2 → 0,2,4; last kept (4) != last frame (5) → append 5.
        let f6 = synth(6);
        let kept6 = select_leaves(&f6, 0.5, 0);
        assert_eq!(offsets(&kept6), vec![0, 2, 4, 5]);
    }

    #[test]
    fn select_leaves_max_forces_uniform_resample() {
        // 10 frames, rate 1.0 → kept = 0..=9 (n=10). max_leaves=4.
        // idx = sorted(unique{ round(i*9/3) for i in 0..4 }) using banker's rounding:
        //   i=0 -> 0.0 -> 0
        //   i=1 -> 3.0 -> 3
        //   i=2 -> 6.0 -> 6
        //   i=3 -> 9.0 -> 9
        // → [0,3,6,9].
        let f = synth(10);
        let kept = select_leaves(&f, 1.0, 4);
        assert_eq!(offsets(&kept), vec![0, 3, 6, 9]);

        // A case exercising banker's rounding on a .5 tie:
        // 6 frames kept (n=6), max_leaves=5 → i*5/4: 0,1.25,2.5,3.75,5.
        //   i=2 -> 2.5 -> round-half-even -> 2 (NOT 3 as away-from-zero would give).
        // → sorted-unique [0,1,2,4,5].
        let f6 = synth(6);
        let kept6 = select_leaves(&f6, 1.0, 5);
        assert_eq!(offsets(&kept6), vec![0, 1, 2, 4, 5]);
    }

    #[test]
    fn select_leaves_empty_is_empty() {
        let kept = select_leaves(&[], 0.5, 0);
        assert!(kept.is_empty());
    }

    /// Unique temp dir under the OS temp root (no tempfile dep).
    fn mk_tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("capture-core-{tag}-{nanos}"));
        fs::create_dir_all(d.join("screenshots")).unwrap();
        d
    }

    #[test]
    fn list_frames_reads_orders_and_offsets() {
        let d = mk_tmp("frames");
        // started_at anchors the epoch; offsets are stamp - epoch.
        fs::write(
            d.join("session.json"),
            r#"{"summary":{"started_at":"2026-06-16T22:01:13.146Z"}}"#,
        )
        .unwrap();
        // Two screenshots, written out of order to prove sorting.
        fs::write(d.join("screenshots/2026-06-16T22-01-15.146Z.png"), b"x").unwrap();
        fs::write(d.join("screenshots/2026-06-16T22-01-13.146Z.jpg"), b"x").unwrap();
        // A non-image and an unparseable stem — both skipped.
        fs::write(d.join("screenshots/notes.txt"), b"x").unwrap();
        fs::write(d.join("screenshots/garbage.png"), b"x").unwrap();

        let frames = list_frames(&d);
        assert_eq!(frames.len(), 2, "two valid screenshots");
        // Sorted ascending: the 13s frame first, then 15s.
        assert!(frames[0].stamp < frames[1].stamp);
        // epoch == started_at (22:01:13.146) → offsets 0.0 then 2.0.
        assert!((frames[0].offset - 0.0).abs() < 1e-6, "got {}", frames[0].offset);
        assert!((frames[1].offset - 2.0).abs() < 1e-6, "got {}", frames[1].offset);
        assert_eq!(frames[0].iso, "2026-06-16T22:01:13.146Z");

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn list_frames_no_screenshots_dir_is_empty() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("capture-core-noshots-{nanos}"));
        fs::create_dir_all(&d).unwrap();
        assert!(list_frames(&d).is_empty());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn recover_epoch_prefers_transcript_first_record() {
        let d = mk_tmp("epoch");
        // transcript first record: start=...01:13.146Z, start_offset=2.0 → epoch = ts(start)-2.
        fs::write(
            d.join("transcript.jsonl"),
            "{\"start\":\"2026-06-16T22:01:13.146Z\",\"start_offset\":2.0,\"text\":\"hi\"}\n",
        )
        .unwrap();
        let epoch = recover_epoch(&d);
        let start_ts = super::parse_iso("2026-06-16T22:01:13.146Z").unwrap();
        assert!((epoch - (start_ts - 2.0)).abs() < 1e-6, "got {epoch}");
        fs::remove_dir_all(&d).ok();
    }
}
