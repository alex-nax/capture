# Spec: Audio

_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

## Purpose
Capture a single audio stream for a session, slice it into fixed-length windows, run each window through an ASR backend, and write timestamped transcripts plus the raw PCM. The scope owns: source selection (per-app helper vs. microphone via `ffmpeg`), the 16 kHz mono s16le byte contract, chunking and offset accounting, anchoring timestamps to first-byte wall-clock arrival, and keeping audio/ASR failures visible in the session status surface. It deliberately knows nothing about other capture components or the MCP layer (see `docs/architecture.md` dependency rules).

## Files
- `src/capture_mcp/audio.py` — the entire scope: `AudioCapture` class, reader/transcribe loop, lifecycle (`start`/`stop`), and teardown. The **source command** (which subprocess emits the PCM) is built per-OS by the platform abstraction, not here.

Collaborators referenced but out of scope: `src/capture_mcp/platform/` (the `AudioSource.command(...)` that selects the helper/ffmpeg; see [platform-abstraction.md](platform-abstraction.md) — `helper_path()` now lives in `platform/macos.py`), `src/capture_mcp/asr/` (the `ASRBackend`/`Segment` interface and `create()` factory), `src/capture_mcp/util.py` (`now`, `iso`), `helper/audiocap` (compiled Swift helper, a process boundary).

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

## Behavior
1. `start()` creates `out_dir` (`mkdir parents/exist_ok`).
2. It calls `asr_pkg.create(self.asr_name)`. On exception it records `self._asr_error`, logs a warning, and sets `self._asr = None` (capture continues without transcription) (`audio.py:90-95`).
3. `_build_command()` delegates to `platform.current().audio_source.command(pid, bundle_id, source, rate=SAMPLE_RATE)` and returns `(None, "none")` when that yields `None`, else the `(argv, mode)` it returns. The per-OS selection lives in the platform backends:
   - **macOS** (`MacAudioSource`): if `source` is `"auto"`/`"app"`, the `audiocap` helper exists, and (`pid` or `bundle_id`) is set → `[<helper>, "--rate", "16000"]` plus `--pid <pid>` (preferred) or `--bundle <bundle_id>`; `mode="app"`. Else if `source == "app"` → `None` (unsatisfiable). Else if `ffmpeg` is on `PATH` → the `avfoundation :default` argv; `mode="mic"`. Else `None`.
   - **Windows** (`Win32AudioSource`): `source=="app"` → `None` (per-app WASAPI loopback not yet wired, feature #21). `auto`/`mic` → an `ffmpeg` `dshow` argv only when `ffmpeg` is present **and** `CAPTURE_DSHOW_AUDIO` names a device; else `None`.
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
- 16 kHz mono signed-16-bit-LE PCM end to end (`SAMPLE_RATE`, `BYTES_PER_SAMPLE`); matches the `docs/architecture.md` "Audio is always 16 kHz mono s16le" constraint. The helper is invoked with `--rate 16000`; ffmpeg is forced to `-ac 1 -ar 16000 -f s16le`.
- Reader-before-files on shutdown: the source is killed and the reader joined BEFORE flushing/closing transcript files (`docs/architecture.md` hard constraint; implemented in `stop()`). Files are closed only when the reader is provably gone.
- Surface failures, don't swallow them: terminal status preservation in `stop()` never overwrites a `failed`/`unavailable`/`no-audio-source` status with `stopped`/`running`; `asr_errors` is exposed and carried into the stopped status (`docs/architecture.md` hard constraint).
- Roll back on partial start: a failure during file-open/Popen tears down the child and closes files before returning (`docs/architecture.md`).
- Capture loops never die: `_read_loop` and the stderr pump catch their own errors; `_transcribe` counts ASR failures rather than propagating them, so one bad window cannot stop the session.
- Offset monotonicity: `chunk_offset` derives from `_samples_consumed`, which advances even when ASR is absent or a window contains no usable samples, so offsets and `audio.s16le` stay aligned.
- Timestamp anchor: absolute stamps use the first-PCM-byte epoch (`_audio_epoch`), falling back to `t0` only if no bytes were ever read (no segments are produced in that case anyway).
- The Swift helper is a process boundary, not a library: `audio.py` only spawns it and reads its stdout/stderr; it does not import or parse its internals beyond the opaque PCM stream.
- Platform: the source command is OS-specific and selected by `platform.current().audio_source` (macOS: ScreenCaptureKit helper / avfoundation; Windows: ffmpeg dshow, with per-app WASAPI loopback still TODO). The chunking/ASR/transcript logic in this scope is OS-neutral. macOS arm64 venv is required for mlx-whisper (the ASR side).

## Failure modes & handling
- No source available (no helper + no ffmpeg, or `source="app"` unsatisfiable): `status = "no-audio-source"`, no files/process created (`audio.py:98-101`).
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
