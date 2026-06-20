//! Timestamps, filesystem-safe stamps, and ISO parse/format — a 1:1 port of
//! `core/util.py` (`now`/`iso`/`fs_stamp`) plus the `frames.py` stem helpers
//! (`_parse_fs_stamp`/`_display_iso`).
//!
//! The on-disk timestamp format is `YYYY-MM-DDTHH:MM:SS.mmmZ` (millisecond precision,
//! UTC), and the filesystem-safe variant replaces `:` with `-`. These must stay
//! byte-identical to the Python so screenshot stems written by either side parse on
//! both. chrono is used only for the calendar arithmetic (parse/format).

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDateTime, Utc};

/// Wall clock as a unix epoch float (seconds). Mirrors `util.now()`.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// ISO-8601 UTC timestamp, millisecond precision, e.g. `2026-06-07T09:47:01.250Z`.
///
/// Mirrors `util.iso()`: `%Y-%m-%dT%H:%M:%S.` + 3-digit millis + `Z`. `None` uses `now()`.
/// The millis are truncated from microseconds (`microsecond // 1000`), matching Python.
pub fn iso(ts: Option<f64>) -> String {
    let secs = ts.unwrap_or_else(now);
    // Split into whole seconds + sub-second nanos, mirroring datetime.fromtimestamp.
    let whole = secs.floor();
    let frac = secs - whole;
    // Round the fraction to MICROSECONDS (mirroring Python datetime.fromtimestamp, which rounds
    // the float to micros) BEFORE truncating to millis. At large epochs f64 can't hold the
    // fraction exactly, so rounding at nanosecond scale then truncating micros→millis drops the
    // last millisecond (.146 → .145); rounding to micros first recovers it.
    let micros = ((frac * 1e6).round() as u32).min(999_999);
    let dt: DateTime<Utc> = DateTime::from_timestamp(whole as i64, micros * 1000)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
    // microsecond // 1000 → millisecond (truncation, matching Python's integer floor div).
    let millis = (dt.timestamp_subsec_micros() / 1000).min(999);
    format!("{}.{:03}Z", dt.format("%Y-%m-%dT%H:%M:%S"), millis)
}

/// Filesystem-safe timestamp for filenames, e.g. `2026-06-07T09-47-01.250Z`.
/// Mirrors `util.fs_stamp()`: `iso(ts).replace(':', "-")`.
pub fn fs_stamp(ts: Option<f64>) -> String {
    iso(ts).replace(':', "-")
}

/// Parse a `2026-06-16T22-01-13.146Z` screenshot stem back to a unix timestamp.
/// Mirrors `frames._parse_fs_stamp`; returns `None` on parse failure.
pub fn parse_fs_stamp(stem: &str) -> Option<f64> {
    let dt = NaiveDateTime::parse_from_str(stem, "%Y-%m-%dT%H-%M-%S%.3fZ").ok()?;
    let utc = dt.and_utc();
    // seconds (incl. millis) as f64, matching datetime.timestamp().
    Some(utc.timestamp() as f64 + utc.timestamp_subsec_nanos() as f64 / 1e9)
}

/// `2026-06-16T22-01-13.146Z` -> `2026-06-16T22:01:13.146Z` (only the time part).
/// Mirrors `frames._display_iso`: split on the first `T`, replace `-`→`:` in the time half.
pub fn display_iso(stem: &str) -> String {
    match stem.split_once('T') {
        Some((date, time)) => format!("{date}T{}", time.replace('-', ":")),
        None => stem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_epoch_zero() {
        assert_eq!(iso(Some(0.0)), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn iso_millis_precision() {
        // 0.250s past a whole second → .250 millis.
        assert_eq!(iso(Some(1.25)), "1970-01-01T00:00:01.250Z");
    }

    #[test]
    fn parse_fs_stamp_round_trips() {
        let stem = "2026-06-16T22-01-13.146Z";
        let ts = parse_fs_stamp(stem).expect("should parse");
        // Round-trips through fs_stamp back to the same stem.
        assert_eq!(fs_stamp(Some(ts)), stem);
    }

    #[test]
    fn parse_fs_stamp_garbage_is_none() {
        assert!(parse_fs_stamp("garbage").is_none());
    }

    #[test]
    fn display_iso_swaps_only_time_half() {
        assert_eq!(
            display_iso("2026-06-16T22-01-13.146Z"),
            "2026-06-16T22:01:13.146Z"
        );
        // No 'T' → returned unchanged.
        assert_eq!(display_iso("nope"), "nope");
    }
}
