"""Platform backend factory.

`current()` returns the `Platform` for the running OS (cached). The OS is chosen
by ``sys.platform``; set ``CAPTURE_PLATFORM=macos|windows`` to override (used by
tests and for forcing a backend). See ``docs/specs/platform-abstraction.md``.
"""

from __future__ import annotations

import os
import sys

from .base import (
    AudioSource,
    Platform,
    ScreenGrabber,
    WindowFinder,
    WindowRef,
    fit_box,
)

__all__ = [
    "AudioSource",
    "Platform",
    "ScreenGrabber",
    "WindowFinder",
    "WindowRef",
    "fit_box",
    "current",
]

_PLATFORM_BY_SYS = {"darwin": "macos", "win32": "windows"}

_cache: dict[str, Platform] = {}


def _resolve_name() -> str:
    name = os.environ.get("CAPTURE_PLATFORM", "auto").strip().lower()
    if name and name != "auto":
        return name
    return _PLATFORM_BY_SYS.get(sys.platform, sys.platform)


def current() -> Platform:
    """The cached `Platform` backend for this OS (or the ``CAPTURE_PLATFORM`` override)."""
    name = _resolve_name()
    plat = _cache.get(name)
    if plat is not None:
        return plat

    if name == "macos":
        from .macos import MacOSPlatform as _P
    elif name == "windows":
        from .windows import WindowsPlatform as _P
    else:
        raise RuntimeError(
            f"unsupported platform {name!r} (sys.platform={sys.platform!r}); "
            "set CAPTURE_PLATFORM=macos|windows to override"
        )

    plat = _P()
    _cache[name] = plat
    return plat
