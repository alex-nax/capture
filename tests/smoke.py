"""End-to-end smoke test for capture-mcp (no pytest needed).

Run:  python tests/smoke.py

Exercises the paths that work without special permissions, on macOS or Windows:
  * launch-mode session: stdout/stderr logging + screenshots (+ format/resolution)
  * MCP async tool surface (capture_start / capture_status / capture_stop)
  * audio chunking + offsets + transcript files, using a stub ASR backend
  * parse_resolution edge cases

It does NOT require Screen Recording / Microphone permission or a GPU. The
per-app audio path and real ASR are validated separately (see README); here ASR
is stubbed so the test is hermetic and fast. Commands and temp paths are built
portably (via ``sys.executable`` and ``tempfile``) so the suite runs on either OS.
"""

from __future__ import annotations

import asyncio
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

PASS, FAIL = "PASS", "FAIL"
results: list[tuple[str, str, str]] = []
BASE = Path(tempfile.mkdtemp(prefix="capmcp-smoke-"))

# The server builds its SessionRegistry at import time; point the on-disk
# session index into the temp dir BEFORE importing it so the suite stays
# hermetic (never touches ~/.capture).
os.environ["CAPTURE_SESSION_INDEX"] = str(BASE / "sessions.jsonl")
# Shrink the events.jsonl snapshot interval so short captures exercise the
# periodic-snapshot path, not just the final snapshot.
os.environ["CAPTURE_EVENTS_SNAPSHOT_SECONDS"] = "0.5"

from capture_mcp.core.audio import SAMPLE_RATE, AudioCapture  # noqa: E402
from capture_mcp.core.asr.base import Segment  # noqa: E402
from capture_mcp.core.registry import SessionRegistry  # noqa: E402
from capture_mcp.core.screenshots import parse_resolution  # noqa: E402
from capture_mcp.core.session import CaptureSession  # noqa: E402
from capture_mcp.server import capture_start, capture_status, capture_stop, list_windows  # noqa: E402


def check(name: str, ok: bool, detail: str = "") -> None:
    results.append((PASS if ok else FAIL, name, detail))
    print(f"[{PASS if ok else FAIL}] {name}{' — ' + detail if detail else ''}")


def _cmdline(parts: list[str]) -> str:
    """Join argv into a command string the host OS (and proc.split_command) round-trips."""
    return subprocess.list2cmdline(parts) if os.name == "nt" else shlex.join(parts)


# A portable child that writes 3 lines to stdout and 3 to stderr with small sleeps.
_LAUNCH_CODE = (
    "import sys, time\n"
    "for i in (1, 2, 3):\n"
    "    sys.stdout.write('out%d\\n' % i)\n"
    "    sys.stderr.write('err%d\\n' % i)\n"
    "    sys.stdout.flush(); sys.stderr.flush()\n"
    "    time.sleep(0.3)\n"
)
# A portable child that streams a raw file to stdout (replaces Unix `cat`).
_STREAM_CODE = "import sys; sys.stdout.buffer.write(open(sys.argv[1], 'rb').read())"


async def test_launch_mode() -> None:
    out = str(BASE / "launch")
    shutil.rmtree(out, ignore_errors=True)
    s = await capture_start(
        output_dir=out,
        command=_cmdline([sys.executable, "-c", _LAUNCH_CODE]),
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

    events = [json.loads(x) for x in (d / "events.jsonl").read_text().strip().splitlines()]
    states = [e["state"] for e in events if e["type"] == "state"]
    check("events: lifecycle states in order", states == ["starting", "running", "stopping", "stopped"],
          f"states={states}")
    snaps = [e for e in events if e["type"] == "snapshot"]
    check("events: periodic + final snapshots, final last", len(snaps) >= 2 and events[-1]["type"] == "snapshot",
          f"snaps={len(snaps)}")
    check("events: final snapshot has final counters",
          snaps[-1]["summary"]["screenshots"] == fin["screenshots"]
          and snaps[-1]["summary"]["state"] == "stopped",
          f"snap_shots={snaps[-1]['summary']['screenshots']} fin={fin['screenshots']}")


async def test_validation() -> None:
    out = str(BASE / "val")
    try:
        await capture_start(output_dir=out, command="ls", pid=1)
        check("validation: rejects 2 targets", False)
    except ValueError:
        check("validation: rejects 2 targets", True)
    try:
        await capture_start(output_dir=out)
        check("validation: rejects 0 targets", False)
    except ValueError:
        check("validation: rejects 0 targets", True)


def test_audio_pipeline() -> None:
    out = BASE / "audio"
    shutil.rmtree(out, ignore_errors=True)
    raw = BASE / "smoke.s16le"
    raw.write_bytes(np.zeros(SAMPLE_RATE * 20, dtype="<i2").tobytes())  # 20s silence

    class StubASR:
        name = "stub"

        def transcribe(self, pcm, sr):
            secs = len(pcm) / sr
            return [Segment(start=0.5, end=max(0.6, secs - 0.5), text=f"chunk {secs:.1f}s")]

        def close(self):
            pass

    import capture_mcp.core.asr as asrpkg

    orig = asrpkg.create
    asrpkg.create = lambda name="auto": StubASR()
    try:
        ac = AudioCapture(out, source="mic", chunk_seconds=8.0, t0=1000.0)
        ac._build_command = lambda: ([sys.executable, "-c", _STREAM_CODE, str(raw)], "file")
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

    offs = [json.loads(x)["start_offset"] for x in lines]
    check("audio: offsets advance", offs == sorted(offs) and offs[0] == 0.5, f"offsets={offs}")


async def test_status_during_start() -> None:
    """start() must not hold the session lock through slow component startup."""
    out = str(BASE / "slowstart")
    orig = CaptureSession._start_audio
    CaptureSession._start_audio = lambda self: time.sleep(1.2)  # simulate slow ASR load
    try:
        t0 = time.monotonic()
        task = asyncio.create_task(
            capture_start(
                output_dir=out,
                command=_cmdline([sys.executable, "-c", "import time; time.sleep(3)"]),
                capture_screenshots=False,
                capture_audio=True,
            )
        )
        await asyncio.sleep(0.4)  # inside the slow start
        st = await capture_status()
        dt = time.monotonic() - t0
        starting = [s for s in st["sessions"] if s["state"] == "starting"]
        check("startlock: visible as 'starting' mid-start", len(starting) == 1,
              f"states={sorted(s['state'] for s in st['sessions'])}")
        check("startlock: status not blocked by start", dt < 1.0, f"dt={dt:.2f}s")
        s = await task
        check("startlock: reaches running", s["state"] == "running", s["state"])
        fin = await capture_stop(s["session_id"])
        check("startlock: stops clean", fin["state"] == "stopped", fin["state"])
    finally:
        CaptureSession._start_audio = orig


def _start_stub_asr_server():
    """Minimal OpenAI-compatible /v1/audio/transcriptions server (hermetic)."""
    import http.server
    import threading as _threading

    seen = {"requests": 0, "wav_ok": False, "model": None, "auth": None}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            seen["requests"] += 1
            seen["wav_ok"] = b"RIFF" in body and b"WAVE" in body
            if b'name="model"' in body:
                seen["model"] = body.split(b'name="model"\r\n\r\n')[1].split(b"\r\n")[0].decode()
            seen["auth"] = self.headers.get("Authorization")
            resp = json.dumps({
                "text": "hello world",
                "segments": [
                    {"start": 0.5, "end": 1.5, "text": " hello "},
                    {"start": 2.0, "end": 3.0, "text": "world"},
                    {"start": 3.5, "end": 4.0, "text": "   "},  # blank: must be skipped
                ],
            }).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(resp)))
            self.end_headers()
            self.wfile.write(resp)

        def log_message(self, *a):  # keep smoke output clean
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    _threading.Thread(target=srv.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{srv.server_address[1]}/v1/audio/transcriptions"
    return srv, url, seen


def test_openai_compat() -> None:
    from capture_mcp.core.asr.openai_compat import OpenAICompat

    srv, url, seen = _start_stub_asr_server()
    try:
        # Direct backend: WAV upload, auth/model fields, verbose_json mapping.
        be = OpenAICompat(url, model="whisper-x", api_key="sek")
        segs = be.transcribe(np.zeros(SAMPLE_RATE * 4, dtype=np.float32), SAMPLE_RATE)
        check("openai: segments mapped, blanks skipped",
              [(s.start, s.end, s.text) for s in segs] == [(0.5, 1.5, "hello"), (2.0, 3.0, "world")],
              str([(s.start, s.end, s.text) for s in segs]))
        check("openai: wav + model + bearer reached server",
              seen["wav_ok"] and seen["model"] == "whisper-x" and seen["auth"] == "Bearer sek",
              str(seen))

        # Full pipeline: AudioCapture with asr_backend="openai" via env config.
        out = BASE / "openai"
        raw = BASE / "openai.s16le"
        raw.write_bytes(np.zeros(SAMPLE_RATE * 20, dtype="<i2").tobytes())
        os.environ["CAPTURE_OPENAI_ASR_URL"] = url
        try:
            ac = AudioCapture(out, source="mic", chunk_seconds=8.0, t0=1000.0, asr_backend="openai")
            ac._build_command = lambda: ([sys.executable, "-c", _STREAM_CODE, str(raw)], "file")
            ac.start()
            while ac._proc and ac._proc.poll() is None:
                time.sleep(0.1)
            time.sleep(0.5)
            ac.stop()
        finally:
            del os.environ["CAPTURE_OPENAI_ASR_URL"]

        lines = [json.loads(x) for x in (out / "transcript.jsonl").read_text().strip().splitlines()]
        offs = [r["start_offset"] for r in lines]
        check("openai: pipeline yields timestamped segments", len(lines) == 6
              and offs == [0.5, 2.0, 8.5, 10.0, 16.5, 18.0], f"offsets={offs}")
        check("openai: absolute timestamps present", all(r["start"] and r["end"] for r in lines))
    finally:
        srv.shutdown()


def test_event_bus() -> None:
    """Subscribers receive component events (log_line, screenshot_taken) live."""
    import queue as _queue

    sess = CaptureSession(
        str(BASE / "bus"),
        command=_cmdline([sys.executable, "-c", _LAUNCH_CODE]),
        screenshot_interval=0.4,
        capture_audio=False,
    )
    sub = sess.events.subscribe()
    sess.start()
    time.sleep(1.4)
    sess.stop()

    got: list[dict] = []
    while True:
        try:
            got.append(sub.get(timeout=0.05))
        except _queue.Empty:
            break
    sub.close()
    types = {e["type"] for e in got}
    log_lines = [e for e in got if e["type"] == "log_line"]
    check("bus: state events delivered", {"state"} <= types, f"types={sorted(types)}")
    check("bus: log_line events with stream tags", len(log_lines) == 6
          and {e["stream"] for e in log_lines} == {"out", "err"}, f"n={len(log_lines)}")
    check("bus: screenshot_taken events delivered", any(e["type"] == "screenshot_taken" for e in got),
          f"types={sorted(types)}")
    check("bus: no drops on a small capture", sub.dropped == 0, f"dropped={sub.dropped}")


def test_registry_history() -> None:
    """A fresh registry rebuilds finished sessions from the on-disk index."""
    idx = Path(os.environ["CAPTURE_SESSION_INDEX"])

    reg = SessionRegistry(index_path=idx)
    stopped = [s for s in reg.summaries() if s["state"] == "stopped"]
    check("registry: stopped sessions recovered from disk", len(stopped) >= 2, f"n={len(stopped)}")

    # A session recorded as live by a process that died -> "interrupted";
    # an index entry whose session.json is gone -> "unknown"; corrupt index
    # lines are tolerated.
    fake_id = "19990101T000000-abc123"
    fake = BASE / "fakehist" / f"capture-{fake_id}"
    fake.mkdir(parents=True, exist_ok=True)
    (fake / "session.json").write_text(
        json.dumps({"config": {}, "summary": {"session_id": fake_id, "state": "running", "dir": str(fake)}})
    )
    with idx.open("a", encoding="utf-8") as f:
        f.write(json.dumps({"id": fake_id, "dir": str(fake)}) + "\n")
        f.write(json.dumps({"id": "19990101T000001-gone00", "dir": str(BASE / "nonexistent")}) + "\n")
        f.write("{not json\n")

    reg2 = SessionRegistry(index_path=idx)
    by_id = {s["session_id"]: s for s in reg2.summaries()}
    check("registry: live-at-crash -> interrupted", by_id.get(fake_id, {}).get("state") == "interrupted",
          str(by_id.get(fake_id, {}).get("state")))
    check("registry: missing session.json -> unknown",
          by_id.get("19990101T000001-gone00", {}).get("state") == "unknown")
    check("registry: summaries oldest-first", list(by_id) == sorted(by_id))


async def test_list_windows() -> None:
    res = await list_windows()
    wins = res["windows"]
    check("windows: shape + count", isinstance(wins, list) and res["count"] == len(wins),
          f"count={res['count']}")
    fields = ["app_name", "height", "pid", "title", "width", "window_id"]
    check("windows: entry fields", all(sorted(w) == fields for w in wins),
          f"n={len(wins)}")
    areas = [w["width"] * w["height"] for w in wins]
    check("windows: largest-first", areas == sorted(areas, reverse=True), f"areas={areas[:5]}")
    if wins:
        name = wins[0]["app_name"]
        sub = await list_windows(app_name=name)
        ok = sub["count"] >= 1 and all(name.lower() in w["app_name"].lower() for w in sub["windows"])
        check("windows: app_name filter", ok, f"{name!r} -> {sub['count']}")
    else:
        check("windows: app_name filter", True, "skipped (no windows; headless?)")


def test_helper_path() -> None:
    """Regression guard: the audiocap helper path must resolve to <repo>/helper/audiocap.

    The M0a split (#25) moved platform/macos.py one level deeper into core/, and the
    parents[N] walk-up had to grow by one (parents[3]->[4]). A too-short walk silently
    points at src/helper/audiocap, so per-app audio degrades to "no audio source" with
    NO error and NO smoke failure (the audio test stubs ASR + uses the mic source).
    This pins the path computation hermetically so the off-by-one can't come back.
    Found in real use mid-meeting (capture produced screenshots but a silent transcript).
    """
    if sys.platform != "darwin":
        check("helper-path: repo-relative (darwin-only)", True, "skipped (not darwin)")
        return
    from capture_mcp.core.platform import macos  # noqa: E402

    repo_root = Path(__file__).resolve().parent.parent
    expected = repo_root / "helper" / "audiocap"
    ok = macos._HELPER == expected
    check("helper-path: resolves to <repo>/helper/audiocap", ok,
          "" if ok else f"{macos._HELPER} != {expected}")
    # When the helper is actually built (this dev box), helper_path() must surface it,
    # not None — the exact end-to-end signal whose absence broke per-app audio.
    if expected.exists():
        check("helper-path: helper_path() returns the built binary",
              macos.helper_path() == expected, str(macos.helper_path()))
    else:
        check("helper-path: helper_path() returns the built binary", True,
              "skipped (helper not built here)")


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
    try:
        await test_launch_mode()
        await test_validation()
        test_audio_pipeline()
        await test_status_during_start()
        test_openai_compat()
        test_event_bus()
        test_registry_history()
        await test_list_windows()
        test_helper_path()
        test_parse_resolution()
    finally:
        shutil.rmtree(BASE, ignore_errors=True)
    failed = [r for r in results if r[0] == FAIL]
    print(f"\n{len(results) - len(failed)}/{len(results)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
