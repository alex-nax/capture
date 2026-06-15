"""Contract tests for capture-mcp's frozen interfaces (feature #27, M0c).

Run:    python tests/contract/run_contracts.py
Regen:  python tests/contract/run_contracts.py --regen   (after an INTENTIONAL change)

Pins the interfaces every frontend (MCP today; daemon/CLI/GUI per
docs/specs/product-architecture.md) builds on, so refactors fail loudly here
instead of silently drifting:

  * MCP tools/list           — tool names + input schemas (descriptions ignored)
  * session dir layout       — file set, session.json key structure,
                               events.jsonl event types + state sequence
  * transcript record shape  — keys of a transcript.jsonl line
  * PCM chunking math        — 20s @ 8s windows -> exact chunk/segment offsets

Goldens live in tests/contract/golden/ and are OS-neutral by construction
(key names and pure-math offsets only — no timestamps, paths, pids, or counts).
"""

from __future__ import annotations

import asyncio
import json
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
GOLDEN = HERE / "golden"
sys.path.insert(0, str(HERE.parent.parent / "src"))

import os  # noqa: E402

BASE = Path(tempfile.mkdtemp(prefix="capmcp-contract-"))
os.environ["CAPTURE_SESSION_INDEX"] = str(BASE / "sessions.jsonl")
os.environ["CAPTURE_EVENTS_SNAPSHOT_SECONDS"] = "0.5"

import numpy as np  # noqa: E402

from capture_mcp.core.audio import SAMPLE_RATE, AudioCapture  # noqa: E402
from capture_mcp.core.asr.base import Segment  # noqa: E402
from capture_mcp.core.session import CaptureSession  # noqa: E402

results: list[tuple[bool, str, str]] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    results.append((ok, name, detail))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}{' — ' + detail if detail else ''}")


def _strip_descriptions(obj):
    """Schemas minus prose: doc edits must not be contract breaks."""
    if isinstance(obj, dict):
        return {k: _strip_descriptions(v) for k, v in sorted(obj.items()) if k != "description"}
    if isinstance(obj, list):
        return [_strip_descriptions(x) for x in obj]
    return obj


# -- current-state extractors ---------------------------------------------------


def current_tools_list() -> list[dict]:
    from capture_mcp.server import mcp

    tools = asyncio.run(mcp.list_tools())
    return [{"name": t.name, "inputSchema": _strip_descriptions(t.inputSchema)} for t in tools]


def current_session_dir() -> dict:
    """Run a real (audio-less) launch capture and describe its artifact layout."""
    code = "import sys,time\nfor i in (1,2):\n print('o%d'%i);sys.stderr.write('e%d\\n'%i);time.sleep(0.2)\n"
    cmd = (
        subprocess.list2cmdline([sys.executable, "-c", code])
        if os.name == "nt"
        else shlex.join([sys.executable, "-c", code])
    )
    sess = CaptureSession(
        str(BASE / "layout"), command=cmd, screenshot_interval=0.4, capture_audio=False
    )
    sess.start()
    time.sleep(1.0)
    sess.stop()
    d = sess.dir

    meta = json.loads((d / "session.json").read_text())
    events = [json.loads(x) for x in (d / "events.jsonl").read_text().strip().splitlines()]
    files = sorted(
        p.name + ("/" if p.is_dir() else "") for p in d.iterdir()
    )
    return {
        "files": files,
        "session_json": {
            "top_level": sorted(meta),
            "config_keys": sorted(meta["config"]),
            "summary_keys": sorted(meta["summary"]),
        },
        "events_jsonl": {
            "event_keys_state": sorted(next(e for e in events if e["type"] == "state")),
            "event_keys_snapshot": sorted(next(e for e in events if e["type"] == "snapshot")),
            "state_sequence": [e["state"] for e in events if e["type"] == "state"],
            "final_line_type": events[-1]["type"],
        },
    }


def current_pcm_chunking() -> dict:
    """20s of PCM through the 8s-window chunker with a stub ASR: pure-math offsets."""

    class StubASR:
        name = "stub"

        def transcribe(self, pcm, sr):
            secs = len(pcm) / sr
            return [Segment(start=0.5, end=max(0.6, secs - 0.5), text=f"chunk {secs:.1f}s")]

        def close(self):
            pass

    import capture_mcp.core.asr as asrpkg

    raw = BASE / "pcm.s16le"
    raw.write_bytes(np.zeros(SAMPLE_RATE * 20, dtype="<i2").tobytes())
    stream_code = "import sys; sys.stdout.buffer.write(open(sys.argv[1],'rb').read())"

    orig = asrpkg.create
    asrpkg.create = lambda name="auto": StubASR()
    try:
        ac = AudioCapture(BASE / "chunks", source="mic", chunk_seconds=8.0, t0=1000.0)
        ac._build_command = lambda: ([sys.executable, "-c", stream_code, str(raw)], "file")
        ac.start()
        while ac._proc and ac._proc.poll() is None:
            time.sleep(0.1)
        time.sleep(0.5)
        ac.stop()
    finally:
        asrpkg.create = orig

    recs = [json.loads(x) for x in (BASE / "chunks" / "transcript.jsonl").read_text().strip().splitlines()]
    return {
        "segments": len(recs),
        "record_keys": sorted(recs[0]),
        "start_offsets": [r["start_offset"] for r in recs],
        "end_offsets": [r["end_offset"] for r in recs],
        "raw_bytes": (BASE / "chunks" / "audio.s16le").stat().st_size,
    }


def current_v1_schema() -> dict:
    """The daemon's /v1 JSON Schema (pydantic models) — the GUI contract firewall."""
    from capture_mcp.daemon.models import v1_schema
    from capture_mcp.daemon.server import API_VERSION

    return v1_schema(API_VERSION)


CONTRACTS = {
    "tools_list": current_tools_list,
    "session_dir": current_session_dir,
    "pcm_chunking": current_pcm_chunking,
    "v1_schema": current_v1_schema,
}


def main() -> int:
    regen = "--regen" in sys.argv
    try:
        for name, fn in CONTRACTS.items():
            cur = fn()
            path = GOLDEN / f"{name}.json"
            if regen:
                GOLDEN.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(cur, indent=2, sort_keys=True) + "\n")
                print(f"[REGEN] {name} -> {path}")
                continue
            want = json.loads(path.read_text())
            ok = cur == want
            detail = ""
            if not ok:
                cur_s = json.dumps(cur, indent=2, sort_keys=True).splitlines()
                want_s = json.dumps(want, indent=2, sort_keys=True).splitlines()
                diff = [f"-{w}|+{c}" for w, c in zip(want_s, cur_s) if w != c][:5]
                detail = "; ".join(diff) or f"line counts {len(want_s)} vs {len(cur_s)}"
            check(f"contract: {name}", ok, detail)
    finally:
        shutil.rmtree(BASE, ignore_errors=True)

    if regen:
        return 0
    failed = [r for r in results if not r[0]]
    print(f"\n{len(results) - len(failed)}/{len(results)} contracts hold")
    if failed:
        print("Contract drift! If the change was INTENTIONAL, update the spec, then: "
              "python tests/contract/run_contracts.py --regen")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
