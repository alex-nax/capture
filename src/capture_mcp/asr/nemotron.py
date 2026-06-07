"""Remote NVIDIA Riva / Nemotron-3.5 ASR adapter.

Nemotron-3.5-ASR is a 600M NeMo model that needs an NVIDIA GPU, so it cannot run
on this Mac. This adapter instead talks to a Riva server hosting it (self-hosted
or via NVIDIA's hosted endpoint). Configure with env vars:

  CAPTURE_RIVA_SERVER       host:port of the Riva gRPC endpoint (e.g. localhost:50051)
  CAPTURE_RIVA_API_KEY      bearer token, for NVIDIA-hosted endpoints (optional)
  CAPTURE_RIVA_FUNCTION_ID  NVIDIA-hosted function id selecting the model (optional)
  CAPTURE_RIVA_LANG         language code, default "en-US"
  CAPTURE_RIVA_MODEL        explicit Riva model name (self-hosted; optional)

The chunked, offline-recognize call here is the simplest correct integration;
swapping to Riva's true streaming API (cache-aware, the model's headline
feature) is a drop-in change to ``transcribe`` once you have live audio frames.
"""

from __future__ import annotations

import logging
import os

import numpy as np

from .base import ASRBackend, Segment

log = logging.getLogger(__name__)


class NemotronRiva(ASRBackend):
    name = "nemotron-riva"

    def __init__(
        self,
        server: str | None = None,
        api_key: str | None = None,
        language: str | None = None,
        model: str | None = None,
        function_id: str | None = None,
    ) -> None:
        import riva.client  # type: ignore

        self._server = server or os.environ.get("CAPTURE_RIVA_SERVER", "localhost:50051")
        self._api_key = api_key or os.environ.get("CAPTURE_RIVA_API_KEY")
        self._function_id = function_id or os.environ.get("CAPTURE_RIVA_FUNCTION_ID")
        self._language = language or os.environ.get("CAPTURE_RIVA_LANG", "en-US")
        self._model = model or os.environ.get("CAPTURE_RIVA_MODEL")

        metadata = []
        use_ssl = False
        if self._api_key:
            metadata.append(("authorization", f"Bearer {self._api_key}"))
            use_ssl = True
        # NVIDIA-hosted endpoints select the model via a function-id header.
        if self._function_id:
            metadata.append(("function-id", self._function_id))
            use_ssl = True
        auth = riva.client.Auth(uri=self._server, use_ssl=use_ssl, metadata_args=metadata)
        self._asr = riva.client.ASRService(auth)
        self._riva = riva.client
        log.info("Riva ASR connected: %s (lang=%s)", self._server, self._language)

    def transcribe(self, pcm: np.ndarray, sample_rate: int) -> list[Segment]:
        pcm16 = (np.clip(pcm, -1.0, 1.0) * 32767.0).astype("<i2").tobytes()

        config = self._riva.RecognitionConfig(
            encoding=self._riva.AudioEncoding.LINEAR_PCM,
            sample_rate_hertz=sample_rate,
            language_code=self._language,
            max_alternatives=1,
            enable_automatic_punctuation=True,
            enable_word_time_offsets=True,
        )
        if self._model:
            config.model = self._model

        resp = self._asr.offline_recognize(pcm16, config)
        out: list[Segment] = []
        for result in resp.results:
            if not result.alternatives:
                continue
            alt = result.alternatives[0]
            words = alt.words
            if words:
                start = words[0].start_time / 1000.0
                end = words[-1].end_time / 1000.0
            else:
                start = end = 0.0
            if alt.transcript.strip():
                out.append(Segment(start=start, end=end, text=alt.transcript.strip()))
        return out


def load() -> ASRBackend:
    return NemotronRiva()
