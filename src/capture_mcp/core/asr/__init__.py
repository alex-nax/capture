"""ASR backend registry / factory.

``create(name)`` returns a backend instance. ``"auto"`` (default) prefers a
local Whisper backend and falls back to Riva/Nemotron if configured. Backends
are imported lazily so a missing optional dependency only fails the backend it
belongs to, not the whole server.
"""

from __future__ import annotations

import logging
import os

import numpy as np

from .base import ASRBackend, Segment

log = logging.getLogger(__name__)

__all__ = ["ASRBackend", "Segment", "create", "available_backends", "is_silent"]

available_backends = ("auto", "local", "whisper", "openai", "openai-compat", "nemotron", "riva")

#: int16-scale RMS below which a chunk is treated as silence and SKIPPED (not sent to
#: Whisper). Whisper hallucinates phantom phrases / token loops on silence — a dead mic
#: (rms ~40) produced "Thank you." every chunk. Tunable via CAPTURE_ASR_SILENCE_RMS.
SILENCE_RMS16 = float(os.environ.get("CAPTURE_ASR_SILENCE_RMS", "70"))


def is_silent(pcm: "np.ndarray", threshold16: float = SILENCE_RMS16) -> bool:
    """True if ``pcm`` (float32 in [-1, 1]) is near-silent — its RMS (rescaled to the
    int16 range) is below ``threshold16``. Used to skip transcribing silent chunks so
    Whisper doesn't hallucinate on them."""
    if pcm.size == 0:
        return True
    rms = float(np.sqrt(np.mean(np.square(pcm, dtype=np.float64)))) * 32768.0
    return rms < threshold16


def create(name: str = "auto") -> ASRBackend:
    name = (name or "auto").lower()

    if name in ("local", "whisper"):
        from . import whisper_local

        return whisper_local.load()

    if name in ("openai", "openai-compat", "openai_compat"):
        from . import openai_compat

        return openai_compat.load()

    if name in ("nemotron", "riva"):
        from . import nemotron

        return nemotron.load()

    if name == "auto":
        try:
            from . import whisper_local

            return whisper_local.load()
        except Exception as e:
            log.warning("local ASR unavailable (%s); trying remote backends", e)
            # Prefer a configured OpenAI-compatible endpoint, then Riva/Nemotron.
            if os.environ.get("CAPTURE_OPENAI_ASR_URL", "").strip():
                try:
                    from . import openai_compat

                    return openai_compat.load()
                except Exception as e2:
                    log.warning("openai-compat ASR unavailable (%s); trying Riva/Nemotron", e2)
            from . import nemotron

            return nemotron.load()

    raise ValueError(f"unknown ASR backend {name!r}; choose from {available_backends}")
