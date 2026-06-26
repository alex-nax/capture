# Spec: Audio

_Status: current as of 2026-06-25 (v3 mic capture moved to AVFoundation, #88). Source of truth = the code; update this spec in the same change as the code._

## Purpose
Capture an audio stream for a session, slice it into fixed-length windows, run each window through an ASR backend, and write timestamped transcripts plus the raw PCM. The scope owns: source selection (app audio vs. microphone — **both via the bundled `audiocap` helper, no ffmpeg** on macOS), the 16 kHz mono s16le byte contract, chunking and offset accounting, anchoring timestamps to first-byte wall-clock arrival, and keeping audio/ASR failures visible in the session status surface. It deliberately knows nothing about other capture components or the MCP layer (see `docs/architecture.md` dependency rules).

One `AudioCapture` instance = one source/track. A session runs the app-audio instance (`track="audio"` → `audio.s16le`/`transcript.*`), and **optionally a second instance for a microphone** (`track="mic"`, `source="mic"`, `mic_device=<id>` → `mic.s16le`/`mic_transcript.*`) — the mic is a SEPARATE track, never mixed with the app audio (`session.py` owns starting both; see `gui.md` for the per-app mic assignment).

## Files
- `src/capture_mcp/core/audio.py` — the entire scope: `AudioCapture` class, reader/transcribe loop, lifecycle (`start`/`stop`), and teardown. The **source command** (which subprocess emits the PCM) is built per-OS by the platform abstraction, not here.

Collaborators referenced but out of scope: `src/capture_mcp/core/platform/` (the `AudioSource.command(...)` that selects the helper/ffmpeg; see [platform-abstraction.md](platform-abstraction.md) — `helper_path()` now lives in `platform/macos.py`), `src/capture_mcp/core/asr/` (the `ASRBackend`/`Segment` interface and `create()` factory), `src/capture_mcp/core/util.py` (`now`, `iso`), `helper/audiocap` (compiled Swift helper, a process boundary).

## Public contract
Module constants (`audio.py:35-37`):
- `SAMPLE_RATE = 16000`
- `BYTES_PER_SAMPLE = 2`
- `MIN_TAIL_BYTES = BYTES_PER_SAMPLE * SAMPLE_RATE // 10` (3200 bytes ≈ 0.1 s) — minimum tail size that still gets transcribed on final flush.

`helper_path()` (the ScreenCaptureKit helper discovery) has moved to `platform/macos.py` as the macOS `AudioSource`'s concern; `audio.py` no longer references it.

`AudioCapture.__init__(out_dir, *, pid=None, bundle_id=None, source="auto", chunk_seconds=8.0, asr_backend="auto", t0=None)`:
- `out_dir: Path` — directory the artifacts are written into (created on `start`).
- `pid: int | None`, `bundle_id: str | None` — target for per-app audio; at least one is required to select the `app` source.
- `source: str` — `"auto"` | `"app"` | `"mic"` (not validated; any other value behaves like the non-`app` path, i.e. falls through to mic).
- `mic_device: str | None` — input-device id used when `source == "mic"` (`None` = default input).
- `track: str` — output naming. `"audio"` → `audio.s16le`/`transcript.*` (default). Any other value (e.g. `"mic"`) → `<track>.s16le`/`<track>_transcript.*`, so a second instance writes alongside the app's without clobbering it.
- `chunk_seconds: float` — window length, clamped to `max(1.0, float(chunk_seconds))`.
- `asr_backend: str` — passed to `asr.create()`; stored as `self.asr_name`.
- `t0: float | None` — session-start epoch fallback; defaults to `now()`.

Public/observed attributes (read by the session/status surface):
- `status: str` — lifecycle/failure string (see Behavior & Failure modes for exact values).
- `mode: str` — `"none"` | `"app"` | `"mic"` (the chosen source kind).
- `segments: int` — count of transcript segments written.
- `asr_errors: int` — count of ASR `transcribe()` exceptions.

Methods: `start() -> None` and `stop() -> None`. Both are synchronous and blocking (the orchestrator is responsible for offloading them off the event loop per `docs/architecture.md`). All other methods (`_build_command`, `_read_loop`, `_transcribe`, etc.) are internal; note `_build_command` is monkeypatched by the smoke test, so its `(cmd, mode)` return shape is effectively a test seam.

stdout/stderr contract of the audio *source* (consumed, not produced, by this scope): the source emits raw signed-16-bit little-endian mono PCM at 16 kHz on stdout; human-readable status on stderr. The Swift helper additionally prints `READY rate=<n> channels=1 fmt=s16le ...` then bytes (see `docs/architecture.md`); `audio.py` does not parse the READY line — it treats stdout as an opaque PCM byte stream.


### Event hook (M0b, feature #26)

`AudioCapture` accepts an optional `emit=None` keyword (an `EventBus.publish`-shaped
callable, normally `CaptureSession.events.publish`). When set, it emits
`transcript_segment` (the jsonl record + count) and `audio_status` {status,mode} at start / no-data failure / stop. Publishing never raises/blocks; with `emit=None` the component is
silent and behaves exactly as before. See [events.md](events.md).

## Behavior
1. `start()` creates `out_dir` (`mkdir parents/exist_ok`).
2. It calls `asr_pkg.create(self.asr_name)`. On exception it records `self._asr_error`, logs a warning, and sets `self._asr = None` (capture continues without transcription) (`audio.py:90-95`).
3. `_build_command()` delegates to `platform.current().audio_source.command(pid, bundle_id, source, rate=SAMPLE_RATE)` and returns `(None, "none")` when that yields `None`, else the `(argv, mode)` it returns. The per-OS selection lives in the platform backends:
   - **macOS** (`MacAudioSource`): if `source` is `"auto"`/`"app"`, the `audiocap` helper exists, and (`pid` or `bundle_id`) is set → `[<helper>, "--rate", "16000"]` plus `--pid <pid>` (preferred) or `--bundle <bundle_id>`; `mode="app"`. Else if `source == "app"` → `None` (unsatisfiable). Else (mic) → `[<helper>, "--rate", "16000", "--mic", <mic_device or "default">]`; `mode="mic"`. **No ffmpeg** — the helper captures the mic via AVFoundation `AVCaptureSession`. **No echo cancellation** (a laptop's built-in mic picks up its own speakers — use headphones; proper AEC without breaking playback is feature #38). `MacAudioSource.list_input_devices()` shells `audiocap --list-mics` (JSON lines) for the selector.
   - **Windows** (`Win32AudioSource`): `source=="auto"/"app"` → `[python, helper/audiocap_win.py, --rate 16000]` with `mode="loopback"` (WASAPI **system loopback** of the default output, auto-reconnecting on device change), when `pyaudiowpatch` + the helper are present; else `app`→`None`. `source=="mic"` → an `ffmpeg` `dshow` argv only when `ffmpeg` is present **and** `CAPTURE_DSHOW_AUDIO` names a device; else `None`. `mode="loopback"` captures the full output mix (the target app plus anything else playing), not a single process.
   The 16 kHz mono s16le stdout contract is identical regardless of which backend/source is chosen.
4. If the command is `None`, `status = "no-audio-source"` and `start()` returns without opening files or a process (`audio.py:98-101`).
5. Otherwise it opens (line-buffered text) `transcript.jsonl` and `transcript.txt`, opens `audio.s16le` (binary), and launches the source via `subprocess.Popen(cmd, stdout=PIPE, stderr=PIPE)`. Any exception here sets `status = "audio-start-failed: <e>"`, tears down the proc and closes files, then returns (`audio.py:106-116`).
6. A daemon stderr-pump thread is started; each non-empty decoded line is stored in `self._last_stderr` and logged as `[audiocap] ...` (`audio.py:221-230`).
7. A daemon reader thread (`audio-reader`) runs `_read_loop` (`audio.py:118-120`).
8. `status` is set to `"running"`, or `"running (asr-unavailable: <err>)"` if ASR failed to load (`audio.py:122`).
9. `_read_loop` (`audio.py:236-269`) repeatedly `stdout.read(4096)` until `_stop` is set, EOF (empty read), or a `ValueError`/`OSError` (stdout closed by `stop()`):
   - On the very first non-empty read, `self._audio_epoch = now()` (first-byte wall-clock anchor).
   - Accumulates `_bytes_in`, writes bytes to `audio.s16le`, appends to `_buf`.
   - While `len(_buf) >= _chunk_bytes` (`chunk_seconds * SAMPLE_RATE * 2`), pops a full chunk and calls `_transcribe`.
10. When the source ends not due to `stop()` and produced zero bytes, `_read_loop` sets a failure status (see Failure modes) (`audio.py:260-269`).
11. `_transcribe(pcm_bytes)` (`audio.py:277-312`):
    - Drops a single odd trailing byte to keep sample alignment.
    - Computes `n_samples`; returns if zero.
    - `chunk_offset = self._samples_consumed / SAMPLE_RATE` (seconds of audio already handed to ASR), then advances `_samples_consumed` by `n_samples`.
    - If no ASR backend, returns (offset accounting still advanced, so `audio.s16le` and offsets stay consistent).
    - Uses `epoch = self._audio_epoch or self.t0`, converts bytes to float32 in [-1, 1) via `np.frombuffer(..., "<i2") / 32768.0`, and calls `self._asr.transcribe(pcm, SAMPLE_RATE)`.
    - For each returned `Segment`, computes `abs_start = epoch + chunk_offset + seg.start` (same for end), and writes a JSONL record and a `txt` line; increments `segments`.
12. `stop()` (`audio.py:150-182`):
    - Sets `_stop`.
    - Kills the source first (`_kill_proc`: `terminate`, wait 3 s, then `kill`, wait 2 s) so stdout reaches EOF.
    - Joins the reader thread (timeout 5 s) and records whether it exited; then closes the proc stdout fd.
    - Only if the reader provably exited: `_flush_chunk(final=True)` (transcribes the remaining buffer if `>= MIN_TAIL_BYTES`) and closes files. If the reader is still alive, it logs a warning and leaves files open for GC to avoid a close+write race.
    - Closes the ASR backend (`self._asr.close()`), swallowing exceptions.
    - Sets terminal status: unless `status` already contains `"failed"`, `"unavailable"`, or `"no-audio-source"`, it becomes `"stopped (asr-errors=N)"` if `asr_errors`, else `"stopped"`.

## Invariants & constraints
- 16 kHz mono signed-16-bit-LE PCM end to end (`SAMPLE_RATE`, `BYTES_PER_SAMPLE`); matches the `docs/architecture.md` "Audio is always 16 kHz mono s16le" constraint. The helper is invoked with `--rate 16000` for both app and `--mic` paths (each emits the same s16le contract, resampling internally as needed).
- Reader-before-files on shutdown: the source is killed and the reader joined BEFORE flushing/closing transcript files (`docs/architecture.md` hard constraint; implemented in `stop()`). Files are closed only when the reader is provably gone.
- Surface failures, don't swallow them: terminal status preservation in `stop()` never overwrites a `failed`/`unavailable`/`no-audio-source` status with `stopped`/`running`; `asr_errors` is exposed and carried into the stopped status (`docs/architecture.md` hard constraint).
- Roll back on partial start: a failure during file-open/Popen tears down the child and closes files before returning (`docs/architecture.md`).
- Capture loops never die: `_read_loop` and the stderr pump catch their own errors; `_transcribe` counts ASR failures rather than propagating them, so one bad window cannot stop the session.
- Offset monotonicity: `chunk_offset` derives from `_samples_consumed`, which advances even when ASR is absent or a window contains no usable samples, so offsets and `audio.s16le` stay aligned.
- Timestamp anchor: absolute stamps use the first-PCM-byte epoch (`_audio_epoch`), falling back to `t0` only if no bytes were ever read (no segments are produced in that case anyway).
- The Swift helper is a process boundary, not a library: `audio.py` only spawns it and reads its stdout/stderr; it does not import or parse its internals beyond the opaque PCM stream.
- Platform: the source command is OS-specific and selected by `platform.current().audio_source` (macOS: ScreenCaptureKit helper / avfoundation; Windows: ffmpeg dshow, with per-app WASAPI loopback still TODO). The chunking/ASR/transcript logic in this scope is OS-neutral. macOS arm64 venv is required for mlx-whisper (the ASR side).

### Mic captured via AVFoundation, not SCK (v3, #88)

The **microphone** is captured through a separate **AVFoundation** path (`AVCaptureSession` +
`AVCaptureAudioDataOutput`), NOT ScreenCaptureKit's `captureMicrophone`. SCK delivers a Bluetooth-HFP
headset at **8 kHz CVSD narrowband** (telephone grade); a direct AVCaptureSession on the SAME device
negotiates **16 kHz mSBC wideband** (confirmed 2026-06-25: SCK mic=8000 while `ffmpeg -f avfoundation
-i :1` and the device nominal rate are 16000 concurrently). Since a headset boom mic sits at the mouth
(high SNR, no room echo), wideband BT can beat the built-in mic for transcription. So in v3:

- **SCK captures app/system audio only** (the `Audio` output → `main_buf`). It is built only when the
  main track is app audio (`audio_source != "mic"`); when the mic IS the main track there is no
  SCStream at all.
- **The mic is an independent AVFoundation capture** (`crates/platform/src/macos_mic.rs`,
  `start_mic_capture_avf`) at the device's NATIVE wideband rate, feeding the right worker buffer
  (`main_buf` when the mic is the main track, else the separate `mic_buf`). macOS runs the SCStream and
  the AVCaptureSession **concurrently in one process** — the "no two concurrent audio SCStreams in a
  process" limit does NOT cross frameworks, so app audio + a wideband mic coexist.
- Engine handles (`crates/engine/src/lib.rs`): `inner.audio_capture` (the SCK app stream) and
  `inner.mic_capture` (the AVF mic) are separate. `set_mic_device` drops/rebuilds ONLY the mic capture
  (the app stream is untouched); the watchdog (#86) rebuilds each independently (app stale →
  `build_audio_stream`, mic stale → `build_mic_capture`), dropping the stalled handle first.
- The output is left at its **NATIVE** format — `audioSettings = nil`. **Pinning Int16 via
  `audioSettings` silently stops `AVCaptureAudioDataOutput` from delivering ANY buffers on macOS**
  (confirmed live 2026-06-25: device resolved + authorized + `isRunning=true` but the delegate never
  fired). The delegate reads the ASBD (`mSampleRate`, `mFormatFlags`, `mBitsPerChannel`,
  `mChannelsPerFrame`) and converts to mono i16 (Float32 → scaled i16, or native Int16 reinterpret,
  taking channel 0). Device resolution + the chosen device + the first buffer's format are logged to
  `~/.capture/mic-avf.log` (the daemon's stderr is `/dev/null` under the launched app). Note the SCK
  enumeration (`/v1/audio/mics`) and AVFoundation's `AVCaptureDevice` list can differ — AVFoundation
  sees a BT-HFP mic SCK doesn't — so resolution matches by `uniqueID`/prefix/`localizedName`. The #87
  empirical rate detection in `audio_worker.rs` stays as the safety net.
- **Platform abstraction.** Both the enumeration and the capture sit behind platform-neutral seams so a
  Windows (WASAPI) adapter slots in: `audio_input_devices()` (macOS impl enumerates via `AVCaptureDevice` —
  the SAME source the capture resolves against, so `/v1/audio/mics` lists the BT-HFP mic ScreenCaptureKit's
  `AudioInputDevice::list()` omits), and `start_mic_capture(device_id, on_samples) -> MicCapture` (NOT a
  backend-specific name; macOS = AVFoundation). The GUI mic picker re-polls `/v1/audio/mics` each ~1 s tick
  so it auto-updates when an input device connects/disconnects (the enumeration is a cheap in-process call).
- **Validated live 2026-06-25:** the Xiaomi Buds captured at **16 kHz** (vs SCK's 8 kHz) with real
  spectral energy above 4.5 kHz (16.5 dB below full-band → genuine wideband) and a clean bilingual
  EN/RU transcript under auto language detection.

Device selection: `start_mic_capture_avf(device_id, …)` resolves `""`/`"default"` to the system default
input; otherwise it matches the requested id against enumerated devices by **exact `uniqueID`**, then
**prefix either direction** (handles composite ids like `"<uid>:input"`), then case-insensitive
**`localizedName`**, falling back to the default device if nothing matches.

## Failure modes & handling
- **Wrong mic sample rate → garbled ASR; empirical rate detection (v3, #87).** _Since #88 the mic no
  longer flows through SCK, so the specific Bluetooth-HFP half-rate mislabel below cannot occur for the
  mic — #87 now runs purely as a safety net for any source still reporting a non-16 kHz rate (and it
  also re-measures the AVFoundation mic's native rate, which is genuine, not mislabeled)._ App/system
  audio is resampled to 16 kHz by SCK, but a mic arrives at its native rate. The platform layer's
  `buffer_sample_rate` is **unreliable for Bluetooth-HFP mics under SCK**: the stream is configured
  `with_sample_rate(16000)` (shared app+mic config), and SCK passes the BT mic's native **8 kHz** through
  while labeling the buffer (both its format description and timing) as the requested **16 kHz** — so it
  reports 16 kHz, no resample runs, and `mic.s16le` is stored at half rate → 2× fast/pitched → repeated
  Whisper hallucinations. **Fix (`crates/engine/src/audio_worker.rs`):** the worker measures the TRUE rate
  out of band from **delivered-samples ÷ wall-clock** over the first `PROBE_SECONDS` (1.5 s), buffering the
  opening audio so none is resampled at the wrong rate, snaps it to the nearest standard rate
  (`round_to_standard_rate`), then resamples everything from the measured rate. The first delivery's burst
  is excluded from the count (it accreted in the shared buffer over an unknown prior interval). App/system
  audio and built-in/USB mics measure ≈16 kHz → unchanged. The measured vs reported rate is emitted as an
  `audio_rate` event (observability). Already-captured half-rate `mic.s16le` is still recoverable with
  `tools/recover_mic_rate.py` (resample 8 k→16 k, re-transcribe with `chunk_seconds` ≥ 24). Even fixed,
  BT-HFP is telephone-grade 8 kHz — the built-in mic is higher fidelity.
- **Silent audio-stream stall → watchdog reconnect (v3, #86).** A macOS audio SCStream can silently stop
  delivering sample buffers mid-capture (no error, not even `-3805`), starving the ASR worker and
  freezing the live transcript while screenshots keep flowing. Each audio sink stamps a per-output
  last-delivery time (`main_last_audio` / `mic_last_audio`); a per-session `audio-watchdog` thread checks
  every ~2 s and, when an ACTIVE source has been quiet >8 s (a live stream delivers continuous buffers
  even during silence, so a gap == a stall), rebuilds JUST that source — the SCK app stream via
  `build_audio_stream`, or (since #88) the AVFoundation mic via `build_mic_capture` — reusing the
  persistent worker buffers so transcription resumes seamlessly. The stalled handle is dropped FIRST;
  rebuilds are rate-limited per source to one per stall window; emits an `audio_reconnect` event + a
  session note.
- No source available (no helper, or `source="app"` unsatisfiable): `status = "no-audio-source"`, no files/process created (`audio.py`).
- File-open or Popen failure during `start()`: `status = "audio-start-failed: <exception>"`; proc torn down, files closed; `start()` returns (`audio.py:111-116`).
- ASR backend fails to load: `_asr = None`, capture continues; `status = "running (asr-unavailable: <err>)"`; transcripts will have no segments but `audio.s16le` is still written (`audio.py:90-95, 122`).
- Source exits abnormally before emitting any bytes (and not via `stop()`): `status = "<mode>-audio-failed (rc=<code>): <last stderr or 'no output'>"`. For `mode == "app"` it appends guidance about Screen Recording permission / `-3805` (`audio.py:260-269`).
- ASR `transcribe()` raises: `asr_errors += 1`; logged on the 1st and every 10th error; `status = "running (asr-errors=N)"`; the window is skipped (`audio.py:292-297`).
- Truncated/odd trailing byte in a chunk: trimmed to sample alignment before decode (`audio.py:278-280`).
- Reader thread does not exit within 5 s on `stop()`: files are intentionally left open (logged warning) to avoid a close+write race; relies on GC (`audio.py:171-172`).
- Subprocess ignores `terminate`: escalates to `kill` (`audio.py:188-200`). All stdout-close and ASR-close paths swallow exceptions.

## Outputs / artifacts
Written into `out_dir`:
- `audio.s16le` — raw captured PCM bytes verbatim (16 kHz mono s16le), binary, written incrementally in `_read_loop`.
- `transcript.jsonl` — one JSON object per segment, line-buffered, UTF-8 (`ensure_ascii=False`). Keys (`audio.py:301-307`): `start` (ISO string via `util.iso`), `end` (ISO string), `start_offset` (float seconds, 3 dp), `end_offset` (float seconds, 3 dp), `text` (segment text).
- `transcript.txt` — human-readable, one line per segment: `[<iso start>] <text>` (`audio.py:311`).

Naming is fixed (no token/timestamp in these filenames); the per-session directory naming lives in the session scope (`<output_dir>/capture-<fs_stamp>-<token>/`).

## Configuration
Constructor parameters (no env vars read in this module):
- `source` — default `"auto"` (`auto`/`app`/`mic`).
- `chunk_seconds` — default `8.0`, floored at `1.0`.
- `asr_backend` — default `"auto"` (resolved by `asr.create`; per architecture map, `auto` → local whisper, fallback Riva).
- `pid` / `bundle_id` — default `None`; one required for the `app` source; `pid` takes precedence over `bundle_id`.
- `t0` — default `now()`; used only as the timestamp fallback before first byte.
- Source discovery (helper path / ffmpeg / device) is owned by the platform backend, not this
  scope: macOS looks for `<repo>/helper/audiocap` and `ffmpeg` (avfoundation `:default`); Windows
  reads `CAPTURE_DSHOW_AUDIO` for the ffmpeg dshow device. See [platform-abstraction.md](platform-abstraction.md).

## Known limitations / open items
- **Audio is per-APPLICATION, not per-window** (macOS hard limit). The `audiocap` helper builds an `SCContentFilter(display:, including:[app], exceptingWindows:[])` — ScreenCaptureKit (and Core Audio process taps) scope audio to a process/app, with no per-window API. So two sessions targeting two windows of the **same** process (e.g. two browser windows, two YouTube tabs) capture the **identical** app-wide stream; their transcripts will match. The daemon (`server._start_session`) detects this — when a new app-audio session's `pid` matches a live session already capturing that pid's audio, it appends a session **note** so the duplication is visible rather than looking like a bug. To get distinct audio, capture from distinct processes (different apps). Screenshots, by contrast, ARE per-window (`window_id`).
- Offline windowing, not true streaming: recognition runs on fixed `chunk_seconds` windows, so segment boundaries and latency are coarse; timestamps can drift if the source inserts silence gaps (noted in the module docstring / README).
- `source` is not validated; an unrecognized value silently behaves like the mic path.
- Mic device is hard-coded to avfoundation `:default`; no device selection.
- The helper's `READY rate=... fmt=s16le` handshake is not parsed/verified — the code trusts the source to honor the 16 kHz mono s16le contract.
- If the reader thread wedges on `stop()`, transcript files are left unclosed (data already line-buffered/flushed survives, but the final tail chunk is skipped). This is a deliberate trade-off, but a wedged reader is described as "rare (child is dead)".
- `t0` fallback for `epoch` is effectively unreachable for emitted segments (segments only exist once bytes arrived, which sets `_audio_epoch`); it remains a defensive default.

## Tests
- `tests/smoke.py::test_audio_pipeline` (`smoke.py:78-116`) exercises the full chunk→ASR→transcript path hermetically: it stubs `asr.create` with a `StubASR` that returns one `Segment(start=0.5, ...)` per call, monkeypatches `_build_command` to a portable streamer `([sys.executable, "-c", <copy-file-to-stdout>, <20s-silence .s16le>], "file")` (cross-platform replacement for the old `cat`), runs `start()`/`stop()`, and asserts:
  - 20 s at `chunk_seconds=8.0` → 3 segments (8 + 8 + 4; the 4 s tail clears `MIN_TAIL_BYTES` and is flushed in `stop()`).
  - `audio.s16le` size == `SAMPLE_RATE * 20 * 2` (raw bytes preserved verbatim).
  - `transcript.jsonl` line count == `segments`.
  - `start_offset` values are non-decreasing and the first is `0.5` (offset accounting + segment-relative timestamps compose correctly).
- Not covered by the hermetic smoke test (validated manually / per README): the real ScreenCaptureKit per-app helper path, real ASR backends, the mic/ffmpeg command, and the failure-status branches (`no-audio-source`, `*-audio-failed`, `audio-start-failed`). Recommended additions: unit tests over `_build_command` for each `source`/helper/ffmpeg/pid-bundle combination, and a test asserting `stop()` preserves a failure status instead of overwriting it with `stopped`.
