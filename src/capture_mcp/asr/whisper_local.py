"""Local Whisper ASR backends that actually run on this Apple Silicon Mac.

Two implementations, tried in order by :func:`load`:
  * ``mlx-whisper``    - Apple-Silicon-native (Metal), fastest here.
  * ``faster-whisper`` - CTranslate2, CPU/CUDA, cross-platform fallback.

Both download their model weights on first use, so the first transcription
needs network access. If neither package is installed, :func:`load` raises and
the session continues without transcription (logged as a warning).
"""

from __future__ import annotations

import logging
import os
import tempfile
import wave
from pathlib import Path

import numpy as np

from .base import ASRBackend, Segment

log = logging.getLogger(__name__)

# Override the model per backend, e.g. CAPTURE_WHISPER_MODEL=mlx-community/whisper-tiny
# or CAPTURE_WHISPER_MODEL=small for faster-whisper. First use downloads weights.
_MLX_DEFAULT = "mlx-community/whisper-large-v3-turbo"
_FW_DEFAULT = "base"


def _write_wav(pcm: np.ndarray, sample_rate: int) -> str:
    fd, name = tempfile.mkstemp(suffix=".wav", prefix="capmcp-")
    os.close(fd)  # we reopen by path via the wave module; don't leak the fd
    path = Path(name)
    pcm16 = np.clip(pcm, -1.0, 1.0)
    pcm16 = (pcm16 * 32767.0).astype("<i2")
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sample_rate)
        w.writeframes(pcm16.tobytes())
    return str(path)


class MlxWhisper(ASRBackend):
    name = "mlx-whisper"

    def __init__(self, model: str | None = None) -> None:
        import mlx_whisper  # noqa: F401  (validate availability early)

        self._model = model or os.environ.get("CAPTURE_WHISPER_MODEL", _MLX_DEFAULT)

    def transcribe(self, pcm: np.ndarray, sample_rate: int) -> list[Segment]:
        import mlx_whisper

        result = mlx_whisper.transcribe(
            pcm.astype(np.float32),
            path_or_hf_repo=self._model,
            word_timestamps=False,
        )
        return [
            Segment(start=float(s["start"]), end=float(s["end"]), text=s["text"].strip())
            for s in result.get("segments", [])
            if s.get("text", "").strip()
        ]


def _add_nvidia_dll_dirs() -> None:
    """On Windows, let CTranslate2 find cuBLAS/cuDNN from the nvidia-*-cu12 pip
    packages (DLLs live under site-packages/nvidia/<lib>/bin). CTranslate2's loader
    searches PATH, so we both add the dirs and prepend them to PATH."""
    if os.name != "nt":
        return
    try:
        import importlib.util
        import sys

        roots: list[str] = []
        spec = importlib.util.find_spec("nvidia")
        if spec and spec.submodule_search_locations:
            roots.extend(spec.submodule_search_locations)
        if not roots:  # fallback: scan sys.path for a nvidia/ dir
            for p in sys.path:
                cand = Path(p) / "nvidia"
                if cand.is_dir():
                    roots.append(str(cand))

        bins: list[str] = []
        for root in roots:
            for sub in Path(root).iterdir():
                binp = sub / "bin"
                if binp.is_dir():
                    bins.append(str(binp))
        for binp in bins:
            try:
                os.add_dll_directory(binp)
            except OSError:
                pass
        if bins:
            os.environ["PATH"] = os.pathsep.join(bins) + os.pathsep + os.environ.get("PATH", "")
            log.debug("added nvidia DLL dirs: %s", bins)
    except Exception:
        log.debug("could not add nvidia DLL dirs", exc_info=True)


def _auto_device() -> str:
    """'cuda' if CTranslate2 sees a CUDA device, else 'cpu'."""
    try:
        import ctranslate2

        if ctranslate2.get_cuda_device_count() > 0:
            return "cuda"
    except Exception:
        log.debug("ctranslate2 CUDA probe failed", exc_info=True)
    return "cpu"


class FasterWhisper(ASRBackend):
    name = "faster-whisper"

    def __init__(
        self,
        model: str | None = None,
        device: str | None = None,
        compute_type: str | None = None,
    ) -> None:
        _add_nvidia_dll_dirs()
        from faster_whisper import WhisperModel

        model = model or os.environ.get("CAPTURE_WHISPER_MODEL", _FW_DEFAULT)
        device = device or os.environ.get("CAPTURE_WHISPER_DEVICE") or _auto_device()
        if compute_type is None:
            compute_type = os.environ.get("CAPTURE_WHISPER_COMPUTE") or (
                "float16" if device == "cuda" else "int8"
            )
        try:
            self._model = WhisperModel(model, device=device, compute_type=compute_type)
        except Exception:
            # A CUDA/DLL/compute mismatch must not kill ASR; fall back to CPU int8.
            if device == "cuda":
                log.warning(
                    "faster-whisper CUDA load failed (model=%s compute=%s); falling back to CPU/int8",
                    model, compute_type, exc_info=True,
                )
                self._model = WhisperModel(model, device="cpu", compute_type="int8")
                device, compute_type = "cpu", "int8"
            else:
                raise
        self.device = device
        self.compute_type = compute_type
        log.info("faster-whisper loaded: model=%s device=%s compute=%s", model, device, compute_type)

    def transcribe(self, pcm: np.ndarray, sample_rate: int) -> list[Segment]:
        path = _write_wav(pcm, sample_rate)
        try:
            segments, _ = self._model.transcribe(path, vad_filter=True)
            return [
                Segment(start=float(s.start), end=float(s.end), text=s.text.strip())
                for s in segments
                if s.text.strip()
            ]
        finally:
            Path(path).unlink(missing_ok=True)


def load() -> ASRBackend:
    errors = []
    for ctor in (MlxWhisper, FasterWhisper):
        try:
            backend = ctor()
            log.info("ASR backend loaded: %s", backend.name)
            return backend
        except Exception as e:  # ImportError or model-load failure
            errors.append(f"{ctor.__name__}: {e}")
    raise RuntimeError(
        "no local Whisper backend available. Install one of:\n"
        "  pip install mlx-whisper      # Apple Silicon\n"
        "  pip install faster-whisper   # cross-platform\n"
        "Tried: " + " | ".join(errors)
    )
