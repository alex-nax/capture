//! Transcript loading + time-range slicing — a 1:1 port of `core/indexer.py`'s
//! `_load_transcript` + `_transcript_slice`.
//!
//! The transcript is `transcript.jsonl`: one JSON object per line with `start_offset`,
//! `end_offset`, and `text` (offsets are seconds on the session timeline, the same axis
//! as `frames::Frame::offset`). `transcript_slice` concatenates the text of segments
//! overlapping a half-open `[lo, hi)` window.

use std::path::Path;

#[derive(Clone, Debug)]
pub struct Segment {
    pub start_offset: f64,
    pub end_offset: f64,
    pub text: String,
}

/// Transcript segments with offsets. Reads `<dir>/transcript.jsonl`; keeps records that have
/// both `start_offset` and `text`; `end_offset` defaults to `start_offset` when absent; text is
/// trimmed. Malformed lines are skipped. Empty if there is no file. Port of `_load_transcript`.
pub fn load_transcript(session_dir: &Path) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let Ok(text) = std::fs::read_to_string(session_dir.join("transcript.jsonl")) else {
        return out;
    };
    for ln in text.lines() {
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(ln) else {
            continue;
        };
        // Require start_offset + text (mirrors `"start_offset" in rec and "text" in rec`).
        if rec.get("start_offset").is_none() || rec.get("text").is_none() {
            continue;
        }
        let start_offset = rec.get("start_offset").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let end_offset = rec
            .get("end_offset")
            .and_then(|v| v.as_f64())
            .unwrap_or(start_offset);
        let text = rec
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        out.push(Segment {
            start_offset,
            end_offset,
            text,
        });
    }
    out
}

/// Concatenate (space-joined) the transcript text of segments overlapping `[lo, hi)`:
/// text non-empty AND `end_offset > lo` AND `start_offset < hi`; result trimmed.
/// Port of `_transcript_slice`.
pub fn transcript_slice(segments: &[Segment], lo: f64, hi: f64) -> String {
    let parts: Vec<&str> = segments
        .iter()
        .filter(|s| !s.text.is_empty() && s.end_offset > lo && s.start_offset < hi)
        .map(|s| s.text.as_str())
        .collect();
    parts.join(" ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn seg(start: f64, end: f64, text: &str) -> Segment {
        Segment {
            start_offset: start,
            end_offset: end,
            text: text.to_string(),
        }
    }

    #[test]
    fn slice_overlap_logic() {
        let segs = vec![
            seg(0.0, 2.0, "a"),
            seg(2.0, 4.0, "b"),
            seg(4.0, 6.0, "c"),
            seg(5.0, 7.0, ""), // empty text is excluded even if it overlaps
        ];
        // Window [2,5): "a" ends at 2 (end_offset > lo is 2>2 false → excluded),
        // "b" (2..4) start<5 && end>2 → in, "c" (4..6) start 4<5 && end 6>2 → in.
        assert_eq!(transcript_slice(&segs, 2.0, 5.0), "b c");
        // Window covering everything but empty excluded.
        assert_eq!(transcript_slice(&segs, -1.0, 100.0), "a b c");
        // Disjoint window.
        assert_eq!(transcript_slice(&segs, 50.0, 60.0), "");
    }

    #[test]
    fn load_transcript_reads_and_defaults() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d: PathBuf = std::env::temp_dir().join(format!("capture-core-tx-{nanos}"));
        fs::create_dir_all(&d).unwrap();
        let lines = [
            r#"{"start_offset":0.0,"end_offset":1.5,"text":"  hello  "}"#, // trimmed
            r#"{"start_offset":2.0,"text":"world"}"#,                      // end defaults to start
            r#"not json"#,                                                 // skipped
            r#"{"text":"no offset"}"#,                                     // missing start_offset → skipped
            r#"{"start_offset":3.0}"#,                                     // missing text → skipped
        ];
        fs::write(d.join("transcript.jsonl"), lines.join("\n")).unwrap();

        let segs = load_transcript(&d);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "hello");
        assert_eq!(segs[0].end_offset, 1.5);
        assert_eq!(segs[1].text, "world");
        assert_eq!(segs[1].end_offset, 2.0, "end_offset defaults to start_offset");

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn load_transcript_missing_file_is_empty() {
        let d = std::env::temp_dir().join("capture-core-tx-missing-xyz123");
        assert!(load_transcript(&d).is_empty());
    }
}
