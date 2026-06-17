"""MCP server exposing on-demand process capture.

Tools:
  * ``capture_start``  - begin capturing a process's window, logs, and audio.
  * ``capture_stop``   - stop a capture and flush to disk.
  * ``capture_status`` - list active/finished captures and their counters.

Run over stdio (the default MCP transport)::

    capture-mcp
    # or
    python -m capture_mcp.server
"""

from __future__ import annotations

import logging
import os
import sys

import anyio
from mcp.server.fastmcp import FastMCP

from .core import list_windows as _list_windows
from .core.registry import SessionRegistry
from .core.session import PRUNE_PARTS, CaptureSession
from .daemon.client import DaemonClient, DaemonError

logging.basicConfig(
    level=logging.INFO,
    stream=sys.stderr,  # stdout is the MCP transport — keep logs off it
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
log = logging.getLogger("capture_mcp")

mcp = FastMCP("capture-mcp")

# Tool bodies do blocking work (subprocess launch, ASR model load, thread joins).
# FastMCP runs sync tools ON the event loop, so we declare the handlers async and
# offload that work with anyio.to_thread.run_sync to keep stdio responsive.

# Live tracking + disk-backed history (rebuilt from CAPTURE_SESSION_INDEX /
# ~/.capture/sessions.jsonl at startup) lives in core.registry; this layer
# only orchestrates. This is the EMBEDDED engine, used when no daemon is present.
registry = SessionRegistry()


# -- daemon-first dispatch ----------------------------------------------------
# When a `captured` daemon is running, the tools proxy to it so an MCP agent
# shares one live registry (and, once packaged+signed, one TCC grant) with the
# GUI/CLI. With no daemon — or CAPTURE_MCP_EMBEDDED=1 (headless/CI) — the tools
# run the engine in-process exactly as before. The check is per-call and cheap
# (a ~2s /v1/health probe), so a daemon started/stopped mid-session is picked up.

def _daemon() -> DaemonClient | None:
    if os.environ.get("CAPTURE_MCP_EMBEDDED"):
        return None
    client = DaemonClient.from_discovery()
    return client if (client is not None and client.available()) else None


def _as_value_error(fn):
    """Run a blocking daemon call, mapping DaemonError to ValueError so FastMCP
    surfaces the daemon's message the same way the embedded path's ValueErrors do."""
    try:
        return fn()
    except DaemonError as e:
        raise ValueError(e.message) from e


@mcp.tool()
async def capture_start(
    output_dir: str,
    command: str | None = None,
    pid: int | None = None,
    window_id: int | None = None,
    app_name: str | None = None,
    bundle_id: str | None = None,
    screenshot_interval: float = 1.0,
    screenshot_format: str = "png",
    screenshot_resolution: str | None = None,
    screenshot_jpeg_quality: int | None = None,
    capture_screenshots: bool = True,
    capture_audio: bool = True,
    audio_source: str = "auto",
    mic_device: str | None = None,
    audio_chunk_seconds: float = 8.0,
    asr_backend: str = "auto",
    cwd: str | None = None,
) -> dict:
    """Start capturing a process. Returns a session summary including ``session_id``.

    Specify the target ONE of three ways:
      * ``command`` - a shell command to launch; its stdout/stderr are captured
        (this is the only mode that captures logs) and its window/audio are
        tracked once it appears.
      * ``pid`` - attach to an already-running process by PID.
      * ``app_name`` - attach by (case-insensitive substring of) the app's name,
        e.g. "Safari"; its main on-screen window is used.

    Artifacts are written under ``<output_dir>/capture-<id>/``:
    timestamped PNG screenshots (every ``screenshot_interval`` seconds),
    ``stdout.log``/``stderr.log``/``output.log``, raw ``audio.s16le``, and
    ``transcript.jsonl``/``transcript.txt`` with each recognized speech segment
    stamped with the absolute time it was spoken.

    Args:
        output_dir: Base directory for the session folder (created if missing).
        command: Command line to launch and capture (mutually exclusive with pid/app_name).
        pid: PID of a running process to attach to.
        app_name: App name substring to attach to.
        window_id: Pin screenshots to this exact window (a window_id from
            list_windows). Refines a pid/app_name target — needed when one
            process owns several windows (e.g. two Chrome windows), which pid alone
            can't disambiguate. Audio stays per-process.
        bundle_id: Bundle id for per-app audio (e.g. "com.apple.Safari"); optional.
        screenshot_interval: Seconds between screenshots (default 1.0).
        screenshot_format: Image format: png (default), jpg/jpeg, tiff, gif, bmp.
        screenshot_resolution: Bounding box "WxH" (e.g. "1280x720"); shots are
            scaled to fit inside it preserving aspect ratio, never upscaled. May
            also include the format, e.g. "1280x720/jpg" or "640x480/png", which
            overrides screenshot_format. Omit for native resolution.
        screenshot_jpeg_quality: JPEG quality 0-100 (only when format is jpg).
        capture_screenshots: Capture window screenshots (default True).
        capture_audio: Capture + transcribe audio (default True).
        audio_source: "auto" (per-app helper, else mic), "app", or "mic".
        mic_device: Also record a microphone as a SEPARATE track (mic.s16le /
            mic_transcript.jsonl), in addition to the app audio. A device id from
            list_audio_devices, or "default" for the system default input.
            Acoustic echo cancellation is applied so laptop-speaker bleed is removed.
        audio_chunk_seconds: Audio window size sent to ASR per pass (default 8.0).
        asr_backend: "auto", "local"/"whisper", or "nemotron"/"riva".
        cwd: Working directory for a launched command.
    """
    def _present(v: object) -> bool:
        # pid=0 counts as provided (and is rejected later as invalid); blank
        # strings do not.
        if v is None:
            return False
        if isinstance(v, str):
            return bool(v.strip())
        return True

    provided = [n for n, v in (("command", command), ("pid", pid), ("app_name", app_name)) if _present(v)]
    if len(provided) == 0:
        raise ValueError("specify exactly one target: command, pid, or app_name")
    if len(provided) > 1:
        raise ValueError(f"specify exactly one target, but got: {', '.join(provided)}")

    kwargs = dict(
        command=command, pid=pid, window_id=window_id, app_name=app_name, bundle_id=bundle_id,
        screenshot_interval=screenshot_interval, screenshot_format=screenshot_format,
        screenshot_resolution=screenshot_resolution, screenshot_jpeg_quality=screenshot_jpeg_quality,
        capture_screenshots=capture_screenshots, capture_audio=capture_audio,
        audio_source=audio_source, mic_device=mic_device, audio_chunk_seconds=audio_chunk_seconds,
        asr_backend=asr_backend, cwd=cwd,
    )

    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(
            lambda: _as_value_error(lambda: d.start(output_dir=output_dir, **kwargs))
        )

    session = CaptureSession(output_dir, **kwargs)
    # Register BEFORE the (possibly slow) start so capture_status can already
    # see the session in state "starting"; a failed start stays visible as
    # state "error" instead of vanishing.
    registry.add(session)
    return await anyio.to_thread.run_sync(session.start)


@mcp.tool()
async def capture_stop(session_id: str | None = None) -> dict:
    """Stop a capture and flush everything to disk.

    Args:
        session_id: The session to stop. If omitted and exactly one capture is
            running, that one is stopped; if several are running, an error lists
            them (pass an explicit id). Use ``capture_status`` to see ids.

    Returns the final session summary.
    """
    d = _daemon()
    if d is not None:
        def _stop_via_daemon() -> dict:
            sid = session_id
            if sid is None:
                running = [s for s in d.sessions()["sessions"] if s.get("state") == "running"]
                if not running:
                    return {"stopped": [], "note": "no running captures"}
                if len(running) > 1:
                    raise ValueError(
                        "multiple captures running; pass session_id. Running: "
                        + ", ".join(s["session_id"] for s in running)
                    )
                sid = running[0]["session_id"]
            return _as_value_error(lambda: d.stop(sid))
        return await anyio.to_thread.run_sync(_stop_via_daemon)

    if session_id is None:
        running = registry.running()
        if not running:
            return {"stopped": [], "note": "no running captures"}
        if len(running) > 1:
            raise ValueError(
                "multiple captures running; pass session_id. Running: "
                + ", ".join(s.id for s in running)
            )
        return await anyio.to_thread.run_sync(running[0].stop)

    session = registry.get(session_id)
    if session is not None:
        return await anyio.to_thread.run_sync(session.stop)
    # A session recovered from a previous server's on-disk history is already
    # finished — return its record (mirrors stop() on a stopped live session).
    record = registry.history_record(session_id)
    if record is not None:
        return record
    raise ValueError(f"unknown session_id {session_id!r}")


@mcp.tool()
async def capture_status(session_id: str | None = None) -> dict:
    """Report capture status.

    Args:
        session_id: If given, return that session's summary; otherwise return a
            list of all sessions this server knows about — those it created
            plus finished ones recovered from the on-disk index at startup.
    """
    d = _daemon()
    if d is not None:
        if session_id is not None:
            return await anyio.to_thread.run_sync(lambda: _as_value_error(lambda: d.session(session_id)))
        return await anyio.to_thread.run_sync(d.sessions)

    if session_id is not None:
        summary = registry.summary(session_id)
        if summary is None:
            raise ValueError(f"unknown session_id {session_id!r}")
        return summary
    return {"sessions": registry.summaries()}


@mcp.tool()
async def capture_prune(session_id: str, parts: list[str]) -> dict:
    """Free disk on a FINISHED capture by removing artifacts; returns freed bytes + the
    refreshed capability flags. (Capability flags also appear in `capture_status`:
    `has_screenshots`, `has_audio`, `has_mic`, `can_retranscribe`.)

    Args:
        session_id: the session to prune (must be stopped).
        parts: any of "screenshots" (delete all screenshots), "screenshots_halve" (drop
            every other frame — half the cadence, full timeline), "audio" (remove the raw
            audio.s16le/mic.s16le — frees the most disk but disables `capture_retranscribe`).
    """
    bad = [p for p in parts if p not in PRUNE_PARTS]
    if not parts or bad:
        raise ValueError(f"parts must be a non-empty subset of {list(PRUNE_PARTS)}; bad: {bad}")
    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(lambda: _as_value_error(lambda: d.prune(session_id, parts)))
    return await anyio.to_thread.run_sync(lambda: registry.prune_session(session_id, parts))


@mcp.tool()
async def capture_retranscribe(
    session_id: str,
    asr_backend: str | None = None,
    model: str | None = None,
    language: str | None = None,
    chunk_seconds: float | None = None,
) -> dict:
    """Re-transcribe a saved capture's audio, replacing its transcript — e.g. to upgrade an
    old session with a stronger model, FIX a wrong-language transcript, or re-chunk it. Requires
    the raw audio still present (`can_retranscribe` in `capture_status`). Runs in the background;
    watch `capture_status` `transcript_segments` (or `/v1/events`).

    Args:
        session_id: the session to re-transcribe (must be stopped, with audio).
        asr_backend: "auto" (default), "local"/"whisper", or "nemotron"/"riva".
        model: optional Whisper repo to switch to first (e.g. "mlx-community/whisper-large-v3-turbo").
        language: ISO code to pin (e.g. "ru", "en") to fix mis-detected speech; "" / "auto" =
            auto-detect. Persists as the active transcription language.
        chunk_seconds: transcription window in seconds (default = the active setting, 30 s).
            Larger windows (≥24 s) avoid Whisper's short-chunk hallucination ("Thank you." on pauses).
    """
    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(
            lambda: _as_value_error(
                lambda: d.retranscribe(session_id, asr_backend, model, language, chunk_seconds)
            )
        )
    raise ValueError("re-transcribe needs the running daemon (not available in the embedded MCP path)")


@mcp.tool()
async def transcription_settings(
    language: str | None = None, chunk_seconds: float | None = None
) -> dict:
    """Get or set the persisted transcription settings shared by all new captures + re-transcribes.

    Call with no args to read the current settings; pass a value to change it. These apply on the
    fly — a `language` change takes effect on a running capture's next chunk, so you can correct a
    live transcript without restarting.

    Args:
        language: ISO code to pin (e.g. "ru", "en"); "" / "auto" = auto-detect. Pinning stops
            Whisper mis-detecting short non-English chunks as English and hallucinating.
        chunk_seconds: transcription window in seconds (1–120; default 30). Shorter = lower latency
            but more hallucination on pauses; longer = more accurate.

    Returns the current `{language, chunk_seconds, active_model, backend_available}`.
    """
    d = _daemon()
    if d is None:
        raise ValueError("transcription settings need the running daemon (not the embedded MCP path)")

    def _apply() -> dict:
        if language is not None:
            d.asr_set_language(language)
        if chunk_seconds is not None:
            d.asr_set_chunk(chunk_seconds)
        models = d.asr_models()
        return {
            "language": models.get("language"),
            "chunk_seconds": models.get("chunk_seconds"),
            "active_model": models.get("active"),
            "backend_available": models.get("backend_available"),
        }

    return await anyio.to_thread.run_sync(lambda: _as_value_error(_apply))


@mcp.tool()
async def capture_import(
    path: str, output_dir: str | None = None, asr_backend: str | None = None
) -> dict:
    """Import an existing audio or video file as a capture session.

    Turns a recording you already have (a meeting capture, a screen recording, a voice
    memo) into a normal session — extracts its audio (and, for video, periodic frames),
    runs ASR, and registers it so it shows up in `capture_list` and the GUI's playback
    scrubber. Runs in the background; watch `/v1/events` (`import`/`import_done`) or poll
    `capture_list` for the new session. Audio-only files become audio-only sessions.

    Args:
        path: absolute path to a local audio/video file (anything AVFoundation decodes —
            .m4a/.mp3/.wav/.mov/.mp4/…).
        output_dir: where to create the session dir (defaults to the daemon's runs dir).
        asr_backend: "auto" (default), "local"/"whisper", or "nemotron"/"riva".
    """
    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(
            lambda: _as_value_error(lambda: d.import_media(path, output_dir, asr_backend))
        )
    raise ValueError("import needs the running daemon (not available in the embedded MCP path)")


@mcp.tool()
async def capture_index(
    session_id: str,
    endpoint: str | None = None,
    provider: str | None = None,
    host: str | None = None,
    port: int | None = None,
    model: str | None = None,
    sample_rate: float | None = None,
    prompt_preset: str | None = None,
    leaf_prompt: str | None = None,
    leaf_schema: dict | None = None,
    classify_prompt: str | None = None,
    max_px: int | None = None,
) -> dict:
    """Build a multimodal index of a finished capture's screenshots with a remote vision LLM.

    Captions the session's screenshots and summarizes the timeline as a binary tree (leaf
    captions → combined range summaries → a whole-session root summary), so the session
    becomes readable at any zoom level. Runs in the background (progress over `/v1/events`:
    `index`→`index_done`/`_error`); fetch the tree afterward from the daemon
    (`GET /v1/sessions/{id}/index`). Requires `can_index` (screenshots present) and a
    configured, reachable vision endpoint — **disabled unless an LM Studio server is set**
    (via `CAPTURE_INDEX_URL` or the `endpoint` arg). **Daemon-only.**

    Args:
        session_id: the session to index (must be stopped, with screenshots).
        endpoint: full chat URL override (e.g. http://192.168.31.217:1234/v1/chat/completions). Takes
            precedence over provider/host/port (back-compat).
        provider: structured provider id instead of a full URL — "lmstudio" (default port 1234),
            "ollama" (11434), "openai" (cloud, needs a key), or "custom" (host = full base URL).
        host: hostname/IP for the provider (or the full base URL for "custom").
        port: port for the provider (defaults to the provider's standard port).
        model: model id override (e.g. "qwen3.5-9b").
        sample_rate: leaf sampling rate in (0,1] (default 0.5 = caption every other frame).
        prompt_preset: per-frame handling — "auto" (default: classify each frame, then run that
            type's structured extractor), or a fixed type: "meeting" (participant names + active
            speaker), "lecture" (slide titles, code, key terms), "coding", "browsing", etc.
        leaf_prompt: a CUSTOM per-frame prompt — YOU (a frontier model) can craft a prompt tailored to
            this session; the cheap local vision model executes it on every frame. With `leaf_schema`
            it's a structured extractor; alone it's a free-text caption.
        leaf_schema: a JSON Schema (object with a `summary` string + your fields, e.g.
            `{"type":"object","properties":{"summary":{"type":"string"},"speaker":{"type":"string"}},
            "required":["summary"]}`) — the local model returns structured data per frame matching it.
        classify_prompt: a CUSTOM classifier prompt (overrides the default content-type classifier).
        max_px: base longest-edge image downscale for the build (default 1024). Code/terminal frames
            are auto-bumped to 2048 regardless; raise this for a whole code-heavy session — higher
            resolution is the proven lever for small-font/IDE OCR (study: 1024→2048 took UE code
            fidelity 0.42→0.88). Costs ~14% more tokens; leave unset for slides/meeting/video.
    The prompts you pass are saved to `<session>/index_prompts.json` so good ones can be folded back
    into the built-in classifier/extractors.
    """
    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(
            lambda: _as_value_error(
                lambda: d.index(session_id, endpoint, model, sample_rate,
                                prompt_preset=prompt_preset, leaf_prompt=leaf_prompt,
                                leaf_schema=leaf_schema, classify_prompt=classify_prompt,
                                max_px=max_px, provider=provider, host=host, port=port)
            )
        )
    raise ValueError("indexing needs the running daemon (not available in the embedded MCP path)")


@mcp.tool()
async def index_models(provider: str | None = None, host: str | None = None,
                       port: int | None = None, key: str | None = None,
                       url: str | None = None) -> dict:
    """List the vision-LLM models a provider has available (to pick `model` for `capture_index`).

    GETs the provider's `/v1/models`. Pass a structured config (`provider` = lmstudio/ollama/openai/
    custom + `host` + `port`) or a full `url`. Returns `{models: [...], provider, reachable}` —
    `reachable: false` with an empty list if the endpoint can't be reached or needs a `key`. **Daemon-only.**
    """
    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(
            lambda: d.index_models(provider=provider, host=host, port=port, key=key, url=url)
        )
    raise ValueError("listing models needs the running daemon (not available in the embedded MCP path)")


@mcp.tool()
async def list_windows(app_name: str | None = None, pid: int | None = None) -> dict:
    """List on-screen top-level windows (the picker for capture targets).

    Use this to discover what `capture_start` can attach to: each entry has
    `window_id`, `pid`, `app_name`, `title`, `width`, `height`, ordered
    largest-first (the first match is what `capture_start` would target).

    Args:
        app_name: Optional case-insensitive substring filter (e.g. "Safari").
        pid: Optional process id filter.
    """
    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(lambda: _as_value_error(lambda: d.windows(app_name=app_name, pid=pid)))
    windows = await anyio.to_thread.run_sync(lambda: _list_windows(pid=pid, app_name=app_name))
    return {"windows": windows, "count": len(windows)}


@mcp.tool()
async def list_audio_devices() -> dict:
    """List microphone/input devices for `capture_start`'s `mic_device`.

    Returns `{devices: [{id, name, default}]}`. Pass a device `id` (or "default")
    as `mic_device` to record that microphone as a separate track. macOS-only for
    now (other platforms return an empty list).
    """
    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(lambda: _as_value_error(d.audio_mics))
    from .core import platform as _platform

    return {"devices": await anyio.to_thread.run_sync(_platform.current().audio_source.list_input_devices)}


@mcp.tool()
async def capture_set_mic(session_id: str, device: str | None = None) -> dict:
    """Switch the microphone on a RUNNING capture, live (no restart). The mic is recorded as
    a separate track (`mic.s16le` / `mic_transcript.*`); switching appends to it so the
    recording stays continuous.

    Args:
        session_id: the running session to change.
        device: an input-device id from `list_audio_devices` (or "default" for the system
            default) turns the mic on / switches it; null / "" turns the mic OFF.

    Returns the updated session summary (`mic_status` reflects the change). **Daemon-only.**
    """
    d = _daemon()
    if d is not None:
        return await anyio.to_thread.run_sync(
            lambda: _as_value_error(lambda: d.set_mic(session_id, device))
        )
    raise ValueError("live mic switching needs the running daemon (not the embedded MCP path)")


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
