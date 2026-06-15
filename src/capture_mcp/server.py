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
from .core.session import CaptureSession
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

    kwargs = dict(
        command=command, pid=pid, app_name=app_name, bundle_id=bundle_id,
        screenshot_interval=screenshot_interval, screenshot_format=screenshot_format,
        screenshot_resolution=screenshot_resolution, screenshot_jpeg_quality=screenshot_jpeg_quality,
        capture_screenshots=capture_screenshots, capture_audio=capture_audio,
        audio_source=audio_source, audio_chunk_seconds=audio_chunk_seconds,
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


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
