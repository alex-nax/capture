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
_last_error: str | None = None


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


def active_device() -> str | None:
    """The explicit device for the active runtime (``cpu``/``cuda``), or None (remote / unset).
    Used to drive the engine's device — no auto-pick, no silent fallback."""
    r = _REGISTRY_BY_ID.get(active_runtime() or "")
    return r.get("device") if r else None


def set_last_error(msg: "str | None") -> None:
    global _last_error
    _last_error = msg


def last_error() -> "str | None":
    return _last_error


def _detect_nvidia() -> bool:
    """Cheap hint (no engine needed): is an NVIDIA GPU likely present? (nvidia-smi on PATH)."""
    import shutil

    return shutil.which("nvidia-smi") is not None


def pack_url(rid: str) -> "str | None":
    """The download URL for ``rid``'s pack: ``CAPTURE_ASR_PACK_URL_<RID>`` override, else composed
    from ``CAPTURE_ASR_PACK_BASE`` + the runtime id + the daemon's Python tag. None if unconfigured
    (hosting is set up by the release tooling)."""
    env = os.environ.get(f"CAPTURE_ASR_PACK_URL_{rid.replace('-', '_').upper()}")
    if env:
        return env
    base = os.environ.get("CAPTURE_ASR_PACK_BASE")
    if base:
        pytag = f"cp{sys.version_info.major}{sys.version_info.minor}-win_amd64"
        return f"{base.rstrip('/')}/runtime-{rid}-{pytag}.zip"
    return None


def _download(url: str, dest: str, on_progress=None) -> None:
    import urllib.request

    req = urllib.request.Request(url, headers={"User-Agent": "capture"})
    with urllib.request.urlopen(req) as r, open(dest, "wb") as f:
        total = int(r.headers.get("Content-Length") or 0)
        done = 0
        while True:
            chunk = r.read(262144)
            if not chunk:
                break
            f.write(chunk)
            done += len(chunk)
            if on_progress:
                on_progress(done, total, "")


def install(rid: str, source: "str | None" = None, on_progress=None) -> dict:
    """Install runtime ``rid`` from a pack. ``source`` may be an http(s) URL, a local ``.zip``, or a
    local directory (dev); else falls back to :func:`pack_url`. Extracts into ``runtime_dir(rid)``.
    Remote runtimes need no install. Does NOT change the active runtime (see :func:`set_active`)."""
    import shutil
    import tempfile
    import zipfile

    r = _REGISTRY_BY_ID.get(rid)
    if r is None:
        raise ValueError(f"unknown runtime {rid!r}")
    if r["kind"] != "local":
        return {"id": rid, "installed": True, "note": "remote runtime needs no install"}
    dest = runtime_dir(rid)
    src = source or pack_url(rid)
    if not src:
        raise RuntimeError(
            f"no pack source for {rid!r} — set CAPTURE_ASR_PACK_BASE / CAPTURE_ASR_PACK_URL_* or pass a source"
        )
    # local directory pack (dev): copy it in
    if Path(str(src)).is_dir():
        if dest.exists():
            shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(src, dest)
        log.info("installed runtime %s from dir %s", rid, src)
        return {"id": rid, "installed": True}
    tmp = None
    try:
        if str(src).lower().startswith(("http://", "https://")):
            fd, tmp = tempfile.mkstemp(suffix=".zip")
            os.close(fd)
            _download(str(src), tmp, on_progress)
            zpath = tmp
        elif Path(str(src)).is_file() and str(src).lower().endswith(".zip"):
            zpath = str(src)
        else:
            raise RuntimeError(f"unusable pack source: {src}")
        if dest.exists():
            shutil.rmtree(dest, ignore_errors=True)
        dest.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(zpath) as z:
            z.extractall(dest)
        log.info("installed runtime %s -> %s", rid, dest)
        return {"id": rid, "installed": True}
    finally:
        if tmp and os.path.exists(tmp):
            try:
                os.unlink(tmp)
            except OSError:
                pass


def set_active(rid: str) -> str:
    """Set ``rid`` as the active runtime (persisted) and load it into the running daemon. Switching
    between two already-loaded runtimes cleanly needs a daemon restart; none→first works in-process."""
    global _activated
    r = _REGISTRY_BY_ID.get(rid)
    if r is None:
        raise ValueError(f"unknown runtime {rid!r}")
    if r["kind"] == "local" and not is_installed(rid):
        raise ValueError(f"runtime {rid!r} is not installed")
    _config.set_(_CONFIG_KEY, rid)
    log.info("active ASR runtime set: %s", rid)
    _activated = None  # allow re-activation for a none→first install
    activate()
    return rid


def status_payload() -> dict:
    """Full payload for ``GET /v1/asr/runtimes``: each runtime + installed/active, plus a GPU hint."""
    active = active_runtime()
    runtimes = []
    for r in REGISTRY:
        runtimes.append(
            {
                **{k: r.get(k) for k in ("id", "label", "kind", "engine", "device", "requires")},
                "installed": is_installed(r["id"]),
                "active": r["id"] == active,
            }
        )
    return {"active": active, "gpu": {"nvidia": _detect_nvidia()}, "runtimes": runtimes}


def backend_report() -> dict:
    """``GET /v1/asr/backend``: the active runtime/engine/device, whether an engine is importable,
    and the last load error (so the GUI can show why ASR is off — never a silent fallback)."""
    import importlib.util

    rid = active_runtime()
    r = _REGISTRY_BY_ID.get(rid or "")
    available = (
        importlib.util.find_spec("faster_whisper") is not None
        or importlib.util.find_spec("mlx_whisper") is not None
    )
    return {
        "runtime": rid,
        "engine": r.get("engine") if r else None,
        "device": r.get("device") if r else None,
        "available": available,
        "error": _last_error,
    }
