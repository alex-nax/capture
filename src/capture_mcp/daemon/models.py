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
    app_name: str | None = None
    bundle_id: str | None = None
    screenshot_interval: float = 1.0
    screenshot_format: str = "png"
    screenshot_resolution: str | None = None
    screenshot_jpeg_quality: int | None = None
    capture_screenshots: bool = True
    capture_audio: bool = True
    audio_source: str = "auto"
    audio_chunk_seconds: float = 8.0
    asr_backend: str = "auto"
    cwd: str | None = None

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
}


def v1_schema(api_version: str) -> dict:
    """The checked-in `/v1` JSON Schema: one JSON Schema per model, sorted."""
    return {
        "api_version": api_version,
        "models": {name: model.model_json_schema() for name, model in sorted(V1_MODELS.items())},
    }
