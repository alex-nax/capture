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
import sys
import threading

import anyio
from mcp.server.fastmcp import FastMCP

from .session import CaptureSession

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

_sessions: dict[str, CaptureSession] = {}
_lock = threading.Lock()
MAX_SESSIONS = 100


def _prune_locked() -> None:
    """Evict oldest *finished* sessions to bound retained history. Caller holds _lock.

    Running sessions are never evicted, so this bounds retained finished sessions
    rather than the absolute registry size (which is fine — concurrent live
    captures are few).
    """
    if len(_sessions) <= MAX_SESSIONS:
        return
    # ids are timestamp-prefixed, so lexical order == chronological order.
    finished = sorted(sid for sid, s in _sessions.items() if s.state != "running")
    for sid in finished[: len(_sessions) - MAX_SESSIONS]:
        _sessions.pop(sid, None)


@mcp.tool()
async def capture_start(
    output_dir: str,
    command: str | None = None,
    pid: int | None = None,
    app_name: str | None = None,
    bundle_id: str | None = None,
    screenshot_interval: float = 1.0,
    screenshot_format: str = "png",
    screenshot_resolution: str | None = None,
    screenshot_jpeg_quality: int | None = None,
    capture_screenshots: bool = True,
    capture_audio: bool = True,
    audio_source: str = "auto",
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

    session = CaptureSession(
        output_dir,
        command=command,
        pid=pid,
        app_name=app_name,
        bundle_id=bundle_id,
        screenshot_interval=screenshot_interval,
        screenshot_format=screenshot_format,
        screenshot_resolution=screenshot_resolution,
        screenshot_jpeg_quality=screenshot_jpeg_quality,
        capture_screenshots=capture_screenshots,
        capture_audio=capture_audio,
        audio_source=audio_source,
        audio_chunk_seconds=audio_chunk_seconds,
        asr_backend=asr_backend,
        cwd=cwd,
    )
    summary = await anyio.to_thread.run_sync(session.start)
    with _lock:
        _sessions[session.id] = session
        _prune_locked()
    return summary


@mcp.tool()
async def capture_stop(session_id: str | None = None) -> dict:
    """Stop a capture and flush everything to disk.

    Args:
        session_id: The session to stop. If omitted and exactly one capture is
            running, that one is stopped; if several are running, an error lists
            them (pass an explicit id). Use ``capture_status`` to see ids.

    Returns the final session summary.
    """
    with _lock:
        running = [s for s in _sessions.values() if s.state == "running"]

    if session_id is None:
        if not running:
            return {"stopped": [], "note": "no running captures"}
        if len(running) > 1:
            raise ValueError(
                "multiple captures running; pass session_id. Running: "
                + ", ".join(s.id for s in running)
            )
        return await anyio.to_thread.run_sync(running[0].stop)

    with _lock:
        session = _sessions.get(session_id)
    if not session:
        raise ValueError(f"unknown session_id {session_id!r}")
    return await anyio.to_thread.run_sync(session.stop)


@mcp.tool()
async def capture_status(session_id: str | None = None) -> dict:
    """Report capture status.

    Args:
        session_id: If given, return that session's summary; otherwise return a
            list of all sessions this server has created.
    """
    with _lock:
        if session_id is not None:
            session = _sessions.get(session_id)
            if not session:
                raise ValueError(f"unknown session_id {session_id!r}")
            return session.summary()
        return {"sessions": [s.summary() for s in _sessions.values()]}


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
