"""Re-transcribe a captured audio file via the capture daemon (v3 ``/v1``).

Live capture timestamps can drift if the loopback lags wall-clock; importing the
saved audio gives a clean, gap-free transcript indexed by audio offset. Under v3 the
ASR engine lives in the native ``captured`` daemon — this proxies it:
``POST /v1/sessions/import`` extracts audio + runs ASR + writes a transcript, then
returns the new session's summary (progress streams over ``/v1/events``).

Accepts either a normal media file (audio/video the daemon can demux) or a raw
headerless ``audio.s16le`` (16 kHz mono signed-16 PCM, as written by the live
capture); the raw form is wrapped into a temporary WAV so the daemon can ingest it.
Pure stdlib — no venv, no deps; just needs a running daemon.

    python3 scripts/transcribe_audio.py <audio.s16le|media-file> [--out-dir DIR] [--asr-backend auto]
"""
from __future__ import annotations

import argparse
import sys
import tempfile
import wave
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "tools"))
import capture_v1  # noqa: E402

# Raw-PCM layout written by the live audio capture (loopback resampled to this).
_PCM_RATE = 16000
_PCM_CHANNELS = 1
_PCM_SAMPLE_WIDTH = 2  # signed 16-bit little-endian


def wrap_pcm_as_wav(pcm: Path, dst: Path) -> None:
    """Wrap headerless 16 kHz mono s16le PCM into a WAV the daemon can demux."""
    with wave.open(str(dst), "wb") as w:
        w.setnchannels(_PCM_CHANNELS)
        w.setsampwidth(_PCM_SAMPLE_WIDTH)
        w.setframerate(_PCM_RATE)
        w.writeframes(pcm.read_bytes())


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("audio", help="path to audio.s16le (raw PCM) or any media file")
    ap.add_argument("--out-dir", default=None,
                    help="output dir for the imported session (default: daemon's runs dir)")
    ap.add_argument("--asr-backend", default="auto",
                    help="ASR backend for the daemon to use (default: auto)")
    args = ap.parse_args()

    daemon = capture_v1.Daemon.discover()
    if daemon is None or not daemon.available():
        print("ERROR: no capture daemon reachable (start `captured`, or set "
              "$CAPTURE_DAEMON_JSON).", file=sys.stderr)
        return 2

    src = Path(args.audio).expanduser()
    if not src.is_file():
        print("ERROR: file not found: %s" % src, file=sys.stderr)
        return 2

    tmp = None
    import_path = src
    if src.suffix == ".s16le":
        # Daemon imports demuxable media; wrap the raw PCM into a WAV first.
        tmp = Path(tempfile.mkdtemp(prefix="transcribe_")) / (src.stem + ".wav")
        print("wrapping raw PCM -> %s ..." % tmp, flush=True)
        wrap_pcm_as_wav(src, tmp)
        import_path = tmp

    body = {"path": str(import_path), "asr_backend": args.asr_backend}
    if args.out_dir:
        body["output_dir"] = str(Path(args.out_dir).expanduser())

    print("importing via daemon (extract audio + ASR)...", flush=True)
    try:
        summary = daemon.post("/v1/sessions/import", body, timeout=3600)
    except Exception as e:
        print("ERROR: import failed: %r" % e, file=sys.stderr)
        return 1
    finally:
        if tmp is not None:
            try:
                tmp.unlink()
                tmp.parent.rmdir()
            except Exception:
                pass

    sid = summary.get("session_id", "?")
    segs = summary.get("transcript_segments")
    sdir = summary.get("dir", "?")
    print("DONE: session %s (segments=%s) -> %s" % (sid, segs, sdir), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
