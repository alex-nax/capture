"""Offline re-transcription of a saved session's ``audio.s16le``.

Re-runs ASR over a finished capture's raw PCM with the active (or a just-selected)
Whisper model, replacing ``transcript.jsonl``/``transcript.txt`` — e.g. to upgrade an
old transcript with a stronger model. The new segment timestamps are anchored to the
SAME audio epoch as the original (recovered from the old transcript's first record, so
subtitles still line up with the screenshots), and the chunking mirrors ``audio.py`` so
offsets match. The previous transcript is kept as ``transcript.prev.jsonl``/``.txt``.
"""

from __future__ import annotations

import json
import logging
from datetime import datetime
from pathlib import Path

import numpy as np

from . import asr as asr_pkg
from .util import iso

log = logging.getLogger(__name__)

SAMPLE_RATE = 16000
BYTES_PER_SAMPLE = 2
MIN_TAIL_BYTES = BYTES_PER_SAMPLE * SAMPLE_RATE // 10  # transcribe tails >= 0.1s (matches audio.py)


def _parse_iso(s: str) -> float | None:
    try:
        return datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()
    except Exception:
        return None


def _recover_epoch(d: Path) -> float:
    """The audio first-byte wall-clock, so new segments align with the screenshots:
    from the existing transcript's first record (``start - start_offset``), else the
    session's ``started_at``, else 0."""
    try:
        for ln in (d / "transcript.jsonl").read_text().splitlines():
            rec = json.loads(ln)
            st, off = _parse_iso(rec.get("start", "")), rec.get("start_offset")
            if st is not None and isinstance(off, (int, float)):
                return st - off
    except Exception:
        pass
    try:
        meta = json.loads((d / "session.json").read_text())
        st = _parse_iso(meta.get("summary", {}).get("started_at", ""))
        if st is not None:
            return st
    except Exception:
        pass
    return 0.0


def retranscribe_session(
    session_dir: "str | Path",
    asr_backend: str = "auto",
    chunk_seconds: float | None = None,
    on_progress=None,
) -> int:
    """Re-transcribe ``audio.s16le`` in ``session_dir``; returns the new segment count.

    ``on_progress(done_bytes, total_bytes, segments)`` is called as it advances. Raises
    ``ValueError`` if there's no audio. The active Whisper model is used (set it first via
    the model manager / ``asr.manager.set_active_model``)."""
    d = Path(session_dir)
    raw = d / "audio.s16le"
    if not (raw.is_file() and raw.stat().st_size > 0):
        raise ValueError("no audio to re-transcribe (audio.s16le missing or empty)")

    pcm_bytes = raw.read_bytes()
    epoch = _recover_epoch(d)
    backend = asr_pkg.create(asr_backend)
    if chunk_seconds is None:
        from .asr import manager as asr_manager

        chunk_seconds = asr_manager.active_chunk_seconds()

    # Preserve the prior transcript before overwriting.
    for name in ("transcript.jsonl", "transcript.txt"):
        f = d / name
        if f.exists():
            f.replace(d / name.replace("transcript", "transcript.prev"))

    chunk_bytes = int(chunk_seconds * SAMPLE_RATE) * BYTES_PER_SAMPLE
    total = len(pcm_bytes)
    segments = 0
    consumed_samples = 0
    try:
        with open(d / "transcript.jsonl", "w", buffering=1) as jf, \
             open(d / "transcript.txt", "w", buffering=1) as tf:
            pos = 0
            while pos < total:
                chunk = pcm_bytes[pos : pos + chunk_bytes]
                pos += len(chunk)
                if len(chunk) < MIN_TAIL_BYTES:
                    break
                chunk_offset = consumed_samples / SAMPLE_RATE
                pcm = np.frombuffer(chunk, dtype="<i2").astype(np.float32) / 32768.0
                if asr_pkg.is_silent(pcm):
                    segs = []  # skip silent chunks (Whisper hallucinates on silence)
                else:
                    try:
                        segs = backend.transcribe(pcm, SAMPLE_RATE)
                    except Exception:
                        log.exception("retranscribe ASR failed at offset %.1fs", chunk_offset)
                        segs = []
                for seg in segs:
                    rec = {
                        "start": iso(epoch + chunk_offset + seg.start),
                        "end": iso(epoch + chunk_offset + seg.end),
                        "start_offset": round(chunk_offset + seg.start, 3),
                        "end_offset": round(chunk_offset + seg.end, 3),
                        "text": seg.text,
                    }
                    jf.write(json.dumps(rec, ensure_ascii=False) + "\n")
                    tf.write(f"[{rec['start']}] {seg.text}\n")
                    segments += 1
                consumed_samples += len(chunk) // BYTES_PER_SAMPLE
                if on_progress and total:
                    on_progress(min(pos, total), total, segments)
    finally:
        try:
            backend.close()
        except Exception:
            pass
    log.info("re-transcribed %s: %d segments", d.name, segments)
    return segments
