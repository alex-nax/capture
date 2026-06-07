"""ASR backend registry / factory.

``create(name)`` returns a backend instance. ``"auto"`` (default) prefers a
local Whisper backend and falls back to Riva/Nemotron if configured. Backends
are imported lazily so a missing optional dependency only fails the backend it
belongs to, not the whole server.
"""

from __future__ import annotations

import logging

from .base import ASRBackend, Segment

log = logging.getLogger(__name__)

__all__ = ["ASRBackend", "Segment", "create", "available_backends"]

available_backends = ("auto", "local", "whisper", "nemotron", "riva")


def create(name: str = "auto") -> ASRBackend:
    name = (name or "auto").lower()

    if name in ("local", "whisper"):
        from . import whisper_local

        return whisper_local.load()

    if name in ("nemotron", "riva"):
        from . import nemotron

        return nemotron.load()

    if name == "auto":
        try:
            from . import whisper_local

            return whisper_local.load()
        except Exception as e:
            log.warning("local ASR unavailable (%s); trying Riva/Nemotron", e)
            from . import nemotron

            return nemotron.load()

    raise ValueError(f"unknown ASR backend {name!r}; choose from {available_backends}")
