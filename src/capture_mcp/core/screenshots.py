"""Periodic, timestamped window screenshots.

This scope owns the platform-neutral parts: the capture *schedule* (a fixed time
grid), window-target resolution with a cross-Space/`_last_wid` fallback, output
filenames, and success/error accounting. The OS-specific pixel capture (encode,
resize, window-vs-screen) is delegated to ``platform.current().screen_grabber``
(macOS: ``screencapture``/``sips``; Windows: GDI+); window discovery to
``platform.current().window_finder``. See ``docs/specs/screenshots.md``.

Output format and resolution are configurable:
  * format     - png (default), jpg/jpeg, tiff, gif, bmp.
  * resolution - a bounding box "WxH" (e.g. "1280x720"); the shot is scaled to
                 fit within it preserving aspect ratio, never upscaled.
"""

from __future__ import annotations

import logging
import threading
from pathlib import Path

from . import platform as _platform
from .util import fs_stamp, now

log = logging.getLogger(__name__)

VALID_FORMATS = ("png", "jpg", "jpeg", "tiff", "gif", "bmp")


def parse_resolution(spec: str | None) -> tuple[int, int, str | None] | None:
    """Parse "WxH" or "WxH/fmt" (e.g. "1280x720/jpg") -> (w, h, fmt_or_None).

    Returns None for an empty spec. Raises ValueError on malformed input.
    """
    if not spec:
        return None
    fmt = None
    s = spec.strip()
    if "/" in s:
        s, fmt = s.split("/", 1)
        fmt = fmt.strip().lower()
        if fmt not in VALID_FORMATS:
            raise ValueError(f"bad format in resolution spec: {fmt!r}; choose from {VALID_FORMATS}")
    s = s.strip().lower().replace("×", "x")
    parts = s.split("x")
    if len(parts) != 2:
        raise ValueError(f"bad resolution {spec!r}; expected WxH like 1280x720")
    try:
        w, h = int(parts[0]), int(parts[1])
    except ValueError:
        raise ValueError(f"bad resolution {spec!r}; width and height must be integers")
    if w < 1 or h < 1:
        raise ValueError(f"bad resolution {spec!r}; dimensions must be positive")
    return w, h, fmt


def _extension(fmt: str) -> str:
    return "jpg" if fmt in ("jpg", "jpeg") else fmt


class Screenshotter:
    """Grabs the target window every ``interval`` seconds into ``out_dir``.

    Re-resolves the window id each tick so it keeps working if the window is
    recreated. Falls back to whole-screen capture when no window can be resolved
    but a pid/app was requested.
    """

    def __init__(
        self,
        out_dir: Path,
        *,
        pid: int | None = None,
        app_name: str | None = None,
        interval: float = 1.0,
        fmt: str = "png",
        resolution: tuple[int, int] | None = None,
        jpeg_quality: int | None = None,
        whole_screen_fallback: bool = True,
        emit=None,
    ) -> None:
        fmt = (fmt or "png").lower()
        if fmt not in VALID_FORMATS:
            raise ValueError(f"bad screenshot format {fmt!r}; choose from {VALID_FORMATS}")

        self.out_dir = out_dir
        self.pid = pid
        self.app_name = app_name
        self.interval = max(0.05, float(interval))
        self.fmt = fmt
        self.resolution = resolution
        self.jpeg_quality = jpeg_quality
        self.whole_screen_fallback = whole_screen_fallback
        # Optional event hook (EventBus.publish-shaped); publishing never raises.
        self._emit = emit

        plat = _platform.current()
        self._finder = plat.window_finder
        self._grabber = plat.screen_grabber

        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._last_wid: int | None = None  # cache so we keep targeting a window that
        # temporarily leaves the on-screen list (e.g. a video player going fullscreen
        # onto its own Space) instead of falling back to whole-screen.
        self.count = 0
        self.errors = 0

    # -- lifecycle ------------------------------------------------------------

    def start(self) -> None:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        self._thread = threading.Thread(target=self._run, name="screenshotter", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread:
            self._thread.join(timeout=self.interval + 2.0)

    # -- capture --------------------------------------------------------------

    def _resolve_window_id(self) -> int | None:
        if self.pid is None and self.app_name is None:
            return None
        w = self._finder.primary(pid=self.pid, app_name=self.app_name)
        if w:
            self._last_wid = w.window_id
            return w.window_id
        # Window not on the current Space right now; keep using the last known id
        # (the grabber still targets it) rather than falling back to whole-screen.
        return self._last_wid

    def _capture_once(self) -> None:
        ts = now()
        wid = self._resolve_window_id()
        # Target was requested but no window id is available and fallback is off:
        # skip this tick (a deliberate skip, not an error).
        if wid is None and (self.pid is not None or self.app_name is not None) and not self.whole_screen_fallback:
            return

        ext = _extension(self.fmt)
        final = self.out_dir / f"{fs_stamp(ts)}.{ext}"
        timeout = max(5.0, self.interval + 5.0)
        ok = self._grabber.capture(
            wid,
            final,
            fmt=self.fmt,
            resolution=self.resolution,
            jpeg_quality=self.jpeg_quality,
            timeout=timeout,
        )
        if ok:
            self.count += 1
            if self._emit:
                self._emit("screenshot_taken", path=str(final), count=self.count)
        else:
            self.errors += 1
            if self._emit:
                self._emit("screenshot_error", errors=self.errors)

    # -- loop -----------------------------------------------------------------

    def _run(self) -> None:
        # Schedule on absolute ticks so screenshots land on the grid even if a
        # capture takes a non-trivial fraction of the interval.
        next_t = now()
        while not self._stop.is_set():
            try:
                self._capture_once()
            except Exception:  # never let the loop die
                self.errors += 1
                log.exception("screenshot tick failed")
            next_t += self.interval
            sleep_for = next_t - now()
            if sleep_for < 0:  # fell behind: skip missed ticks, fire next on-grid
                missed = int((-sleep_for) // self.interval) + 1
                next_t += missed * self.interval
                sleep_for = max(0.0, next_t - now())
            self._stop.wait(sleep_for)
