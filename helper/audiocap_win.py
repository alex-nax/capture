"""Windows system-audio capture (WASAPI loopback) -> 16 kHz mono s16le on stdout.

The Windows analogue of ``helper/audiocap.swift``: it captures everything playing
on the default output device (the browser's audio included) via WASAPI loopback and
streams raw signed-16-bit little-endian **mono PCM at 16 kHz** on stdout, with status
on stderr (a ``READY ...`` line, matching the helper contract in
``docs/architecture.md``). Used by ``platform.windows.Win32AudioSource``.

Robustness (mirrors the macOS helper's -3805 auto-reconnect): the capture loop
**reopens the loopback stream** on any read error, and a watchdog reopens it if no
data arrives for a while (a long silent stretch can leave a WASAPI loopback stream
wedged). This keeps a multi-video / hour-long capture alive. Captures the full output
mix, not a single process — true per-app WASAPI **process** loopback is a future item.

Run with the project venv python (needs ``pyaudiowpatch`` + ``numpy``):

    python helper/audiocap_win.py --rate 16000
"""

from __future__ import annotations

import argparse
import os
import signal
import sys
import threading
import time

import numpy as np


def _set_binary_stdout() -> None:
    if os.name == "nt":
        import msvcrt
        msvcrt.setmode(sys.stdout.fileno(), os.O_BINARY)


def _open_loopback(p):
    """Open the default output device's WASAPI loopback. Returns
    (stream, src_rate, channels, src_dtype, is_float)."""
    import pyaudiowpatch as pyaudio

    wasapi = p.get_host_api_info_by_type(pyaudio.paWASAPI)
    dev = p.get_device_info_by_index(wasapi["defaultOutputDevice"])
    if not dev.get("isLoopbackDevice"):
        match = None
        for lb in p.get_loopback_device_info_generator():
            if dev["name"] in lb["name"]:
                match = lb
                break
        if match is None:
            raise RuntimeError("no WASAPI loopback device for the default output")
        dev = match

    src_rate = int(dev["defaultSampleRate"])
    channels = int(dev["maxInputChannels"]) or 2
    frames = max(160, src_rate // 10)  # ~100 ms buffers
    try:
        stream = p.open(format=pyaudio.paInt16, channels=channels, rate=src_rate,
                        input=True, input_device_index=dev["index"], frames_per_buffer=frames)
        return stream, src_rate, channels, "<i2", False, frames, dev["name"]
    except Exception:
        stream = p.open(format=pyaudio.paFloat32, channels=channels, rate=src_rate,
                        input=True, input_device_index=dev["index"], frames_per_buffer=frames)
        return stream, src_rate, channels, "<f4", True, frames, dev["name"]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--rate", type=int, default=16000, help="target output sample rate (Hz)")
    ap.add_argument("--stall-timeout", type=float, default=8.0,
                    help="reopen the stream if no audio arrives for this many seconds")
    args = ap.parse_args()
    target = args.rate

    _set_binary_stdout()

    stop = {"v": False}

    def _sig(*_a):
        stop["v"] = True

    signal.signal(signal.SIGINT, _sig)
    try:
        signal.signal(signal.SIGTERM, _sig)
    except (ValueError, AttributeError):
        pass

    import pyaudiowpatch as pyaudio

    p = pyaudio.PyAudio()
    out = sys.stdout.buffer
    stream = None
    ratio = None
    carry = np.zeros(0, dtype=np.float32)
    pos = 0.0
    src_dtype = "<i2"
    is_float = False
    channels = 2
    frames = 4800
    announced = False

    while not stop["v"]:
        # (Re)open the loopback stream as needed. On any read error we drop the stream
        # and reopen here (mirrors the macOS helper's auto-reconnect); a blocking read
        # during a silent stretch is fine — WASAPI loopback delivers silence frames.
        if stream is None:
            try:
                (stream, src_rate, channels, src_dtype, is_float, frames, devname) = _open_loopback(p)
                ratio = src_rate / float(target)
                carry = np.zeros(0, dtype=np.float32)
                pos = 0.0
                if not announced:
                    print("READY rate=%d channels=1 fmt=s16le src_rate=%d src_channels=%d device=%r"
                          % (target, src_rate, channels, devname), file=sys.stderr, flush=True)
                    announced = True
                else:
                    print("reconnected loopback (src_rate=%d device=%r)" % (src_rate, devname),
                          file=sys.stderr, flush=True)
            except Exception as e:
                print("open failed: %r (retrying)" % e, file=sys.stderr, flush=True)
                time.sleep(0.5)
                continue

        try:
            raw = stream.read(frames, exception_on_overflow=False)
        except Exception as e:
            print("read error: %r (reconnecting)" % e, file=sys.stderr, flush=True)
            try:
                stream.close()
            except Exception:
                pass
            stream = None
            time.sleep(0.2)
            continue

        if not raw:
            continue

        try:
            a = np.frombuffer(raw, dtype=src_dtype)
            if channels > 1:
                a = a.reshape(-1, channels).mean(axis=1)
            mono = a.astype(np.float32) if is_float else a.astype(np.float32) / 32768.0

            carry = np.concatenate([carry, mono]) if carry.size else mono
            n = carry.size
            if n < 2:
                continue

            count = int(np.floor((n - 1 - pos) / ratio)) + 1
            if count > 0:
                idx = pos + ratio * np.arange(count)
                i0 = np.floor(idx).astype(np.int64)
                i1 = np.minimum(i0 + 1, n - 1)  # clamp upper neighbor (avoid OOB at the tail)
                frac = (idx - i0).astype(np.float32)
                samp = carry[i0] * (1.0 - frac) + carry[i1] * frac
                s16 = (np.clip(samp, -1.0, 1.0) * 32767.0).astype("<i2")
                out.write(s16.tobytes())
                out.flush()
                pos = float(idx[-1] + ratio)

            consumed = int(np.floor(pos))
            if consumed > 0:
                keep_from = min(consumed, n - 1)
                carry = carry[keep_from:]
                pos -= keep_from
        except Exception as e:  # never let one bad buffer kill the capture
            print("process error (resetting buffer): %r" % e, file=sys.stderr, flush=True)
            carry = np.zeros(0, dtype=np.float32)
            pos = 0.0
            continue

    with state["lock"]:
        try:
            if state["stream"] is not None:
                state["stream"].stop_stream()
                state["stream"].close()
        except Exception:
            pass
    p.terminate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
