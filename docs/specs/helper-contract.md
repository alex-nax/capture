# Spec: Audio helper contract (frozen)

_Status: current as of 2026-06-10. Source of truth = the code; update this spec in the same change as the code._

> **This protocol is FROZEN shared property** (product-architecture.md invariant).
> Every audio source helper — today's Swift `audiocap` (macOS, per-app/system) and
> `audiocap_win.py` (Windows, system loopback), and the planned native Windows
> per-process-loopback helper (feature #34) — must speak exactly this contract.
> The engine consumes helpers ONLY through it; nothing may parse helper internals.

## Purpose

Define the process-boundary protocol between the capture engine
(`core/audio.py` via `platform.current().audio_source`) and any audio-source
helper subprocess. Implementation details per helper live in
[screencapturekit-helper.md](screencapturekit-helper.md) (macOS) and
[windows.md](windows.md)/[platform-abstraction.md](platform-abstraction.md);
this spec is only the contract.

## Files

- `helper/audiocap.swift` — macOS ScreenCaptureKit helper (per-app / `--system`).
- `helper/audiocap_win.py` — Windows WASAPI system-loopback helper.
- Consumers: `core/audio.py` (`_read_loop` on stdout, `_spawn_stderr_logger` on
  stderr), `core/platform/macos.py` / `core/platform/windows.py` (build argv).

## Public contract

### argv

- macOS: `audiocap --pid <PID> | --bundle <bundle.id> | --system | --mic [<deviceID>] [--rate <hz>]`
  (exactly one target; no target → usage on stderr, exit 2). `--mic` captures the microphone via
  AVFoundation `AVCaptureSession` (NOT ScreenCaptureKit) — same s16le + READY contract — and needs only
  the Microphone permission, no Screen Recording. The optional value is a device uniqueID from
  `--list-mics`; absent/`default` = system default input. **No echo cancellation** — a laptop's built-in
  mic captures its own speaker output (use headphones; proper AEC is tracked as feature #38). A
  voice-processing (`setVoiceProcessingEnabled`) attempt was reverted because it ducked/muted other apps'
  audio.
- macOS: `audiocap --list-mics` — prints one JSON object per stdout line `{"id","name","default"}`
  for the available input devices, then exits 0 (used by `GET /v1/audio/mics`). Not a PCM stream.
- Windows: `python helper/audiocap_win.py [--rate <hz>] [--stall-timeout <s>]`
  (system mix only today; `--stall-timeout` is accepted but currently a no-op —
  see Known limitations).
- `--rate` defaults to 16000 on both. The engine always requests 16000.

### stdout — PCM only, nothing else, ever

- Raw signed-16-bit little-endian **mono** PCM at the requested rate.
- No header, no framing, no length prefix; the byte stream is the format.
- Unbuffered/flushed writes (macOS: unbuffered `FileHandle`; Windows: explicit
  `flush()` per write, binary mode via `msvcrt.setmode`).
- EOF on stdout means the helper ended (parent decides why via exit code +
  last stderr line).

### stderr — human-readable status lines

- `READY rate=<n> channels=1 fmt=s16le ...` is emitted **once capture is
  actually flowing-capable** (macOS: after `startCapture` succeeds; Windows:
  after the loopback stream opens). It is **NOT the first line** — diagnostics
  precede it (macOS prints `content: apps=... displays=...` and `target=...`
  first). **Parents/probes must scan stderr lines for the `READY ` prefix**,
  never read line 1. Trailing fields after `fmt=s16le` are informational and
  may vary per helper (`target=...`, `src_rate=...`, `device=...`,
  `(reconnect #N)`) — match on the prefix only.
- All other lines are free-form diagnostics for humans/logs. The engine logs
  them (`[audiocap] ...`) and keeps only the most recent line for failure
  reporting (`core/audio.py` `_last_stderr`).

### Exit codes

- `0` — clean shutdown (signal received, or parent closed stdout/EPIPE).
- `1` — fatal capture failure (macOS: permission error, or reconnect budget
  exhausted with no audio ever received).
- `2` — usage error (no target).
- macOS-specific startup failures: `3` no app matched pid/bundle, `4` no
  display, `5` shareable-content enumeration failed.

### Signals

- `SIGTERM`/`SIGINT` → stop capture, exit 0. (macOS guards reconnects during
  shutdown with `stopping`; exactly one thread performs the exit.)
- macOS ignores `SIGPIPE`: a closed parent stdout surfaces as a failed write →
  clean exit 0.

### Error taxonomy (macOS SCStream codes — load-bearing, do not change)

- **`-3801` (userDeclined) / `-3803` (missingEntitlements)**: genuine
  permission failures. Not retried; report on stderr and exit 1. The fix is a
  Screen Recording grant (see permissions-and-signing.md).
- **`-3805` (connection interrupted)**: transient (Space/display/focus
  changes). The helper **auto-reconnects** with backoff (0.25s × attempts,
  capped at 2s); if audio has *never* flowed, it gives up after 20 attempts
  (exit 1); once audio has flowed, it retries indefinitely and `READY ...
  (reconnect #N)` marks recovery.
- Windows analogue: any stream read error → close + reopen loopback (`read
  error: ... (reconnecting)` / `reconnected loopback ...` on stderr),
  indefinitely; device changes are absorbed by reopening the (new) default
  device.

## Behavior

The engine (`core/audio.py::_read_loop`) reads stdout in 4 KB blocks, anchors
the transcript timeline to the wall clock of the **first** PCM bytes, and
treats helper EOF with zero bytes received as a failure to surface
(`<mode>-audio-failed (rc=...): <last stderr line>`). Reconnect gaps inside a
helper are invisible to the engine except as missing wall-clock time (no
silence is synthesized — a known timeline-drift source, see audio.md).

## Invariants & constraints

- **stdout is PCM-sacred**: a helper that prints anything else to stdout
  corrupts audio; all prose goes to stderr. (Same discipline as the MCP
  server's stdout.)
- **READY is scanned, not positional** — fixed by this spec after the
  `audiocap.swift` header comment falsely claimed "first line is READY"
  (the READY emit follows `content:`/`target=` diagnostics).
- **Mono only.** Multichannel sources are downmixed inside the helper
  (Windows: channel mean; macOS: SCStream is configured `channelCount = 1`).
- **The engine never inspects helper internals** — argv in, PCM/stderr/rc out.
  A replacement helper (e.g. the native Windows per-process one, #34) is a
  drop-in if it honors this file.

## Failure modes & handling

Covered above (exit codes, taxonomy). Additionally: a helper that emits no
PCM and no stderr diagnostics will be reported by the engine as
`(rc=<n>): no output`.

## Outputs / artifacts

None on disk — the helper is a pure pipe. (The engine persists `audio.s16le`,
transcripts, and events; see audio.md / events.md.)

## Configuration

- `--rate` (both helpers; engine passes 16000).
- Windows `--stall-timeout` — reserved, currently unused.
- No environment variables are part of the contract.

## Known limitations / open items

- `audiocap_win.py --stall-timeout` is parsed but not implemented (a no-data
  watchdog for wedged loopback streams); blocking reads normally deliver
  silence frames, so this has not bitten in practice. Implement or drop when
  the native Windows helper (#34) replaces this one.
- Windows helper captures the full system mix, not per-process (the #34
  refinement: Process Loopback API with process-tree mode, speaking exactly
  this contract).
- Reconnect gaps are not silence-filled, so long captures can drift vs wall
  clock (documented in audio.md; offline re-transcription is the workaround).

## Tests

- `tests/contract/run_contracts.py` pins the engine-side consumers
  (transcript record shape, chunk/offset math) that any helper feeds into.
- The helper protocol itself is verified by the documented manual checks:
  `bash scripts/build_helper.sh` then `./helper/audiocap --system` → scan
  stderr for `READY`, expect PCM on stdout (AGENTS.md testing section); on
  Windows, `python helper/audiocap_win.py` likewise. A hermetic stderr-scan
  probe is folded into the packaged-engine work (#31's `capture doctor`).
