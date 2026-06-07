"""Offline re-transcribe a captured audio.s16le with faster-whisper (authoritative).

Live capture timestamps can drift if the loopback lags wall-clock; transcribing the
saved PCM directly gives a clean, gap-free transcript indexed by audio offset.

    python scripts/transcribe_audio.py <audio.s16le> [--model large-v3]
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from capture_mcp.asr.whisper_local import FasterWhisper  # noqa: E402


def fmt(t: float) -> str:
    h = int(t // 3600); m = int((t % 3600) // 60); s = t % 60
    return "%02d:%02d:%05.2f" % (h, m, s)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("audio")
    ap.add_argument("--model", default="large-v3")
    args = ap.parse_args()
    os.environ["CAPTURE_WHISPER_MODEL"] = args.model

    data = np.fromfile(args.audio, dtype="<i2").astype(np.float32) / 32768.0
    print("loaded %.1f min; loading %s on GPU..." % (data.size / 16000 / 60, args.model), flush=True)
    fw = FasterWhisper(model=args.model)
    print("device=%s compute=%s; transcribing..." % (fw.device, fw.compute_type), flush=True)

    segments, info = fw._model.transcribe(data, vad_filter=True, language="en",
                                          beam_size=5, condition_on_previous_text=False)
    base = Path(args.audio).with_suffix("")
    txt = open(str(base) + ".full_transcript.txt", "w", encoding="utf-8")
    jsonl = open(str(base) + ".full_transcript.jsonl", "w", encoding="utf-8")
    n = 0
    for s in segments:
        t = s.text.strip()
        if not t:
            continue
        txt.write("[%s] %s\n" % (fmt(s.start), t))
        jsonl.write(json.dumps({"start": round(s.start, 2), "end": round(s.end, 2), "text": t}, ensure_ascii=False) + "\n")
        n += 1
        if n % 50 == 0:
            print("  %d segments (at %s)..." % (n, fmt(s.start)), flush=True)
    txt.close(); jsonl.close()
    print("DONE: %d segments -> %s.full_transcript.{txt,jsonl}" % (n, base), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
