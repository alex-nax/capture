"""OpenAI-compatible remote ASR backend (feature #28).

POSTs each PCM chunk — wrapped as an in-memory WAV — to any server implementing
the OpenAI audio-transcriptions API (``POST <url>`` as ``multipart/form-data``
with a ``file`` part, optional ``model``/``language`` fields, and
``response_format=verbose_json`` for per-segment timestamps). Works against
faster-whisper-server / speaches, whisper.cpp's server, vLLM, NVIDIA NIMs with
an OpenAI facade, or api.openai.com itself — which turns the Nemotron
WSL2/Docker lab into a plain endpoint.

Deliberately **stdlib-only** (urllib + wave): usable from the ``minimal``
install with zero ASR dependencies.

Configuration (env, read by ``load()``):
  CAPTURE_OPENAI_ASR_URL       full endpoint URL, e.g.
                               ``http://localhost:8000/v1/audio/transcriptions``
                               (required to enable the backend)
  CAPTURE_OPENAI_ASR_MODEL     ``model`` form field (optional; many local
                               servers ignore it or have a default)
  CAPTURE_OPENAI_ASR_KEY       bearer token (optional; sent as Authorization)
  CAPTURE_OPENAI_ASR_LANGUAGE  ``language`` form field (optional)
  CAPTURE_OPENAI_ASR_TIMEOUT   per-request seconds (default 120)
"""

from __future__ import annotations

import io
import json
import logging
import os
import urllib.error
import urllib.request
import uuid
import wave

import numpy as np

from .base import ASRBackend, Segment

log = logging.getLogger(__name__)

ENV_URL = "CAPTURE_OPENAI_ASR_URL"
ENV_MODEL = "CAPTURE_OPENAI_ASR_MODEL"
ENV_KEY = "CAPTURE_OPENAI_ASR_KEY"
ENV_LANGUAGE = "CAPTURE_OPENAI_ASR_LANGUAGE"
ENV_TIMEOUT = "CAPTURE_OPENAI_ASR_TIMEOUT"


def load() -> "OpenAICompat":
    url = os.environ.get(ENV_URL, "").strip()
    if not url:
        raise RuntimeError(
            f"{ENV_URL} is not set "
            "(e.g. http://localhost:8000/v1/audio/transcriptions)"
        )
    return OpenAICompat(
        url,
        model=os.environ.get(ENV_MODEL) or None,
        api_key=os.environ.get(ENV_KEY) or None,
        language=os.environ.get(ENV_LANGUAGE) or None,
        timeout=float(os.environ.get(ENV_TIMEOUT, "120")),
    )


def _to_wav(pcm: np.ndarray, sample_rate: int) -> bytes:
    """float32 [-1,1] mono -> in-memory 16-bit PCM WAV."""
    s16 = (np.clip(pcm, -1.0, 1.0) * 32767.0).astype("<i2")
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sample_rate)
        w.writeframes(s16.tobytes())
    return buf.getvalue()


def _multipart(fields: dict[str, str], file_field: str, filename: str, payload: bytes) -> tuple[bytes, str]:
    """Encode fields + one file as multipart/form-data (stdlib has no helper)."""
    boundary = uuid.uuid4().hex
    out = io.BytesIO()
    for k, v in fields.items():
        out.write(
            f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode()
        )
    out.write(
        f'--{boundary}\r\nContent-Disposition: form-data; name="{file_field}"; '
        f'filename="{filename}"\r\nContent-Type: audio/wav\r\n\r\n'.encode()
    )
    out.write(payload)
    out.write(f"\r\n--{boundary}--\r\n".encode())
    return out.getvalue(), f"multipart/form-data; boundary={boundary}"


class OpenAICompat(ASRBackend):
    name = "openai-compat"

    def __init__(
        self,
        url: str,
        *,
        model: str | None = None,
        api_key: str | None = None,
        language: str | None = None,
        timeout: float = 120.0,
    ) -> None:
        self.url = url
        self.model = model
        self.api_key = api_key
        self.language = language
        self.timeout = timeout

    def transcribe(self, pcm: np.ndarray, sample_rate: int) -> list[Segment]:
        duration = len(pcm) / float(sample_rate)
        fields = {"response_format": "verbose_json"}
        if self.model:
            fields["model"] = self.model
        if self.language:
            fields["language"] = self.language
        body, content_type = _multipart(fields, "file", "chunk.wav", _to_wav(pcm, sample_rate))

        headers = {"Content-Type": content_type}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        req = urllib.request.Request(self.url, data=body, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                payload = json.load(resp)
        except urllib.error.HTTPError as e:
            # Include the server's explanation; AudioCapture counts the raise
            # as an asr_error and the capture continues.
            detail = e.read(500).decode(errors="replace")
            raise RuntimeError(f"ASR endpoint {e.code}: {detail}") from e

        return self._parse(payload, duration)

    @staticmethod
    def _parse(payload: dict, duration: float) -> list[Segment]:
        segments = payload.get("segments")
        if isinstance(segments, list):
            out = []
            for s in segments:
                text = (s.get("text") or "").strip()
                if not text:
                    continue
                start = max(0.0, float(s.get("start", 0.0)))
                end = min(duration, max(start, float(s.get("end", duration))))
                out.append(Segment(start=start, end=end, text=text))
            return out
        # Plain `json` response shape: one undifferentiated text blob.
        text = (payload.get("text") or "").strip()
        if text:
            return [Segment(start=0.0, end=duration, text=text)]
        if "text" in payload:
            return []  # explicit empty transcription (silence)
        raise RuntimeError(f"unrecognized transcription response keys: {sorted(payload)}")
