"""ASR backend interface.

A backend receives mono float32 PCM chunks (range [-1, 1]) at a known sample
rate and returns recognized segments with timestamps *relative to the start of
the chunk*. The caller adds the chunk's absolute offset to place each segment on
the capture timeline.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np


@dataclass
class Segment:
    start: float  # seconds, relative to the chunk passed to transcribe()
    end: float
    text: str


class ASRBackend:
    name = "base"
    target_sample_rate = 16000  # what the backend wants its PCM resampled to

    def transcribe(self, pcm: np.ndarray, sample_rate: int) -> list[Segment]:
        raise NotImplementedError

    def close(self) -> None:  # optional cleanup hook
        pass
