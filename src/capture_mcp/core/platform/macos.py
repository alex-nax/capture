"""macOS platform backend.

Wraps the existing, verified macOS capture paths behind the platform interfaces:
  * `MacWindowFinder`  — Quartz `CGWindowList` discovery (delegates to
    ``capture_mcp.windows``, unchanged).
  * `MacScreenGrabber` — ``screencapture`` (+ ``sips`` for resize/convert).
  * `MacAudioSource`   — the ScreenCaptureKit ``audiocap`` helper (per-app audio),
    else ``ffmpeg`` ``avfoundation`` microphone capture.

The behavior here is the macOS behavior that shipped before the abstraction; the
loop/scheduling/chunking logic lives platform-neutrally in ``screenshots.py`` /
``audio.py``. See ``docs/specs/{screenshots,windows,audio}.md``.
"""

from __future__ import annotations

import logging
import shutil
import struct
import subprocess
from pathlib import Path

from .. import windows as _quartz
from .base import AudioSource, Platform, ScreenGrabber, WindowFinder, WindowRef, fit_box

log = logging.getLogger(__name__)

# Repo root holding the compiled Swift helper: src/capture_mcp/core/platform/macos.py
# -> parents[0]=platform, [1]=core, [2]=capture_mcp, [3]=src, [4]=repo root.
# (The M0a split (#25) moved this module one level deeper into core/, so the
# walk-up gained one step — a too-short walk silently disables per-app audio.)
_HELPER = Path(__file__).resolve().parents[4] / "helper" / "audiocap"


def helper_path() -> Path | None:
    """Path to the built ScreenCaptureKit helper, if present."""
    return _HELPER if _HELPER.exists() else None


def _sc_format(fmt: str) -> str:
    return "jpg" if fmt in ("jpg", "jpeg") else fmt


def _sips_format(fmt: str) -> str:
    return "jpeg" if fmt in ("jpg", "jpeg") else fmt


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


class MacWindowFinder(WindowFinder):
    def find(self, pid: int | None = None, app_name: str | None = None) -> list[WindowRef]:
        return [
            WindowRef(
                window_id=w.window_id,
                pid=w.owner_pid,
                app_name=w.owner_name,
                title=w.title,
                width=w.width,
                height=w.height,
            )
            for w in _quartz.find_windows(pid=pid, app_name=app_name)
        ]


class MacScreenGrabber(ScreenGrabber):
    def capture(
        self,
        window_id: int | None,
        out_path: Path,
        *,
        fmt: str,
        resolution: tuple[int, int] | None = None,
        jpeg_quality: int | None = None,
        timeout: float | None = None,
    ) -> bool:
        timeout = timeout if timeout is not None else 10.0
        target = ["-l", str(window_id)] if window_id is not None else []
        needs_post = resolution is not None or (
            fmt in ("jpg", "jpeg") and jpeg_quality is not None
        )

        if not needs_post:
            cmd = ["screencapture", "-x", "-o", "-t", _sc_format(fmt), *target, str(out_path)]
            return self._run(cmd, out_path, timeout)

        # Resize / re-encode path: capture to a temp PNG, then sips. The temp name
        # must NOT start with a dot — screencapture refuses hidden destination paths.
        tmp = out_path.with_name(out_path.stem + ".tmp.png")
        cap = ["screencapture", "-x", "-o", "-t", "png", *target, str(tmp)]
        if not self._run(cap, tmp, timeout):
            tmp.unlink(missing_ok=True)
            return False

        sips = ["sips"]
        if resolution is not None:
            size = _png_size(tmp)
            if size:
                tw, th = fit_box(size[0], size[1], *resolution)
                sips += ["-z", str(th), str(tw)]  # sips takes height then width
        sips += ["-s", "format", _sips_format(fmt)]
        if fmt in ("jpg", "jpeg") and jpeg_quality is not None:
            sips += ["-s", "formatOptions", str(jpeg_quality)]
        sips += [str(tmp), "--out", str(out_path)]

        ok = self._run(sips, out_path, timeout)
        tmp.unlink(missing_ok=True)
        return ok

    def _run(self, cmd: list[str], expected: Path, timeout: float) -> bool:
        """Run a capture/convert command; require ``expected`` to appear.

        ``screencapture`` exits 0 even when it can't write the file, so success is
        verified by the file actually existing. A timeout keeps a hung tool from
        wedging the screenshot loop / blocking shutdown.
        """
        try:
            proc = subprocess.run(cmd, capture_output=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            log.warning("%s timed out after %.1fs", cmd[0], timeout)
            return False
        if proc.returncode != 0 or not expected.exists():
            stderr = proc.stderr.decode(errors="replace").strip()
            stdout = proc.stdout.decode(errors="replace").strip()
            log.warning(
                "%s failed (rc=%s, wrote=%s): %s",
                cmd[0], proc.returncode, expected.exists(), stderr or stdout,
            )
            return False
        return True


class MacAudioSource(AudioSource):
    def command(
        self,
        *,
        pid: int | None,
        bundle_id: str | None,
        source: str,
        rate: int,
    ) -> tuple[list[str], str] | None:
        want_app = source in ("auto", "app")
        hp = helper_path()
        if want_app and hp and (pid is not None or bundle_id is not None):
            cmd = [str(hp), "--rate", str(rate)]
            if pid is not None:
                cmd += ["--pid", str(pid)]
            elif bundle_id is not None:
                cmd += ["--bundle", str(bundle_id)]
            return cmd, "app"

        if source == "app":
            return None  # explicitly wanted app audio but cannot satisfy it

        # Microphone fallback via ffmpeg avfoundation.
        if shutil.which("ffmpeg"):
            cmd = [
                "ffmpeg", "-hide_banner", "-loglevel", "warning",
                "-f", "avfoundation", "-i", ":default",
                "-ac", "1", "-ar", str(rate),
                "-f", "s16le", "-",
            ]
            return cmd, "mic"
        return None


class MacOSPlatform(Platform):
    name = "macos"

    def __init__(self) -> None:
        super().__init__(MacWindowFinder(), MacScreenGrabber(), MacAudioSource())
