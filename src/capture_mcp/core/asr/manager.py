"""Whisper model manager — list / download / select the active model.

Backs the daemon's ``/v1/asr/*`` routes and the GUI's model picker. The catalog is
**runtime-aware** (:func:`catalog`): ``mlx-community/whisper-*`` repos on Apple Silicon
(mlx-whisper), ``Systran/faster-whisper-*`` repos on Windows/Linux (faster-whisper / CTranslate2).
Weights live in the shared HF cache (``~/.cache/huggingface`` / ``%USERPROFILE%\\.cache``) and
download on demand (never bundled). The active model is persisted via
:mod:`capture_mcp.core.config` so a model chosen in the GUI applies to new captures everywhere.

Both runtimes are optional deps; :func:`runtime_available` reports "ASR runtime missing" (instead
of failing) when neither is importable — e.g. a lean / screenshots-only build.
"""

from __future__ import annotations

import logging
import threading
from pathlib import Path

from .. import config as _config

log = logging.getLogger(__name__)

#: Curated catalogs — the repos offered in the GUI, ordered by download size. Weights live in the
#: shared HF cache (downloaded on demand, NOT bundled). There are two, one per local runtime:
#:  * `mlx-community/whisper-*` — Apple-Silicon mlx-whisper (macOS). Naming is inconsistent
#:    (`whisper-tiny` but `whisper-base-mlx`; `whisper-base`/`whisper-small`/`whisper-large-v3`
#:    do NOT exist) — IDs VERIFIED against HuggingFace.
#:  * `Systran/faster-whisper-*` — CTranslate2 (faster-whisper) for Windows/Linux, CPU or CUDA.
#: `catalog()` picks the one matching the runtime this daemon actually bundles, so the GUI only
#: ever offers models the daemon can load.
_MLX_CATALOG: list[dict] = [
    {"repo": "mlx-community/whisper-tiny", "name": "Whisper Tiny", "size_label": "~74 MB"},
    {"repo": "mlx-community/whisper-base-mlx", "name": "Whisper Base", "size_label": "~144 MB"},
    {
        "repo": "mlx-community/whisper-large-v3-turbo-q4",
        "name": "Whisper Large v3 Turbo (4-bit)",
        "size_label": "~464 MB",
    },
    {"repo": "mlx-community/whisper-small-mlx", "name": "Whisper Small", "size_label": "~481 MB"},
    {"repo": "mlx-community/whisper-medium-mlx", "name": "Whisper Medium", "size_label": "~1.5 GB"},
    {
        "repo": "mlx-community/whisper-large-v3-turbo",
        "name": "Whisper Large v3 Turbo",
        "size_label": "~1.6 GB",
    },
]
_FASTER_CATALOG: list[dict] = [
    {"repo": "Systran/faster-whisper-tiny", "name": "Whisper Tiny (faster-whisper)", "size_label": "~75 MB"},
    {"repo": "Systran/faster-whisper-base", "name": "Whisper Base (faster-whisper)", "size_label": "~145 MB"},
    {"repo": "Systran/faster-whisper-small", "name": "Whisper Small (faster-whisper)", "size_label": "~484 MB"},
    {"repo": "Systran/faster-whisper-medium", "name": "Whisper Medium (faster-whisper)", "size_label": "~1.5 GB"},
    {"repo": "Systran/faster-whisper-large-v3", "name": "Whisper Large v3 (faster-whisper)", "size_label": "~3.1 GB"},
]

_MLX_DEFAULT_REPO = "mlx-community/whisper-large-v3-turbo"
_FASTER_DEFAULT_REPO = "Systran/faster-whisper-base"  # small + CPU-friendly default on Windows

_CONFIG_KEY = "whisper_model"
_LANG_KEY = "whisper_language"
_CHUNK_KEY = "audio_chunk_seconds"


def _mlx_available() -> bool:
    import importlib.util

    return importlib.util.find_spec("mlx_whisper") is not None


def _faster_available() -> bool:
    import importlib.util

    return importlib.util.find_spec("faster_whisper") is not None


def _use_faster() -> bool:
    """Use the faster-whisper catalog when mlx is absent but faster-whisper is present (the
    Windows/CUDA build). On Apple Silicon (mlx present) keep the mlx catalog."""
    return _faster_available() and not _mlx_available()


def catalog() -> list[dict]:
    """The model catalog for the runtime this daemon actually bundles."""
    return _FASTER_CATALOG if _use_faster() else _MLX_CATALOG


def default_repo() -> str:
    """The default active model for the bundled runtime."""
    return _FASTER_DEFAULT_REPO if _use_faster() else _MLX_DEFAULT_REPO


def _known_repos() -> set[str]:
    return {m["repo"] for m in catalog()}

#: Default transcription chunk length. Whisper is trained on 30 s windows; shorter
#: chunks (the old 8 s) make it hallucinate phantom phrases ("Thank you.") on pauses /
#: non-English audio, so 30 s is the reliable default. Tunable via the setting below.
DEFAULT_CHUNK_SECONDS = 30.0
CHUNK_BOUNDS = (1.0, 120.0)


def runtime_available() -> bool:
    """True iff a local Whisper runtime can run here — mlx-whisper (Apple Silicon) OR
    faster-whisper (CTranslate2; Windows/Linux, CPU or CUDA). False only on a lean build with
    neither (so the GUI reports "ASR runtime missing" instead of offering models that can't load)."""
    return _mlx_available() or _faster_available()


#: A repo is "downloaded" once config.json + ANY weight file is cached. mlx-community repos ship
#: ``weights.npz`` (most) or ``weights.safetensors`` (full large-v3-turbo); faster-whisper /
#: CTranslate2 repos ship ``model.bin``.
_WEIGHT_FILES = ("weights.npz", "weights.safetensors", "model.bin")


def is_downloaded(repo: str) -> bool:
    """True iff ``repo``'s weights are already in the HF cache (no network)."""
    try:
        from huggingface_hub import try_to_load_from_cache
    except Exception:
        return False
    if not isinstance(try_to_load_from_cache(repo, "config.json"), str):
        return False
    return any(isinstance(try_to_load_from_cache(repo, w), str) for w in _WEIGHT_FILES)


def active_model() -> str:
    """The configured active model, validated against the bundled runtime's catalog (config →
    default). A stale cross-platform value (e.g. an mlx repo synced onto a Windows box, which
    CTranslate2 can't load) is ignored in favour of the default, so the backend never gets a
    model it can't open. Env still wins at load time."""
    val = _config.get(_CONFIG_KEY)
    if isinstance(val, str) and val.strip() in _known_repos():
        return val.strip()
    return default_repo()


def set_active_model(repo: str) -> str:
    """Persist ``repo`` as the active model. Raises ValueError if not in the (runtime) catalog."""
    known = _known_repos()
    if repo not in known:
        raise ValueError(f"unknown model {repo!r}; choose from {sorted(known)}")
    _config.set_(_CONFIG_KEY, repo)
    log.info("active whisper model set: %s", repo)
    return repo


def active_language() -> str | None:
    """The configured transcription language (ISO code like ``ru``/``en``), or ``None``
    for auto-detect. Pinning the language stops Whisper mis-detecting a short chunk as
    English and hallucinating — but it's the user's choice (a persisted setting)."""
    val = _config.get(_LANG_KEY)
    return val.strip() if isinstance(val, str) and val.strip() else None


def set_active_language(language: str | None) -> str | None:
    """Persist the transcription language. ``None``/``""``/``"auto"`` = auto-detect.
    Accepts a short ISO-639 code (loosely validated: 2–5 letters)."""
    lang = (language or "").strip().lower()
    if lang in ("", "auto"):
        _config.set_(_LANG_KEY, "")
        log.info("transcription language set: auto")
        return None
    if not (2 <= len(lang) <= 5 and lang.isalpha()):
        raise ValueError(f"invalid language {language!r}; use an ISO code like 'ru', 'en' (or 'auto')")
    _config.set_(_LANG_KEY, lang)
    log.info("transcription language set: %s", lang)
    return lang


def active_chunk_seconds() -> float:
    """The configured transcription chunk length (seconds); default 30 s."""
    val = _config.get(_CHUNK_KEY)
    try:
        secs = float(val)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return DEFAULT_CHUNK_SECONDS
    lo, hi = CHUNK_BOUNDS
    return min(hi, max(lo, secs))


def set_chunk_seconds(seconds: float) -> float:
    """Persist the transcription chunk length (clamped to ``CHUNK_BOUNDS``)."""
    try:
        secs = float(seconds)
    except (TypeError, ValueError):
        raise ValueError(f"invalid chunk length {seconds!r}; give seconds (e.g. 30)")
    lo, hi = CHUNK_BOUNDS
    secs = min(hi, max(lo, secs))
    _config.set_(_CHUNK_KEY, secs)
    log.info("transcription chunk length set: %.0fs", secs)
    return secs


def catalog_status(downloading: object = ()) -> dict:
    """The full payload for ``GET /v1/asr/models``.

    ``downloading`` is the set of repos currently being fetched (the daemon's
    in-flight set) so a fresh poll reflects an in-progress download too.
    """
    active = active_model()
    dl = set(downloading)
    return {
        "backend_available": runtime_available(),
        "active": active,
        "language": active_language(),
        "chunk_seconds": active_chunk_seconds(),
        "models": [
            {
                **m,
                "downloaded": is_downloaded(m["repo"]),
                "active": m["repo"] == active,
                "downloading": m["repo"] in dl,
            }
            for m in catalog()
        ],
    }


def _repo_cache_dir(repo: str) -> Path:
    """The HF cache directory for ``repo`` (may not exist yet)."""
    from huggingface_hub import constants

    return Path(constants.HF_HUB_CACHE) / ("models--" + repo.replace("/", "--"))


def _repo_cache_bytes(repo: str) -> int:
    """Bytes currently on disk for ``repo`` (incl. in-progress ``.incomplete`` blobs)."""
    d = _repo_cache_dir(repo)
    if not d.exists():
        return 0
    total = 0
    for f in d.rglob("*"):
        try:
            if f.is_file() and not f.is_symlink():  # blobs are real files; snapshots symlink them
                total += f.stat().st_size
        except OSError:
            pass
    return total


def _repo_total_bytes(repo: str) -> int:
    """Total download size for ``repo`` from the Hub (0 if offline/unknown)."""
    try:
        from huggingface_hub import HfApi

        info = HfApi().model_info(repo, files_metadata=True)
        return sum(int(s.size or 0) for s in info.siblings or [])
    except Exception:
        return 0


def download(repo: str, on_progress=None) -> str:
    """Download ``repo``'s weights into the HF cache (blocking). Returns the repo.

    Validates against the catalog (no arbitrary repo fetches). ``on_progress`` is
    ``(downloaded_bytes, total_bytes, filename)``; safe to omit.

    Progress is measured by polling the repo's on-disk cache size against the Hub's
    reported total — backend-agnostic, since hf_hub's accelerated (xet/hf_transfer)
    download paths bypass the Python ``tqdm`` progress hook.
    """
    if repo not in _known_repos():
        raise ValueError(f"unknown model {repo!r}; choose from {sorted(_known_repos())}")
    from huggingface_hub import constants, snapshot_download

    # Force the plain HTTP backend. The xet backend streams content-addressed
    # chunks into a *separate* cache and only materializes the final blob at the
    # very end, so the on-disk byte poll below would read ~0 % until it suddenly
    # jumps to 100 % — i.e. no visible progress. The plain backend instead grows a
    # `<blob>.incomplete` file inside the repo dir that the poll can measure. The
    # constant is read live at download time (file_download.py), so setting it here
    # takes effect for this call regardless of import order.
    constants.HF_HUB_DISABLE_XET = True

    total = _repo_total_bytes(repo) if on_progress is not None else 0
    stop = threading.Event()
    if on_progress is not None and total > 0:

        def _poll() -> None:
            while not stop.wait(0.5):
                on_progress(min(_repo_cache_bytes(repo), total), total, "")

        threading.Thread(target=_poll, name=f"asr-dl-poll-{repo}", daemon=True).start()

    try:
        snapshot_download(repo)
    finally:
        stop.set()
    if on_progress is not None and total > 0:
        on_progress(total, total, "")  # final 100%
    log.info("downloaded whisper model: %s", repo)
    return repo


def delete(repo: str) -> dict:
    """Remove ``repo``'s weights from the HF cache. Returns ``{repo, freed_bytes}``.

    Validates against the catalog (no arbitrary path deletes). Deleting the *active*
    model is allowed — its status simply reverts to "active · needs download" until
    it is re-fetched, which the catalog reports on the next poll.
    """
    if repo not in _known_repos():
        raise ValueError(f"unknown model {repo!r}; choose from {sorted(_known_repos())}")
    import shutil

    freed = _repo_cache_bytes(repo)
    d = _repo_cache_dir(repo)
    if d.exists():
        shutil.rmtree(d, ignore_errors=True)
    log.info("deleted whisper model: %s (%d bytes freed)", repo, freed)
    return {"repo": repo, "deleted": True, "freed_bytes": freed}
