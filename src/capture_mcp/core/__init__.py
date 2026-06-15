"""capture_mcp.core: the capture engine, independent of any frontend.

Everything in this package is callable without the MCP layer: `CaptureSession`
orchestrates one capture; `SessionRegistry` tracks sessions in-process and
rebuilds finished-session history from disk. The MCP server (`capture_mcp.server`)
is one thin client of this package; future frontends (daemon, CLI, GUI) are
expected to consume the same surface (see docs/specs/product-architecture.md).
"""

from .registry import MAX_SESSIONS, SessionRegistry
from .session import CaptureSession

__all__ = ["CaptureSession", "SessionRegistry", "MAX_SESSIONS", "list_windows"]


def list_windows(pid: int | None = None, app_name: str | None = None) -> list[dict]:
    """On-screen top-level windows as JSON-ready dicts, largest area first.

    The shared window picker for every frontend (MCP `list_windows` tool today;
    the daemon's `/v1/windows` and the GUI later). Filters mirror
    `WindowFinder.find`: by pid, by case-insensitive app-name substring, or
    neither (all normal-layer windows).
    """
    from . import platform as _platform

    return [
        {
            "window_id": w.window_id,
            "pid": w.pid,
            "app_name": w.app_name,
            "title": w.title,
            "width": w.width,
            "height": w.height,
        }
        for w in _platform.current().window_finder.find(pid=pid, app_name=app_name)
    ]
