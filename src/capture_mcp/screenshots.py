"""Periodic, timestamped window screenshots via the ``screencapture`` CLI.

Output format and resolution are configurable:
  * format     - png (default), jpg/jpeg, tiff, gif, bmp.
  * resolution - a bounding box "WxH" (e.g. "1280x720"); the shot is scaled to
                 fit within it preserving aspect ratio, never upscaled.

When neither a resolution nor a jpeg quality is requested, ``screencapture``
writes the target format directly (fast path). Otherwise the shot is captured to
a temporary PNG and post-processed with ``sips`` (a built-in macOS tool) to
resize and/or re-encode.
"""

from __future__ import annotations

import logging
import struct
import subprocess
import threading
from pathlib import Path

from . import windows
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


def _png_size(path: Path) -> tuple[int, int] | None:
    """Read pixel (width, height) from a PNG IHDR header without decoding it."""
    try:
        with open(path, "rb") as f:
            head = f.read(24)
        if len(head) < 24 or head[:8] != b"\x89PNG\r\n\x1a\n":
            return None
        w, h = struct.unpack(">II", head[16:24])
        return int(w), int(h)
    except OSError:
        return None


def _fit(sw: int, sh: int, bw: int, bh: int) -> tuple[int, int]:
    """Largest (w, h) fitting in box (bw, bh) preserving aspect, never upscaled."""
    scale = min(bw / sw, bh / sh, 1.0)
    return max(1, round(sw * scale)), max(1, round(sh * scale))


def _sc_format(fmt: str) -> str:
    return "jpg" if fmt in ("jpg", "jpeg") else fmt


def _sips_format(fmt: str) -> str:
    return "jpeg" if fmt in ("jpg", "jpeg") else fmt


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
        w = windows.primary_window(pid=self.pid, app_name=self.app_name)
        if w:
            self._last_wid = w.window_id
            return w.window_id
        # Window not on the current Space right now; keep using the last known id
        # (screencapture -l still grabs it) rather than falling back to whole-screen.
        return self._last_wid

    def _target_args(self, wid: int | None) -> list[str] | None:
        """screencapture target flags; None means 'skip this tick'."""
        if wid is not None:
            return ["-l", str(wid)]
        if self.pid is not None or self.app_name is not None:
            if not self.whole_screen_fallback:
                return None  # target requested but not on screen yet
        return []  # whole screen

    def _run_cmd(self, cmd: list[str], expected: Path) -> bool:
        """Run a capture/convert command and require ``expected`` to appear.

        ``screencapture`` exits 0 even when it can't write the file (e.g. a bad
        destination), so success is verified by the file actually existing. A
        timeout (bounded by the interval) keeps a hung tool from wedging the loop
        and blocking shutdown.
        """
        timeout = max(5.0, self.interval + 5.0)
        try:
            proc = subprocess.run(cmd, capture_output=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            self.errors += 1
            log.warning("%s timed out after %.1fs", cmd[0], timeout)
            return False
        if proc.returncode != 0 or not expected.exists():
            self.errors += 1
            stderr = proc.stderr.decode(errors="replace").strip()
            stdout = proc.stdout.decode(errors="replace").strip()
            log.warning(
                "%s failed (rc=%s, wrote=%s): %s",
                cmd[0], proc.returncode, expected.exists(), stderr or stdout,
            )
            return False
        return True

    def _capture_once(self) -> None:
        ts = now()
        wid = self._resolve_window_id()
        target = self._target_args(wid)
        if target is None:
            return

        ext = _extension(self.fmt)
        final = self.out_dir / f"{fs_stamp(ts)}.{ext}"
        needs_post = self.resolution is not None or (
            self.fmt in ("jpg", "jpeg") and self.jpeg_quality is not None
        )

        if not needs_post:
            cmd = ["screencapture", "-x", "-o", "-t", _sc_format(self.fmt), *target, str(final)]
            if self._run_cmd(cmd, final):
                self.count += 1
            return

        # Resize / re-encode path: capture to temp PNG, then sips. Note: the temp
        # name must NOT start with a dot — screencapture refuses hidden paths.
        tmp = self.out_dir / f"{fs_stamp(ts)}.tmp.png"
        cap = ["screencapture", "-x", "-o", "-t", "png", *target, str(tmp)]
        if not self._run_cmd(cap, tmp):
            tmp.unlink(missing_ok=True)
            return

        sips = ["sips"]
        if self.resolution is not None:
            size = _png_size(tmp)
            if size:
                tw, th = _fit(size[0], size[1], *self.resolution)
                sips += ["-z", str(th), str(tw)]  # sips takes height then width
        sips += ["-s", "format", _sips_format(self.fmt)]
        if self.fmt in ("jpg", "jpeg") and self.jpeg_quality is not None:
            sips += ["-s", "formatOptions", str(self.jpeg_quality)]
        sips += [str(tmp), "--out", str(final)]

        ok = self._run_cmd(sips, final)
        tmp.unlink(missing_ok=True)
        if ok:
            self.count += 1

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
