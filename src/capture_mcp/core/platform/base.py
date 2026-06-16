"""Platform abstraction interfaces.

capture-mcp captures three OS-specific things — window discovery, screenshots,
and per-app audio — behind these interfaces so that everything *above* them
(session orchestration, screenshot scheduling, audio chunking, ASR, transcripts,
the MCP tool surface, and the on-disk session layout) is shared, unchanged,
across macOS and Windows.

`platform.current()` (see ``__init__.py``) returns the concrete `Platform` for
the running OS. Only the concrete backends below the interface know about
``screencapture``/Quartz/ScreenCaptureKit (macOS) or GDI+/``EnumWindows``
(Windows). See ``docs/specs/platform-abstraction.md`` for the contract.
"""

from __future__ import annotations

import abc
from dataclasses import dataclass
from pathlib import Path


@dataclass
class WindowRef:
    """A platform-neutral handle to one on-screen window.

    ``window_id`` is the OS id that the `ScreenGrabber` targets: a ``CGWindowID``
    on macOS (``screencapture -l <id>``) or an ``HWND`` on Windows (GDI capture).
    The remaining fields let `session` resolve a pid/title from an app name.
    """

    window_id: int
    pid: int
    app_name: str
    title: str
    width: int
    height: int

    @property
    def area(self) -> int:
        return self.width * self.height


def fit_box(sw: int, sh: int, bw: int, bh: int) -> tuple[int, int]:
    """Largest (w, h) fitting inside box (bw, bh), preserving aspect, never upscaled."""
    scale = min(bw / sw, bh / sh, 1.0)
    return max(1, round(sw * scale)), max(1, round(sh * scale))


class WindowFinder(abc.ABC):
    """Maps a process (pid) or app-name substring to its on-screen window(s)."""

    @abc.abstractmethod
    def find(self, pid: int | None = None, app_name: str | None = None) -> list[WindowRef]:
        """Matching top-level windows, largest area first (possibly empty)."""

    def primary(self, pid: int | None = None, app_name: str | None = None) -> WindowRef | None:
        """The largest matching window, or ``None`` if there is no match."""
        wins = self.find(pid=pid, app_name=app_name)
        return wins[0] if wins else None


class ScreenGrabber(abc.ABC):
    """Captures a single screenshot of a window (or the whole screen)."""

    @abc.abstractmethod
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
        """Write one screenshot to ``out_path``.

        ``window_id is None`` captures the whole (primary) screen; otherwise the
        identified window. The image is encoded as ``fmt`` and, when ``resolution``
        is given, scaled to fit inside it preserving aspect ratio and never
        upscaled (see `fit_box`). ``jpeg_quality`` applies only to jpg/jpeg.
        Returns ``True`` iff ``out_path`` was actually written (callers count
        ``False`` as an error; the grabber logs the reason). ``timeout`` bounds any
        subprocess the backend shells out to (ignored by in-process backends).
        """


class AudioSource(abc.ABC):
    """Builds the command for a process that emits the audio PCM contract."""

    @abc.abstractmethod
    def command(
        self,
        *,
        pid: int | None,
        bundle_id: str | None,
        source: str,
        rate: int,
        mic_device: str | None = None,
    ) -> tuple[list[str], str] | None:
        """``(argv, mode)`` for a process whose **stdout is 16 kHz mono signed-16-bit
        little-endian PCM**, or ``None`` if no source can satisfy this request.

        ``source`` is ``"auto"`` | ``"app"`` | ``"mic"``; ``mode`` in the return is
        the kind actually selected (``"app"`` | ``"mic"``). ``mic_device`` is an
        optional input-device id (from :meth:`list_input_devices`) used when
        ``source == "mic"``; ``None`` means the system default input.
        """

    def list_input_devices(self) -> list[dict]:
        """Available microphone/input devices as ``[{id, name, default}]`` (possibly
        empty). Default impl returns ``[]``; backends with a mic source override it."""
        return []


class Platform:
    """Aggregate of the three backends for one OS. Backends are stateless/thread-safe
    and shared (the factory caches one `Platform` per resolved backend name)."""

    name: str = "base"

    def __init__(
        self,
        window_finder: WindowFinder,
        screen_grabber: ScreenGrabber,
        audio_source: AudioSource,
    ) -> None:
        self.window_finder = window_finder
        self.screen_grabber = screen_grabber
        self.audio_source = audio_source
