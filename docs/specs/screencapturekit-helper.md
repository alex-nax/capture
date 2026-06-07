# Spec: ScreenCaptureKit Helper (audiocap)
_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

## Purpose
`audiocap` is a standalone macOS command-line binary that captures audio from a single
application (by PID or bundle id) or from the whole display ("system" audio) using
ScreenCaptureKit, converts it to raw 16 kHz mono signed-16-bit little-endian PCM, and
streams those bytes on stdout so a parent process can pipe them into an ASR backend.
Human-readable status goes to stderr. It is a **process boundary**, not a library
(see `docs/architecture.md`): the only stable contract is the stdout PCM byte stream plus
the stderr status lines.

## Files
- `helper/audiocap.swift` — the entire helper (CLI parsing, `AudioSink` stream output/delegate, reconnect logic, signal handling, run loop). Single-file program.

## Public contract

### Invocation (CLI)
Build (from the file header comment):
```
swiftc -O -o audiocap audiocap.swift -framework ScreenCaptureKit -framework AVFoundation -framework CoreMedia
```

Usage forms (one target selector is required):
```
audiocap --pid <PID> [--rate 16000]
audiocap --bundle <bundle.id> [--rate 16000]
audiocap --system [--rate 16000]
```

Flags (parsed by `argValue(_:)` / `CommandLine.arguments`, lines 26-35):
- `--pid <Int32>` — target a running application by process id. Parsed via `Int32($0)`; a non-numeric value yields `nil` (treated as "not provided").
- `--bundle <String>` — target a running application by `bundleIdentifier`.
- `--system` — boolean presence flag; capture whole-display audio.
- `--rate <Double>` — desired output sample rate in Hz. Default `16000`. Parsed as `Double`; unparseable falls back to `16000` (line 34). Note: passed to `SCStreamConfiguration.sampleRate` as `Int(targetRate)` and reported in the READY line as `Int(targetRate)`.

If none of `--pid`, `--bundle`, `--system` is provided, the helper prints a usage line to stderr and exits `2` (lines 37-40).

### stdout contract
Raw PCM only: signed 16-bit little-endian, mono (1 channel), interleaved, at `<rate>` Hz
(default 16000). Output format constructed in `AudioSink.init` (lines 54-62) as
`AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: outRate, channels: 1, interleaved: true)`.
Bytes are written in `writeInt16` (lines 125-139) from `int16ChannelData[0]`. No headers, no
framing — a continuous byte stream. (Endianness is the native host layout, which is
little-endian on all supported Apple hardware; this matches the s16le claim end-to-end per
`docs/architecture.md`.)

### stderr contract
Human-readable status lines via `logErr` (lines 42-44), each newline-terminated. Notable lines:
- `content: apps=<n> displays=<n> windows=<n>` — after enumerating shareable content (line 240).
- `target=<label>; starting capture...` (line 289), where `<label>` is `system` or `"<appName> pid=<pid>"`.
- `READY rate=<n> channels=1 fmt=s16le target=<label>` — emitted on successful `startCapture()` (line 227). On a reconnect it has the suffix ` (reconnect #<n>)`. This is the line parents key off of to know capture started. Note it is **not guaranteed to be the literal first stderr line** — `content:` and `target=...` lines precede it. The file header comment claims "first line is READY", but the current code emits diagnostic lines before it; this is a discrepancy worth noting.
- `audio flowing` — emitted exactly once, the first time PCM bytes are produced (line 129).
- Various error/diagnostic lines (see Failure modes).

### Exit codes
- `2` — no target selector provided (usage error).
- `3` — no running application matched the given pid/bundle (line 251-252).
- `4` — no display available (line 241-243).
- `5` — startup failed (exception enumerating shareable content / wiring, line 291-293).
- `1` — permission error (`-3801`/`-3803`) via `shutdown(1)` (line 149-150); or gave up after >20 failed reconnects with no audio (line 210-213); or `startup` content-task threw.
- `0` — clean shutdown via SIGTERM/SIGINT, or stdout closed (broken pipe) during a write.

## Behavior
1. Parse CLI args (lines 26-40). Validate that at least one of `--pid`/`--bundle`/`--system` is present; otherwise print usage and `exit(2)`.
2. Ignore `SIGPIPE` (`signal(SIGPIPE, SIG_IGN)`, line 171) so a closed stdout surfaces as a throwing write instead of killing the process with a signal.
3. Create the audio dispatch queue (`audiocap.audio`), the global `signalSources` array, and the global `captureStream` holder (lines 173-175).
4. Construct the shutdown guard (`shutdownLock`, `didShutdown`, `shutdown(_:)`, lines 179-190) and the `AudioSink` with the requested output rate (line 192).
5. In an async `Task` (lines 237-295): fetch `SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)`; log the content counts; take the first display (else log + `exit(4)`).
6. Build the `SCContentFilter` and `capLabel` (lines 246-257):
   - `--system`: `SCContentFilter(display:excludingWindows: [])`, label `"system"`.
   - app target: resolve the app via `findApp` (PID match first, then bundle id); if none, log + `exit(3)`. Build `SCContentFilter(display:including: [app], exceptingWindows: [])`, label `"<appName> pid=<pid>"`.
7. Build the shared `SCStreamConfiguration` (lines 259-270): `capturesAudio = true`, `sampleRate = Int(targetRate)`, `channelCount = 1`, `excludesCurrentProcessAudio = true`, plus a minimal-but-valid video config (`width = 128`, `height = 128`, `minimumFrameInterval = 1/1`, `queueDepth = 6`). The video config is required even for audio-only capture; very small sizes are rejected on recent macOS.
8. Install SIGTERM/SIGINT handlers (lines 272-287): for each signal, `signal(s, SIG_IGN)` then a `DispatchSource.makeSignalSource` on `.main` whose handler sets `stopping = true`, calls `stopCapture()`, then `shutdown(0)`. Sources are retained in `signalSources` for the process lifetime.
9. Log `target=...; starting capture...` and call `connect()` (lines 289-290).
10. `connect()` (lines 218-235): if `stopping`, return. Guard the filter/config. In a `Task`, build `SCStream(filter:configuration:delegate: sink)`, assign it to the global `captureStream` (so ARC cannot release it mid-capture), `addStreamOutput(sink, type: .audio, sampleHandlerQueue: audioQueue)`, `await startCapture()`, then log the READY line (with reconnect suffix when applicable). On throw, log `startCapture failed: ...; retrying` and call `scheduleReconnect()`.
11. Per audio sample buffer, `AudioSink.stream(_:didOutputSampleBuffer:of:)` (lines 64-102): ignore non-audio or invalid buffers; copy the CMSampleBuffer into an `AVAudioPCMBuffer` via `makeInputBuffer` (lines 105-123, using `CMSampleBufferCopyPCMDataIntoAudioBufferList`); lazily create the `AVAudioConverter` from the real input format to the Int16 output format (logging a resample note if input rate != output rate); convert one buffer's worth of frames (single `.haveData` then `.noDataNow`); on convert error, log and return; else call `writeInt16`.
12. `writeInt16` (lines 125-139): from `int16ChannelData[0]`, build `Data` of `frameLength * 2` bytes; on the very first data, set `everGotData` and log `audio flowing`; reset `reconnects = 0` (a healthy stream resets backoff); `try stdout.write(contentsOf:)`. On a throw (broken pipe), log `stdout closed (...); exiting` and `shutdown(0)`.
13. `RunLoop.main.run()` (line 297) keeps the process alive for the async tasks, signal sources, and reconnect timers.

## Invariants & constraints
- **stdout is sacred** — only raw PCM bytes are ever written to stdout. All status/diagnostics go to stderr (`logErr`). This mirrors the `docs/architecture.md` hard constraint "stdout is sacred" applied to the helper's PCM stream; corrupting stdout breaks the downstream ASR pipe.
- **Stable stderr contract** — `docs/architecture.md` requires keeping `READY rate=<n> channels=1 fmt=s16le ...` then bytes. The READY token format must not change without coordinating with the parent (`proc.py`/`audio.py`).
- **Audio is always 16 kHz mono s16le end to end** (`docs/architecture.md` naming/conventions). The helper requests 16 kHz mono from SCStream and emits Int16 mono; `--rate` exists but the rest of the system assumes 16000.
- **The capture stream is globally retained** — `captureStream` is a top-level `var` (line 175, assigned line 224) so ARC never releases the `SCStream` mid-capture. Same for `signalSources` (retained for process lifetime) and the `converter` (retained across callbacks so resampler tail carries over).
- **Single shutdown** — all exit paths funnel through `shutdown(_:)` (lines 181-190). `shutdownLock` + `didShutdown` ensure exactly one thread calls `exit`; any losing thread parks in an infinite `Thread.sleep` loop. Termination can be requested concurrently (EPIPE on the audio queue, a signal handler, the stream-error delegate), so this guard is required.
- **`stopping` gates reconnects** — set to `true` before `stopCapture()` in the signal handler so an in-flight stream error during shutdown does not trigger a reconnect (`scheduleReconnect`/`connect` both early-return when `stopping`).
- **Converter is built lazily from the real input format**, not assumed — created on the first sample buffer from `pcm.format` to the Int16 output format (lines 73-79).
- **macOS-only**; requires the Screen Recording permission for the launching process (file header, lines 16-17).

## Failure modes & handling
- **Broken pipe / parent gone (EPIPE):** SIGPIPE is ignored; the `stdout.write` in `writeInt16` throws; the helper logs `stdout closed (...); exiting` and `shutdown(0)` (lines 133-138). Clean exit.
- **Stream interrupted, `-3805` (connection interrupted, e.g. Space/display/focus change):** `stream(_:didStopWithError:)` (lines 141-154) logs the stop, and if not `stopping` and not a permission code, calls `scheduleReconnect()`. **This is retried.**
- **Permission errors `-3801` (userDeclined) / `-3803` (missingEntitlements):** logged with `permission error — grant Screen Recording ...; not retrying` and `shutdown(1)` (lines 147-150). **These are NOT retried.**
- **Reconnect backoff:** `scheduleReconnect()` (lines 205-216) increments `reconnects`, then schedules `connect()` on main after `delay = min(2.0, 0.25 * reconnects)` (linear ramp capped at 2s). A healthy write resets `reconnects = 0` (line 130).
- **Give-up guard:** if `!everGotData && reconnects > 20`, log `giving up after <n> failed connection attempts with no audio` and `shutdown(1)` (lines 210-213). The header comment estimates ~30s of attempts with backoff. Once any audio has flowed (`everGotData == true`), reconnects are effectively unbounded.
- **`startCapture()` throws:** logged as `startCapture failed: ...; retrying` then `scheduleReconnect()` (lines 229-233). Note: a `-3801`/`-3803` raised at `startCapture` time (rather than via the delegate) would still be retried here, since the code-specific check lives only in the delegate — possible inconsistency worth noting.
- **PCM copy failure:** `CMSampleBufferCopyPCMDataIntoAudioBufferList` non-`noErr` → logged, buffer dropped (lines 118-121).
- **Convert error:** logged as `convert error: ...`, buffer dropped (lines 97-100).
- **No display:** log + `exit(4)`. **No matching app:** log + `exit(3)`. **Startup exception:** log + `exit(5)`.
- **Resample note:** if the input sample rate differs from the requested output rate, logs a one-time `note: input rate ... != ...; resampling` (lines 75-77). Since 16 kHz is requested from SCStream, this is usually a pure Float32->Int16 conversion with no resampling.

## Outputs / artifacts
The helper writes **no files**. Its only outputs are:
- **stdout:** raw s16le mono PCM at the configured rate (continuous, unframed byte stream).
- **stderr:** newline-terminated human-readable status/diagnostic lines (see Public contract).

File persistence (session.json, transcripts, recordings) is the parent process's responsibility, not the helper's.

## Configuration
- **No environment variables** are read by the helper.
- **CLI parameters** (see Public contract): `--pid` (no default; mutually selective), `--bundle` (no default), `--system` (boolean, default off), `--rate` (default `16000.0`).
- **Hardcoded stream config:** `channelCount = 1`, `excludesCurrentProcessAudio = true`, `width = 128`, `height = 128`, `minimumFrameInterval = 1/1`, `queueDepth = 6`.
- **Hardcoded reconnect tuning:** backoff `0.25 * reconnects` capped at `2.0s`; give-up threshold `reconnects > 20` while `!everGotData`.

## Known limitations / open items
- **Resampler tail loss:** the converter is retained across callbacks, so only the final buffer's tail at stream end can be lost (described as negligible, lines 67-72).
- **READY is not literally the first stderr line:** the header comment says "first line is READY" but `content:` and `target=...` lines are emitted before it. Parents should scan for the READY token rather than read line 1. (Documentation/code discrepancy.)
- **Output endianness** is native host order (little-endian on supported Apple hardware) rather than explicitly byte-swapped; correct in practice but implicit.
- **`startCapture()`-time permission errors** are not distinguished from transient errors (they get retried), unlike permission errors surfaced via the delegate. Possible inconsistency.
- **`--rate` is effectively cosmetic for the wider system:** the rest of the pipeline assumes 16 kHz, and `sampleRate`/READY use `Int(targetRate)`, so non-integer rates are truncated.
- **Selector precedence:** if multiple of `--pid`/`--bundle`/`--system` are passed, `--system` wins the filter branch only when set (checked first), and within the app branch PID is matched before bundle. Behavior with conflicting flags is implicit, not validated.
- No automated unit tests exist inside the Swift file itself (it is a single executable).

## Tests
- **Smoke test:** `tests/smoke.py` is the project-level smoke harness referenced by the architecture docs; the helper should be exercised end-to-end through it (launch the binary against a known audio-producing target, confirm the READY line and `audio flowing` appear on stderr and that PCM bytes arrive on stdout). Verify exact path/contents of `tests/smoke.py` before relying on specifics — its current coverage of the helper was not confirmed while writing this spec.
- **Manual verification suggestions:**
  - Run `audiocap --system` (or `--pid <pid>` of a playing app) and confirm: `READY rate=16000 channels=1 fmt=s16le target=...` then `audio flowing`, and that `audiocap ... | wc -c` accumulates bytes.
  - Pipe into `ffplay -f s16le -ar 16000 -ch_layout mono -` (or `aplay`/`sox`) to confirm intelligible audio.
  - Trigger a reconnect by switching Spaces/displays while capturing in the background; confirm a `stream stopped ... -3805` line followed by `READY ... (reconnect #n)`.
  - Confirm permission handling: with Screen Recording denied, expect `permission error ... not retrying` and exit `1`.
  - Confirm pipe handling: close the reader (e.g. `head -c <n>`); expect `stdout closed ...; exiting` and exit `0`.
  - Confirm signal handling: send SIGTERM/SIGINT; expect clean `stopCapture` and exit `0`.
