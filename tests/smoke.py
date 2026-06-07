"""End-to-end smoke test for capture-mcp (no pytest needed).

Run:  python tests/smoke.py

Exercises the paths that work without special permissions:
  * launch-mode session: stdout/stderr logging + screenshots (+ format/resolution)
  * MCP async tool surface (capture_start / capture_status / capture_stop)
  * audio chunking + offsets + transcript files, using a stub ASR backend
  * parse_resolution edge cases

It does NOT require Screen Recording / Microphone permission or a GPU. The
per-app ScreenCaptureKit audio path and real ASR are validated separately (see
README); here ASR is stubbed so the test is hermetic and fast.
"""

from __future__ import annotations

import asyncio
import shutil
import sys
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from capture_mcp.audio import SAMPLE_RATE, AudioCapture  # noqa: E402
from capture_mcp.asr.base import Segment  # noqa: E402
from capture_mcp.screenshots import parse_resolution  # noqa: E402
from capture_mcp.server import capture_start, capture_status, capture_stop  # noqa: E402

PASS, FAIL = "PASS", "FAIL"
results: list[tuple[str, str, str]] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    results.append((PASS if ok else FAIL, name, detail))
    print(f"[{PASS if ok else FAIL}] {name}{' — ' + detail if detail else ''}")


async def test_launch_mode() -> None:
    out = "/tmp/capmcp-smoke-launch"
    shutil.rmtree(out, ignore_errors=True)
    s = await capture_start(
        output_dir=out,
        command="bash -c 'for i in 1 2 3; do echo out$i; echo err$i 1>&2; sleep 0.3; done'",
        screenshot_interval=0.4,
        screenshot_resolution="640x480/jpg",
        capture_audio=False,
    )
    await asyncio.sleep(1.6)
    fin = await capture_stop(s["session_id"])
    d = Path(fin["dir"])
    check("launch: state stopped", fin["state"] == "stopped", fin["state"])
    check("launch: stdout/stderr captured", fin["log_lines"] == 6, f"log_lines={fin['log_lines']}")
    check("launch: screenshots captured", fin["screenshots"] >= 2, f"n={fin['screenshots']}")
    jpgs = list((d / "screenshots").glob("*.jpg"))
    check("launch: jpg format honored", len(jpgs) == fin["screenshots"], f"jpg={len(jpgs)}")
    check("launch: logs on disk", (d / "output.log").exists() and (d / "stderr.log").exists())
    st = await capture_status(s["session_id"])
    check("status: queryable by id", st["session_id"] == s["session_id"])


async def test_validation() -> None:
    try:
        await capture_start(output_dir="/tmp/x", command="ls", pid=1)
        check("validation: rejects 2 targets", False)
    except ValueError:
        check("validation: rejects 2 targets", True)
    try:
        await capture_start(output_dir="/tmp/x")
        check("validation: rejects 0 targets", False)
    except ValueError:
        check("validation: rejects 0 targets", True)


def test_audio_pipeline() -> None:
    out = Path("/tmp/capmcp-smoke-audio")
    shutil.rmtree(out, ignore_errors=True)
    raw = Path("/tmp/capmcp-smoke.s16le")
    raw.write_bytes(np.zeros(SAMPLE_RATE * 20, dtype="<i2").tobytes())  # 20s silence

    class StubASR:
        name = "stub"

        def transcribe(self, pcm, sr):
            secs = len(pcm) / sr
            return [Segment(start=0.5, end=max(0.6, secs - 0.5), text=f"chunk {secs:.1f}s")]

        def close(self):
            pass

    import capture_mcp.asr as asrpkg

    orig = asrpkg.create
    asrpkg.create = lambda name="auto": StubASR()
    try:
        ac = AudioCapture(out, source="mic", chunk_seconds=8.0, t0=1000.0)
        ac._build_command = lambda: (["cat", str(raw)], "file")
        ac.start()
        while ac._proc and ac._proc.poll() is None:
            time.sleep(0.1)
        time.sleep(0.5)
        ac.stop()
    finally:
        asrpkg.create = orig

    check("audio: 20s -> 3 chunks (8+8+4)", ac.segments == 3, f"segments={ac.segments}")
    check("audio: raw saved", (out / "audio.s16le").stat().st_size == SAMPLE_RATE * 20 * 2)
    lines = (out / "transcript.jsonl").read_text().strip().splitlines()
    check("audio: jsonl lines == segments", len(lines) == ac.segments, f"lines={len(lines)}")
    import json

    offs = [json.loads(x)["start_offset"] for x in lines]
    check("audio: offsets advance", offs == sorted(offs) and offs[0] == 0.5, f"offsets={offs}")


def test_parse_resolution() -> None:
    check("parse: WxH/fmt", parse_resolution("1280x720/jpg") == (1280, 720, "jpg"))
    check("parse: WxH", parse_resolution("640x480") == (640, 480, None))
    check("parse: None", parse_resolution(None) is None)
    for bad in ("bad", "10x", "1x2x3", "axb", "0x0"):
        try:
            parse_resolution(bad)
            check(f"parse: rejects {bad!r}", False)
        except ValueError:
            check(f"parse: rejects {bad!r}", True)


async def main() -> int:
    await test_launch_mode()
    await test_validation()
    test_audio_pipeline()
    test_parse_resolution()
    failed = [r for r in results if r[0] == FAIL]
    print(f"\n{len(results) - len(failed)}/{len(results)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
