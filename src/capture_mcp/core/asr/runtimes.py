"""ASR runtime packs — install a chosen speech-recognition engine *after* the app is installed.

The packaged Windows daemon is frozen **lean** (no ASR engine bundled). The user picks a runtime
matching their hardware in the GUI; the app downloads a prebuilt **pack** (the engine's wheels,
built for the frozen daemon's Python ABI) into the runtime dir, and :func:`activate` puts that dir
on ``sys.path`` + the DLL search path so the **frozen** daemon can import it. No active runtime =
transcription is off (reported, never silently degraded). See docs/specs/asr-runtimes.md.

Registry note: the GPU story is designed to offer several engines ("option 3 track"); only NVIDIA
(CUDA) + CPU + remote are implemented now, with AMD/Intel (whisper.cpp Vulkan / ONNX DirectML)
deferred — they slot in as new REGISTRY entries + packs without changing this mechanism.
"""

from __future__ import annotations

import logging
import os
import sys
from pathlib import Path

from .. import config as _config

log = logging.getLogger(__name__)

#: config key holding the active runtime id (set by the GUI when a runtime is installed/selected).
_CONFIG_KEY = "asr_runtime"

#: The selectable runtimes. ``pip`` lists the pack contents (built per Python ABI by the release
#: tooling). ``device`` is the explicit CTranslate2 device for local faster-whisper runtimes.
REGISTRY: list[dict] = [
    {
        "id": "faster-cpu",
        "label": "CPU — faster-whisper",
        "kind": "local",
        "engine": "faster-whisper",
        "device": "cpu",
        "requires": "Any x64 CPU. Works everywhere; slower than a GPU.",
        "pip": ["faster-whisper"],
    },
    {
        "id": "faster-cuda",
        "label": "NVIDIA GPU — faster-whisper (CUDA)",
        "kind": "local",
        "engine": "faster-whisper",
        "device": "cuda",
        "requires": "NVIDIA GPU + recent driver. Fast.",
        "pip": ["faster-whisper", "nvidia-cublas-cu12", "nvidia-cudnn-cu12"],
    },
    {
        "id": "remote",
        "label": "Remote (OpenAI-compatible / Riva)",
        "kind": "remote",
        "engine": "openai-compat",
        "device": None,
        "requires": "A reachable endpoint; no local install. Configure the URL in Settings.",
        "pip": [],
    },
    # Deferred (registry slots, not yet installable): AMD/Intel GPU via whisper.cpp (Vulkan) and/or
    # ONNX Runtime (DirectML). Added later as new entries + packs; no mechanism change.
]

_REGISTRY_BY_ID = {r["id"]: r for r in REGISTRY}
_activated: str | None = None


def base_dir() -> Path:
    """Where runtime packs are installed (per-user, writable, outside the read-only install)."""
    if sys.platform == "win32":
        root = os.environ.get("LOCALAPPDATA") or str(Path.home())
        return Path(root) / "Capture" / "runtimes"
    return Path.home() / ".capture" / "runtimes"


def runtime_dir(rid: str) -> Path:
    return base_dir() / rid


def get(rid: str) -> dict | None:
    return _REGISTRY_BY_ID.get(rid)


def active_runtime() -> str | None:
    """The configured active runtime id, or None (no runtime chosen → ASR off)."""
    v = _config.get(_CONFIG_KEY)
    return v.strip() if isinstance(v, str) and v.strip() else None


def is_installed(rid: str) -> bool:
    """A local runtime is installed once its pack dir exists and is non-empty; remote is always 'installed'."""
    r = _REGISTRY_BY_ID.get(rid)
    if r is None:
        return False
    if r["kind"] == "remote":
        return True
    d = runtime_dir(rid)
    return d.is_dir() and any(d.iterdir())


def _add_dll_dirs(root: Path) -> None:
    """Add ``root`` and any subdir containing a DLL (ctranslate2 ships its DLLs in-package; nvidia
    libs under ``nvidia/*/bin``) to the Windows DLL search path so the engine's C-extensions load."""
    if sys.platform != "win32":
        return
    dirs: set[str] = {str(root)}
    try:
        for dll in root.rglob("*.dll"):
            dirs.add(str(dll.parent))
    except OSError:
        pass
    for d in dirs:
        if Path(d).is_dir():
            try:
                os.add_dll_directory(d)
            except OSError:
                pass
    os.environ["PATH"] = os.pathsep.join(dirs) + os.pathsep + os.environ.get("PATH", "")


def activate() -> str | None:
    """Put the active runtime's pack on ``sys.path`` + the DLL search path so the frozen daemon can
    import its engine. Idempotent. ``CAPTURE_ASR_RUNTIME_DIR`` overrides the dir (dev/test/spike).
    Returns the activated runtime id (or ``"override"``), else None."""
    global _activated
    if _activated is not None:
        return _activated
    override = os.environ.get("CAPTURE_ASR_RUNTIME_DIR")
    if override and Path(override).is_dir():
        d = Path(override)
        sys.path.insert(0, str(d))
        _add_dll_dirs(d)
        _activated = "override"
        log.info("ASR runtime activated (override): %s", d)
        return _activated
    rid = active_runtime()
    if not rid:
        return None
    r = _REGISTRY_BY_ID.get(rid)
    if r is None or r["kind"] == "remote":
        _activated = rid  # remote needs no path injection
        return rid
    d = runtime_dir(rid)
    if not d.is_dir():
        log.warning("active ASR runtime %r is not installed at %s", rid, d)
        return None
    sys.path.insert(0, str(d))
    _add_dll_dirs(d)
    _activated = rid
    log.info("ASR runtime activated: %s (%s)", rid, d)
    return rid
