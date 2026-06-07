# Spec: ASR (speech recognition backends)
_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

## Purpose
Provide a single, swappable speech-to-text interface (`ASRBackend`) used by the audio capture
pipeline (`audio.py`) to turn mono float32 PCM chunks into timestamped text segments. A factory
(`create`) selects a backend by name, defaulting to local Whisper on this Apple Silicon Mac with an
automatic fallback to a remote NVIDIA Riva / Nemotron endpoint. Per the architecture's dependency
rules, all ASR access goes through this interface and no other module imports a concrete backend
directly.

## Files
- `src/capture_mcp/asr/__init__.py` — registry / factory (`create`, `available_backends`); re-exports `ASRBackend`, `Segment`.
- `src/capture_mcp/asr/base.py` — interface + `Segment` dataclass.
- `src/capture_mcp/asr/whisper_local.py` — local Whisper backends (`MlxWhisper`, `FasterWhisper`) and their `load()`.
- `src/capture_mcp/asr/nemotron.py` — remote Riva / Nemotron adapter (`NemotronRiva`) and its `load()`.

## Public contract

### `Segment` (base.py:16–20)
Dataclass with fields:
- `start: float` — seconds, **relative to the chunk passed to `transcribe()`** (not the capture timeline).
- `end: float` — seconds, relative to the chunk.
- `text: str`.

The caller is responsible for adding the chunk's absolute offset to place segments on the capture timeline.

### `ASRBackend` (base.py:23–31)
Class attributes:
- `name: str` — backend identifier (`"base"` on the base class; concrete backends override).
- `target_sample_rate: int = 16000` — the rate the backend wants its PCM resampled to.

Methods:
- `transcribe(self, pcm: np.ndarray, sample_rate: int) -> list[Segment]` — abstract on the base
  (`raise NotImplementedError`). `pcm` is mono float32 in range `[-1, 1]`; `sample_rate` is that
  chunk's rate. Returns a list of `Segment`.
- `close(self) -> None` — optional cleanup hook; base implementation is a no-op.

### `create(name: str = "auto") -> ASRBackend` (`__init__.py`:22–46)
Factory. `name` is lowercased (None/empty coerced to `"auto"`). Mapping:
- `"local"` or `"whisper"` → `whisper_local.load()`.
- `"nemotron"` or `"riva"` → `nemotron.load()`.
- `"auto"` (default) → try `whisper_local.load()`; on any `Exception`, log a warning and fall back to `nemotron.load()`.
- Anything else → raises `ValueError(f"unknown ASR backend {name!r}; choose from {available_backends}")`.

Backends are imported lazily inside each branch so a missing optional dependency only fails the
backend it belongs to.

### `available_backends` (`__init__.py`:19)
Tuple `("auto", "local", "whisper", "nemotron", "riva")`. Note `"auto"` is the only name not also a
concrete backend selector.

### `whisper_local.load() -> ASRBackend` (whisper_local.py:91–105)
Tries constructors in order `(MlxWhisper, FasterWhisper)`; returns the first that constructs
successfully (logging `ASR backend loaded: <name>`). If both fail, raises `RuntimeError` with install
hints and the accumulated per-constructor error strings.

### `nemotron.load() -> ASRBackend` (nemotron.py:94–95)
Returns `NemotronRiva()` (constructed from env vars). No internal try/fallback.

## Behavior

Runtime steps for the common `create("auto")` path:

1. `create("auto")` calls `whisper_local.load()`.
2. `load()` instantiates `MlxWhisper()` first. Its `__init__` does `import mlx_whisper` (validates the
   package is importable) and resolves the model name from `CAPTURE_WHISPER_MODEL` or the default
   `mlx-community/whisper-large-v3-turbo` (whisper_local.py:28, 49–52).
3. If `mlx-whisper` is unavailable (ImportError) or constructing fails, `load()` records the error and
   tries `FasterWhisper()`. Its `__init__` imports `from faster_whisper import WhisperModel`, resolves
   the model from `CAPTURE_WHISPER_MODEL` or default `"base"`, and constructs `WhisperModel(model,
   device="cpu", compute_type="int8")` (whisper_local.py:69–76).
4. If both fail, `load()` raises `RuntimeError`; in the `"auto"` path `create` catches it, logs
   `local ASR unavailable (...); trying Riva/Nemotron`, and calls `nemotron.load()`.

`MlxWhisper.transcribe` (whisper_local.py:54–66):
1. Calls `mlx_whisper.transcribe(pcm.astype(np.float32), path_or_hf_repo=self._model, word_timestamps=False)`.
2. Maps `result["segments"]` to `Segment(start, end, text.strip())`, skipping segments whose stripped text is empty.

`FasterWhisper.transcribe` (whisper_local.py:78–88):
1. Writes the PCM to a temp WAV via `_write_wav` (mono, 16-bit LE, clipped to `[-1,1]` then scaled by 32767).
2. Calls `self._model.transcribe(path, vad_filter=True)` (VAD filtering enabled).
3. Maps returned segments to `Segment(start, end, text.strip())`, skipping empty text.
4. In a `finally`, deletes the temp WAV (`Path(path).unlink(missing_ok=True)`).

`NemotronRiva.__init__` (nemotron.py:33–61):
1. `import riva.client`.
2. Resolves `server` (default `localhost:50051`), `api_key`, `function_id`, `language` (default
   `en-US`), `model` (no default) from constructor args or `CAPTURE_RIVA_*` env vars.
3. Builds gRPC metadata: if `api_key` set, appends `("authorization", "Bearer <key>")` and sets
   `use_ssl=True`; if `function_id` set, appends `("function-id", <id>)` and sets `use_ssl=True`.
4. Constructs `riva.client.Auth(uri=server, use_ssl=..., metadata_args=metadata)` and
   `riva.client.ASRService(auth)`; logs `Riva ASR connected: <server> (lang=<lang>)`.

`NemotronRiva.transcribe` (nemotron.py:63–91):
1. Converts PCM to 16-bit LE bytes (clip to `[-1,1]`, scale by 32767).
2. Builds a `RecognitionConfig` with `LINEAR_PCM`, the chunk's `sample_rate_hertz`, the configured
   `language_code`, `max_alternatives=1`, `enable_automatic_punctuation=True`,
   `enable_word_time_offsets=True`; sets `config.model` only if a model was configured.
3. Calls `self._asr.offline_recognize(pcm16, config)`.
4. For each result with alternatives, takes alternative 0; derives `start`/`end` from the first/last
   word's `start_time`/`end_time` divided by 1000.0 (ms → s), or `0.0` if no words; appends a
   `Segment` only when the stripped transcript is non-empty.

## Invariants & constraints
- **Single interface.** All ASR access goes through `ASRBackend`; adding a backend = new module + one
  branch in `asr/__init__.py:create`. Nothing else imports a concrete backend directly (architecture.md
  dependency rules).
- **Lazy imports.** Concrete backend modules and their third-party deps must only be imported inside
  the relevant `create`/`load` branch, so one missing optional dependency does not break the others or
  the server import.
- **PCM contract.** Backends receive mono float32 PCM in `[-1, 1]`; segment timestamps are relative to
  the chunk, not the global timeline (base.py docstring).
- **Audio format.** The pipeline is 16 kHz mono s16le end to end and `target_sample_rate` defaults to
  16000 (architecture.md naming/conventions; base.py:25).
- **No stdout pollution.** This scope logs only via the `logging` module (to stderr by the server's
  config) and must never `print()` — stdout is the MCP transport (architecture.md hard constraints).
- **Failures stay visible.** ASR load/transcribe failures must surface to the caller so the session can
  reflect them in `audio_status` / `asr_errors`; this scope does not swallow them silently (it raises or
  logs warnings).
- **arm64 venv required for mlx-whisper** (architecture.md platform note).

## Failure modes & handling
- **mlx-whisper not installed / fails to construct:** `MlxWhisper()` raises; `whisper_local.load()`
  catches, records the error, and tries `FasterWhisper`.
- **Both local backends unavailable:** `load()` raises `RuntimeError` listing install hints and the
  per-constructor errors. In the `"auto"` path this is caught by `create`, which logs a warning and
  falls back to Riva/Nemotron.
- **Unknown backend name:** `create` raises `ValueError`.
- **Riva package missing / server unreachable / auth wrong:** `NemotronRiva.__init__` raises (import or
  connection error). `nemotron.load()` does not catch it, so the exception propagates to the caller. In
  the `"auto"` fallback, if Riva also fails the original local error is already lost (replaced by the
  Riva exception).
- **Empty / silent audio:** transcribe methods return `[]` (empty/whitespace segments are filtered out;
  Riva returns no results).
- **faster-whisper temp WAV:** always deleted in a `finally`, even if `transcribe` raises.
- **Network on first use:** local Whisper backends download model weights on first transcription, so the
  first call needs network access (whisper_local.py docstring).

## Outputs / artifacts
- This scope writes **no persistent files**. Its product is in-memory `list[Segment]` returned to the
  caller (`audio.py`), which is responsible for transcript files.
- `FasterWhisper` writes a transient temp WAV via `tempfile.mkstemp(suffix=".wav", prefix="capmcp-")`
  in the temp dir, then deletes it. The file descriptor from `mkstemp` is closed immediately and the
  file is reopened by path through the `wave` module (whisper_local.py:32–43).

## Configuration

Local Whisper (whisper_local.py):
- `CAPTURE_WHISPER_MODEL` — model name/repo. Used by both backends.
  - For `MlxWhisper`: an mlx-community HF repo; default `mlx-community/whisper-large-v3-turbo`.
  - For `FasterWhisper`: a faster-whisper model name; default `base`.
- `FasterWhisper.compute_type` — constructor arg, default `"int8"`; no env var. Device is hardcoded `"cpu"`.

Riva / Nemotron (nemotron.py:43–47):
- `CAPTURE_RIVA_SERVER` — Riva gRPC `host:port`; default `localhost:50051`.
- `CAPTURE_RIVA_API_KEY` — bearer token for NVIDIA-hosted endpoints; optional. Presence enables SSL.
- `CAPTURE_RIVA_FUNCTION_ID` — NVIDIA-hosted function id selecting the model; optional. Presence enables SSL.
- `CAPTURE_RIVA_LANG` — language code; default `en-US`.
- `CAPTURE_RIVA_MODEL` — explicit self-hosted Riva model name; optional, no default (only set on config when present).

Factory:
- `create(name)` — `name` default `"auto"`; accepted values per `available_backends`.

## Known limitations / open items
- **`mlx-community/whisper-base` does NOT exist.** The documented `CAPTURE_WHISPER_MODEL` example in
  the code comment is `mlx-community/whisper-tiny`. For mlx-whisper, set a valid repo such as
  `mlx-community/whisper-tiny` or rely on the default `mlx-community/whisper-large-v3-turbo`. Do not set
  `mlx-community/whisper-base` (it will fail to download).
- **Riva/Nemotron is coded but unverified.** The remote adapter has not been validated end to end
  against a live Riva server in this project. Treat `NemotronRiva` and the `CAPTURE_RIVA_*` config as
  best-effort/untested.
- **Offline-recognize, not streaming.** The Riva path uses chunked `offline_recognize`; switching to
  Riva's cache-aware streaming API (the model's headline feature) is noted as a future drop-in change to
  `transcribe` (nemotron.py docstring).
- **`auto` fallback masks the local error.** When both local and Riva fail in `auto`, the surfaced
  exception is the Riva one; the original local `RuntimeError` is only logged as a warning.
- **No `close()` overrides.** Neither concrete backend overrides `close()`; the Riva gRPC channel /
  Whisper model are not explicitly torn down.
- **`compute_type` not env-configurable**, and faster-whisper is CPU-only here (`device="cpu"` hardcoded).

## Tests
- `tests/smoke.py` is the project smoke harness; ASR coverage here should be verified through it where
  applicable (confirm at the path — this spec does not assume specific assertions exist).
- Suggested checks for this scope:
  - `create("auto")` returns a backend or falls back cleanly when local deps are absent; `create("bogus")`
    raises `ValueError`.
  - With a deterministic small model (e.g. `CAPTURE_WHISPER_MODEL=mlx-community/whisper-tiny`),
    `transcribe` on a short WAV returns non-empty `Segment`s with monotonic `start <= end` and stripped text.
  - `_write_wav` round-trips: produced WAV is mono/16-bit/`sample_rate`, and the temp file is removed
    after `FasterWhisper.transcribe`.
  - Empty/silent PCM returns `[]`.
  - Riva: schema/shape tests against a mocked `riva.client` (live-server tests are out of scope until the
    adapter is verified).
