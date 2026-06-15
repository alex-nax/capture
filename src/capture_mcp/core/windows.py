"""macOS window discovery via CoreGraphics (Quartz).

Maps a process (pid / app name) to its on-screen window ids so that
``screencapture -l <id>`` can grab just that window.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass
class WindowInfo:
    window_id: int
    owner_pid: int
    owner_name: str
    title: str
    width: int
    height: int
    layer: int

    @property
    def area(self) -> int:
        return self.width * self.height


def _list_windows(on_screen_only: bool = True) -> list[WindowInfo]:
    # Imported lazily so the module imports even where Quartz is unavailable.
    from Quartz import (
        CGWindowListCopyWindowInfo,
        kCGNullWindowID,
        kCGWindowListOptionAll,
        kCGWindowListOptionOnScreenOnly,
    )

    option = kCGWindowListOptionOnScreenOnly if on_screen_only else kCGWindowListOptionAll
    raw = CGWindowListCopyWindowInfo(option, kCGNullWindowID) or []
    out: list[WindowInfo] = []
    for w in raw:
        bounds = w.get("kCGWindowBounds", {})
        out.append(
            WindowInfo(
                window_id=int(w.get("kCGWindowNumber", 0)),
                owner_pid=int(w.get("kCGWindowOwnerPID", 0)),
                owner_name=str(w.get("kCGWindowOwnerName", "") or ""),
                title=str(w.get("kCGWindowName", "") or ""),
                width=int(bounds.get("Width", 0)),
                height=int(bounds.get("Height", 0)),
                layer=int(w.get("kCGWindowLayer", 0)),
            )
        )
    return out


def _match(wins: list[WindowInfo], pid: int | None, needle: str | None) -> list[WindowInfo]:
    matches = []
    for w in wins:
        if w.layer != 0 or w.width < 1 or w.height < 1:
            continue
        if pid is not None and w.owner_pid != pid:
            continue
        if needle is not None and needle not in w.owner_name.lower():
            continue
        matches.append(w)
    matches.sort(key=lambda w: w.area, reverse=True)
    return matches


def find_windows(pid: int | None = None, app_name: str | None = None) -> list[WindowInfo]:
    """Return windows for a pid or (case-insensitive substring) app name.

    Only normal-layer (layer 0) windows are considered, largest first. On-screen
    windows are preferred, but if none match we fall back to the full window list
    so we can still target an app whose window is on another Space/Desktop (or
    fullscreen) — ``screencapture -l`` captures it regardless of Space.
    """
    needle = app_name.lower() if app_name else None
    matches = _match(_list_windows(on_screen_only=True), pid, needle)
    if matches:
        return matches
    return _match(_list_windows(on_screen_only=False), pid, needle)


def primary_window(pid: int | None = None, app_name: str | None = None) -> WindowInfo | None:
    wins = find_windows(pid=pid, app_name=app_name)
    return wins[0] if wins else None
