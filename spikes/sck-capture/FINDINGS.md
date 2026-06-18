# SPIKE A (#59) — ScreenCaptureKit from Rust: findings

**Verdict: YES — viable via the `screencapturekit` crate (v7.0.1), with one one-time packaging caveat.**
All three target capabilities compiled and ran on the first real attempt. Zero unsafe, no objc2 fallback needed.

## Crate choice
Higher-level **`screencapturekit = 7.0.1`** (doom-fish), the safe binding — NOT the raw
`objc2-screen-capture-kit` route (never needed it). Features `macos_15_0` (cumulative: 13_0 audio + 14_0
screenshots). PNG via the `png` crate. Built on thin `apple-cf`/`apple-metal` crates that ship Swift bridges.

API is a near 1:1 mirror of the Swift helper: `SCShareableContent::get()`, builder `SCContentFilter`
(`with_including_applications(&[app], &[])` == the helper's per-app filter), `SCStreamConfiguration`
(`with_captures_audio/sample_rate/channel_count/excludes_current_process_audio`), `SCScreenshotManager`,
trait/closure `SCStreamOutputTrait`, and `CMSampleBuffer::audio_buffer_list()`. All accessors present
(process_id, bundle_identifier, application_name, window_id, title, owning_application).

## Build outcome
Builds cleanly EXCEPT a transitive dep — **`apple-metal` 0.8.8** — whose Swift bridge references macOS 26
SDK symbols (`MTLSamplerReductionMode`, `descriptor.reductionMode/lodBias`) and fails on this macOS 15.7 SDK.
It is non-optional / not feature-gated (range `>=0.6,<0.9`). **Fix: pin `apple-metal = "=0.6.0"`** → builds in ~6s.
The spike main.rs compiled with no changes against the API as designed.

## Runtime outcome (TCC)
Screen Recording was ALREADY granted to the parent terminal, so the bare `cargo run` binary inherited the
grant — NO prompt, NO -3801/-3803/-3805 denial (a denied path was therefore not exercised). One wrinkle: the
binary links the crates' Swift bridges and dyld couldn't find `@rpath/libswift_Concurrency.dylib` (no
Swift-runtime rpath embedded). Worked around with `DYLD_LIBRARY_PATH=…/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx`.
A shipped product embeds the rpath at link time (standard Swift-interop).

| Capability | Result |
|---|---|
| 1. list shareable content | PASS — 2 displays, 309 windows, 31 apps; id/owner/title printed |
| 2. screenshot | PASS — real screen content → /tmp/sck_spike.png, valid PNG 1512x982 RGBA |
| 3. per-app audio | PASS (pipeline) — SCStream filtered to one app, delegate fired, audio_buffer_list → Float32→i16 → 47360 s16le samples in 3s ≈ 16 kHz mono → /tmp/sck_spike.s16le. All-zero only because the target app (Finder) was silent; buffers flowed + converted correctly. |

## Ergonomics / coverage
- Async delegate model is easy: `SCStreamOutputTrait: Send + Sync`, one `did_output_sample_buffer`; shared
  Mutex<File> + AtomicU64 into it, no lifetime fights.
- Sample-buffer extraction clean: `sample.audio_buffer_list()` → `list.get(0).data()` = raw Float32. SCK
  delivers our requested 16 kHz directly, so a trivial Float32→i16 cast replaces the helper's AVAudioConverter.
- Coverage gap: only the helper's NON-SCK paths are missing — `--mic` (AVCaptureSession) and
  `--extract-audio/-frames` (AVAssetReader / AVAssetImageGenerator) are AVFoundation, not in this crate.
  Would need objc2-avfoundation or a tiny shim. (Crate does expose `with_captures_microphone`, untested.)

## Effort estimate
- SCK audio + screenshots + listing: ~1–2 days (spike covers the hard parts; remaining = reconnect/backoff
  for -3805, SIGPIPE/stdout streaming, pid/bundle CLI).
- Packaging (one-time, shared): ~0.5–1 day — pin apple-metal=0.6.0 + embed Swift-runtime rpath.
- AVFoundation mic + file import: +1–2 days for full Rust parity, OR keep a tiny AVFoundation-only shim.

**Recommendation:** ScreenCaptureKit-from-Rust is turnkey enough via the `screencapturekit` crate to build the
v3 platform crate without Swift for the core capture path. Keep a thin native shim only for AVFoundation file
import / mic. Risk LOW, gated on the two one-time packaging fixes.
