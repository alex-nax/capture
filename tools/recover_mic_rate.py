#!/usr/bin/env python3
"""Recover a mic track recorded at the WRONG sample rate.

A Bluetooth headset mic runs at 8 kHz in hands-free (call) mode, but the v3 capture
treats every mic stream as 16 kHz — so the mic `.s16le` is written at half rate and
plays back ~2x fast / pitched up, which makes Whisper emit garbled, repeated
hallucinations. The actual audio is all there; it's just mislabeled. This tool
re-interprets it at its TRUE source rate and resamples to a real 16 kHz so it plays at
the correct speed and transcribes cleanly.

    # whole-session mic, default BT rate (8 kHz), write next to the original:
    python tools/recover_mic_rate.py --session 33ddb2

    # explicit file + source rate, and re-transcribe via the daemon (import):
    python tools/recover_mic_rate.py --in mic.s16le --src-rate 8000 --transcribe

Outputs `<name>.16k.s16le` + `<name>.16k.wav`. With --transcribe it imports the WAV
into the running daemon as a new session whose transcript is the corrected mic audio.

Future: fold this into a daemon "recover track" route + a GUI action (see #87 once the
mic-rate capture bug is fixed at the source).
"""
from __future__ import annotations

import argparse
import glob
import json
import os
import sys
import urllib.request
import wave

TARGET = 16000  # the ASR / on-disk s16le contract rate


def _resample_s16_mono(raw: bytes, src_rate: int, dst_rate: int) -> bytes:
    """src_rate -> dst_rate linear resample of mono signed-16 LE PCM. Tries stdlib
    `audioop` (fast C), falls back to a pure-Python linear interpolator (audioop is
    removed in Python 3.13)."""
    if src_rate == dst_rate:
        return raw
    try:
        import warnings

        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)  # audioop is deprecated in 3.11, gone in 3.13
            import audioop

            converted, _ = audioop.ratecv(raw, 2, 1, src_rate, dst_rate, None)
        return converted
    except Exception:
        import array

        src = array.array("h")
        src.frombytes(raw)
        n = len(src)
        if n < 2:
            return raw
        out_n = int(n * dst_rate / src_rate)
        out = array.array("h", bytes(2 * out_n))
        step = src_rate / dst_rate
        for i in range(out_n):
            pos = i * step
            j = int(pos)
            frac = pos - j
            a = src[j]
            b = src[j + 1] if j + 1 < n else a
            out[i] = int(a + (b - a) * frac)
        return out.tobytes()


def _find_session_dir(token: str) -> str | None:
    if os.path.isdir(token):
        return token
    runs = os.path.expanduser("~/.capture/runs")
    for d in sorted(glob.glob(os.path.join(runs, "*")), reverse=True):
        if token in os.path.basename(d):
            return d
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description="Resample a wrongly-rated mic .s16le back to real 16 kHz")
    ap.add_argument("--session", help="session id (suffix) or dir; uses its mic.s16le")
    ap.add_argument("--in", dest="infile", help="path to a mic .s16le (instead of --session)")
    ap.add_argument("--src-rate", type=int, default=8000,
                    help="the rate the audio was ACTUALLY captured at (Bluetooth HFP mic = 8000; default 8000)")
    ap.add_argument("--out", help="output .s16le (default: alongside the input)")
    ap.add_argument("--transcribe", action="store_true",
                    help="import the corrected WAV into the running daemon to get a clean transcript")
    a = ap.parse_args()

    if a.session:
        d = _find_session_dir(a.session)
        if not d:
            print(f"session not found: {a.session}", file=sys.stderr)
            return 2
        infile = os.path.join(d, "mic.s16le")
    elif a.infile:
        infile = a.infile
    else:
        print("pass --session <id|dir> or --in <mic.s16le>", file=sys.stderr)
        return 2
    if not os.path.isfile(infile):
        print(f"not found: {infile}", file=sys.stderr)
        return 2

    raw = open(infile, "rb").read()
    raw = raw[: len(raw) // 2 * 2]  # keep whole samples
    n_in = len(raw) // 2
    real_secs = n_in / a.src_rate  # the file's TRUE duration (samples interpreted at the real rate)
    as_stored = n_in / TARGET      # how long it (wrongly) plays back as 16 kHz
    print(f"in : {infile}")
    print(f"     {n_in} samples — {real_secs:.1f}s of real audio, currently plays as {as_stored:.1f}s "
          f"(~{real_secs/as_stored:.2f}x too fast)")

    converted = _resample_s16_mono(raw, a.src_rate, TARGET)
    out_s16 = a.out or (os.path.splitext(infile)[0] + ".16k.s16le")
    with open(out_s16, "wb") as f:
        f.write(converted)
    out_secs = (len(converted) // 2) / TARGET
    print(f"out: {out_s16}")
    print(f"     {len(converted)//2} samples — {out_secs:.1f}s @ {TARGET} Hz (correct speed restored)")

    wav = os.path.splitext(out_s16)[0] + ".wav"
    with wave.open(wav, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(TARGET)
        w.writeframes(converted)
    print(f"wav: {wav}")

    if a.transcribe:
        dj = json.load(open(os.path.expanduser("~/.capture/daemon.json")))
        ep = dj["endpoint"].rstrip("/")
        h = {"Authorization": "Bearer " + dj["token"], "Content-Type": "application/json"}
        body = json.dumps({"path": os.path.abspath(wav)}).encode()
        req = urllib.request.Request(ep + "/v1/sessions/import", data=body, method="POST", headers=h)
        r = json.load(urllib.request.urlopen(req, timeout=900))
        sid = r.get("session_id")
        print(f"imported as session {sid} — its transcript is the corrected mic audio "
              f"(open it in the app, or GET /v1/sessions/{sid}/transcript).")
    else:
        print("\nNext: play the .wav to confirm it sounds right, or re-run with --transcribe.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
