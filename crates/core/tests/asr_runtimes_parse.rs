//! Regression: the GUI deserializes GET /v1/asr/runtimes into `capture_core::v1::AsrRuntimes`.
//! Feed it the EXACT bytes the Windows daemon returns (captured via curl) and assert it yields the
//! 3 runtimes — if this fails, the GUI's `unwrap_or_default()` silently shows an empty engine list.
use capture_core::v1::AsrRuntimes;

const PAYLOAD: &str = include_str!("asr_runtimes_payload.json");

#[test]
fn windows_runtimes_payload_deserializes() {
    let parsed: AsrRuntimes = serde_json::from_str(PAYLOAD)
        .unwrap_or_else(|e| panic!("AsrRuntimes parse failed: {e}\npayload:\n{PAYLOAD}"));
    assert_eq!(parsed.runtimes.len(), 3, "expected 3 runtimes, got {}", parsed.runtimes.len());
    assert!(parsed.runtimes.iter().any(|r| r.id == "whisper-cuda"));
    assert_eq!(parsed.active.as_deref(), Some("whisper-cuda"));
}
