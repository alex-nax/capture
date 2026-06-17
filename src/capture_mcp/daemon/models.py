"""Pydantic models = the `/v1` API contract ("contract firewall").

Source of truth for the daemon's request/response shapes. `StartSessionRequest`
validates POST /v1/sessions at runtime; the response models are NOT enforced at
runtime (the daemon serves engine dicts, resilient to benign additions) but ARE
pinned by the contract test, which (a) round-trips live responses through them
and (b) compares `v1_schema()` to a checked-in golden — so the API can't drift
silently, and the JSON Schema can generate the GPUI app's Rust types.

Pydantic is already a transitive dependency (via `mcp`), so this adds nothing to
the install. Models use `extra="forbid"` so an unexpected field is a contract
breach, caught in CI rather than shipped.
"""

from __future__ import annotations

from pydantic import BaseModel, ConfigDict, model_validator


def _present(v: object) -> bool:
    if v is None:
        return False
    if isinstance(v, str):
        return bool(v.strip())
    return True


class _Strict(BaseModel):
    model_config = ConfigDict(extra="forbid")


class StartSessionRequest(_Strict):
    """Body of POST /v1/sessions (mirrors the MCP `capture_start` args)."""

    output_dir: str
    command: str | None = None
    pid: int | None = None
    #: Optional refinement of a `pid`/`app_name` target: pin screenshots to this exact
    #: window (a `CGWindowID`/HWND from the picker). Without it the screenshotter grabs
    #: the app's *primary* window — wrong when one process owns several (e.g. two Chrome
    #: windows share a pid). Audio stays per-process, so it's unaffected.
    window_id: int | None = None
    app_name: str | None = None
    bundle_id: str | None = None
    screenshot_interval: float = 1.0
    screenshot_format: str = "png"
    screenshot_resolution: str | None = None
    screenshot_jpeg_quality: int | None = None
    capture_screenshots: bool = True
    capture_audio: bool = True
    audio_source: str = "auto"
    #: Optional input-device id (from GET /v1/audio/mics). When set, the session ALSO
    #: records that microphone as a SEPARATE track (mic.s16le / mic_transcript.jsonl),
    #: never mixed with the app audio. The GUI sets this on one app's session only.
    mic_device: str | None = None
    #: Transcription chunk length; ``None`` uses the persisted setting (default 30 s).
    #: 8 s chunks made Whisper hallucinate on pauses / non-English audio.
    audio_chunk_seconds: float | None = None
    asr_backend: str = "auto"
    cwd: str | None = None
    #: Capture preset (#54): one choice that records the capture intent + the default index preset for
    #: the session (meeting/coding/lecture/auto/general/custom). The GUI also applies its mic/screenshot
    #: defaults; the recorded `index_preset` is what a later index (GUI or MCP) defaults to.
    preset: str | None = None

    @model_validator(mode="after")
    def _exactly_one_target(self) -> "StartSessionRequest":
        if not self.output_dir.strip():
            raise ValueError("output_dir is required")
        present = [n for n in ("command", "pid", "app_name") if _present(getattr(self, n))]
        if len(present) != 1:
            raise ValueError("specify exactly one target: command, pid, or app_name")
        return self

    def session_kwargs(self) -> dict:
        """The CaptureSession(**kwargs) args (everything except output_dir)."""
        return self.model_dump(exclude={"output_dir"})


class SessionSummary(_Strict):
    """The session summary returned by start/stop/status (mirrors `session.py`)."""

    session_id: str
    state: str
    dir: str
    pid: int | None = None
    window_title: str | None = None
    started_at: str | None = None
    stopped_at: str | None = None
    screenshots: int
    screenshot_errors: int
    log_lines: int
    process_running: bool | None = None
    audio_mode: str
    audio_status: str
    transcript_segments: int
    asr_errors: int
    mic_status: str = "off"
    mic_segments: int = 0
    mic_device: str | None = None
    #: Capture preset (#54) + the index preset it implies — recorded so a later index defaults to it.
    capture_preset: str | None = None
    index_preset: str | None = None
    # Capability flags from on-disk artifacts (recomputed each read; reflect pruning).
    has_screenshots: bool = True
    has_audio: bool = True
    has_mic: bool = False
    can_retranscribe: bool = True
    can_index: bool = True
    notes: list[str]


class SessionsResponse(_Strict):
    sessions: list[SessionSummary]


class WindowInfo(_Strict):
    window_id: int
    pid: int
    app_name: str
    title: str
    width: int
    height: int


class WindowsResponse(_Strict):
    windows: list[WindowInfo]
    count: int


class TranscriptSegment(_Strict):
    start: str
    end: str
    start_offset: float
    end_offset: float
    text: str


class TranscriptResponse(_Strict):
    session_id: str
    segments: list[TranscriptSegment]
    count: int


class HealthResponse(_Strict):
    ok: bool
    version: str
    api_version: str
    pid: int
    platform: str
    sessions: dict  # {live: int, history: int}


class ErrorResponse(_Strict):
    error: str


class AsrModelRequest(_Strict):
    """Body of POST /v1/asr/model and POST /v1/asr/models/download."""

    repo: str


class ImportMediaRequest(_Strict):
    """Body of POST /v1/sessions/import — turn an existing audio/video file into a session.

    The daemon extracts the file's audio (and, for video, frames) via the bundled helper,
    then runs ASR, registering the result as a finished session. ``output_dir`` defaults to
    the daemon's runs dir when omitted."""

    path: str
    output_dir: str | None = None
    asr_backend: str = "auto"
    screenshot_interval: float = 2.0

    @model_validator(mode="after")
    def _path_required(self) -> "ImportMediaRequest":
        if not self.path.strip():
            raise ValueError("path is required")
        return self


class IndexRequest(_Strict):
    """Body of POST /v1/sessions/{id}/index — build the multimodal index (#44).

    Caption the session's screenshots with a remote vision LLM and summarize the timeline
    as a binary tree. ``endpoint``/``model`` override the daemon's ``CAPTURE_INDEX_*`` env
    config (so the GUI can carry the LM Studio URL). ``sample_rate`` (0<rate≤1) decimates
    the frames to the leaf set; ``max_leaves`` caps it; ``fuse_transcript`` folds the
    time-aligned transcript into each combine."""

    #: Structured provider config (#52): the daemon composes the chat URL from
    #: ``provider`` + ``host`` + ``port`` (e.g. lmstudio/ollama/openai/custom). A full
    #: ``endpoint`` URL still wins when given (back-compat with CAPTURE_INDEX_URL / the old GUI).
    provider: str | None = None
    host: str | None = None
    port: int | None = None
    endpoint: str | None = None
    model: str | None = None
    sample_rate: float = 0.5
    max_leaves: int = 512
    fuse_transcript: bool = True
    #: Per-frame prompt PRESET ("auto"/"meeting"/"lecture"/…) — what's right for a meeting is
    #: wrong for a lecture. Custom prompts (typically crafted by a frontier model calling
    #: capture_index, executed cheaply by the LOCAL vision model), saved to the session for tuning:
    #:   • ``leaf_prompt`` + ``leaf_schema`` → a custom STRUCTURED extractor (one schema per frame).
    #:   • ``leaf_prompt`` alone → a custom free-text caption.
    #:   • ``classify_prompt`` → overrides the auto classifier's prompt.
    #: ``None`` → fall back to the session's recorded `index_preset` (#54), then "auto".
    prompt_preset: str | None = None
    leaf_prompt: str | None = None
    leaf_schema: dict | None = None
    classify_prompt: str | None = None
    #: Override the base longest-edge image downscale for this build (default 1024 via env). Code/
    #: terminal leaves are auto-bumped to CAPTURE_INDEX_CODE_MAX_PX (2048) regardless; raise this to
    #: lift the floor for a whole code-heavy build (daf420 study: 1024→2048 took UE code 0.42→0.88).
    max_px: int | None = None

    @model_validator(mode="after")
    def _bounds(self) -> "IndexRequest":
        if not (0.0 < self.sample_rate <= 1.0):
            raise ValueError("sample_rate must be in (0, 1]")
        if self.max_leaves < 1:
            raise ValueError("max_leaves must be >= 1")
        return self


class AsrModelInfo(_Strict):
    repo: str
    name: str
    size_label: str
    downloaded: bool
    active: bool
    downloading: bool = False


class AsrModelsResponse(_Strict):
    backend_available: bool
    active: str
    language: str | None = None
    chunk_seconds: float = 30.0
    models: list[AsrModelInfo]


#: name -> model, the registry the schema + round-trip contract iterate over.
V1_MODELS: dict[str, type[BaseModel]] = {
    "StartSessionRequest": StartSessionRequest,
    "SessionSummary": SessionSummary,
    "SessionsResponse": SessionsResponse,
    "WindowInfo": WindowInfo,
    "WindowsResponse": WindowsResponse,
    "TranscriptSegment": TranscriptSegment,
    "TranscriptResponse": TranscriptResponse,
    "HealthResponse": HealthResponse,
    "ErrorResponse": ErrorResponse,
    "AsrModelRequest": AsrModelRequest,
    "AsrModelInfo": AsrModelInfo,
    "AsrModelsResponse": AsrModelsResponse,
    "ImportMediaRequest": ImportMediaRequest,
    "IndexRequest": IndexRequest,
}


def v1_schema(api_version: str) -> dict:
    """The checked-in `/v1` JSON Schema: one JSON Schema per model, sorted."""
    return {
        "api_version": api_version,
        "models": {name: model.model_json_schema() for name, model in sorted(V1_MODELS.items())},
    }
