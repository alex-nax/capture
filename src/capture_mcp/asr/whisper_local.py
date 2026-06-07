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


class FasterWhisper(ASRBackend):
    name = "faster-whisper"

    def __init__(self, model: str | None = None, compute_type: str = "int8") -> None:
        from faster_whisper import WhisperModel

        model = model or os.environ.get("CAPTURE_WHISPER_MODEL", _FW_DEFAULT)
        self._model = WhisperModel(model, device="cpu", compute_type=compute_type)

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
