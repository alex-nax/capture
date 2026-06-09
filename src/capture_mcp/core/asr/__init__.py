"""ASR backend registry / factory.

``create(name)`` returns a backend instance. ``"auto"`` (default) prefers a
local Whisper backend and falls back to Riva/Nemotron if configured. Backends
are imported lazily so a missing optional dependency only fails the backend it
belongs to, not the whole server.
"""

from __future__ import annotations

import logging
import os

from .base import ASRBackend, Segment

log = logging.getLogger(__name__)

__all__ = ["ASRBackend", "Segment", "create", "available_backends"]

available_backends = ("auto", "local", "whisper", "openai", "openai-compat", "nemotron", "riva")


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
