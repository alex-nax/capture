"""Hermetic test for the multimodal index (#44): fake vision server + synthetic session.

No network, no real model, no permissions. Spins up a local HTTP server that mimics an
LM Studio OpenAI endpoint (canned captions/summaries), builds an index over synthetic
screenshots + a transcript, and asserts the tree shape, raw artifacts, transcript fusion,
and checkpoint-resume behaviour.

Run: ``.venv/bin/python tests/indexing_hermetic.py`` → expect ``ALL PASSED``.
"""

from __future__ import annotations

import json
import sys
import tempfile
import threading
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from capture_mcp.core import frames as frames_mod  # noqa: E402
from capture_mcp.core import indexer  # noqa: E402
from capture_mcp.core import vision_client  # noqa: E402

# Image bytes only need a PNG signature; content is ignored (max_px=0 skips sips, the
# fake server doesn't decode the image).
_PNG_1x1 = b"\x89PNG\r\n\x1a\n" + b"\x00fake-frame-bytes"

_calls = {"caption": 0, "combine": 0}


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):  # quiet
        pass

    def do_GET(self):
        if self.path.endswith("/models"):
            self._json(200, {"data": [{"id": "fake-vlm"}]})
        else:
            self._json(404, {"error": "nope"})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        msg = body["messages"][-1]["content"]
        if "response_format" in body:  # a STRUCTURED leaf-extraction call → return JSON
            _calls["caption"] += 1
            text = json.dumps({"summary": f"CAPTION#{_calls['caption']}", "content_type": "general"})
        else:  # a free-text combine call
            _calls["combine"] += 1
            has_tx = "TX:" in (msg if isinstance(msg, str) else "")
            text = f"SUMMARY#{_calls['combine']}" + (" [+tx]" if has_tx else "")
        self._json(200, {"choices": [{"message": {"role": "assistant", "content": text}}]})

    def _json(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def _fs_stamp(ts: float) -> str:
    dt = datetime.fromtimestamp(ts, tz=timezone.utc)
    return (dt.strftime("%Y-%m-%dT%H:%M:%S.") + f"{dt.microsecond // 1000:03d}Z").replace(":", "-")


def _make_session(d: Path, n_frames: int, base: float) -> None:
    shots = d / "screenshots"
    shots.mkdir(parents=True)
    for i in range(n_frames):
        (shots / f"{_fs_stamp(base + i)}.png").write_bytes(_PNG_1x1)
    # A transcript spanning the timeline (offsets in seconds since base).
    with (d / "transcript.jsonl").open("w") as f:
        for i in range(n_frames):
            f.write(json.dumps({
                "start": "", "end": "",
                "start_offset": float(i), "end_offset": float(i) + 0.9,
                "text": f"TX:said-{i}",
            }) + "\n")
    (d / "session.json").write_text(json.dumps({
        "config": {"audio_source": "import"},
        "summary": {"started_at": datetime.fromtimestamp(base, tz=timezone.utc)
                    .strftime("%Y-%m-%dT%H:%M:%S.000Z")},
    }))


results = []


def check(name, ok, detail=""):
    results.append(ok)
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail and not ok else ""))


def main() -> int:
    srv = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    port = srv.server_address[1]
    url = f"http://127.0.0.1:{port}/v1/chat/completions"
    client = vision_client.VisionClient(url, model="fake-vlm", api_key=None, timeout=10, max_px=0)

    check("available() preflight", client.available(), "GET /v1/models should 200")

    base = 1_750_000_000.0
    with tempfile.TemporaryDirectory() as td:
        d = Path(td) / "capture-x"
        _make_session(d, n_frames=8, base=base)

        # rate 1.0 → all 8 frames are leaves; tree has 2n-1 = 15 nodes.
        fr = frames_mod.list_frames(d)
        check("list_frames count", len(fr) == 8, f"got {len(fr)}")
        check("frame offsets aligned", fr[0].offset == 0.0 and fr[3].offset == 3.0,
              f"{fr[0].offset},{fr[3].offset}")

        leaves = frames_mod.select_leaves(fr, sample_rate=0.5, max_leaves=512)
        check("sample_rate 0.5 → every other (+end)", len(leaves) in (4, 5), f"got {len(leaves)}")

        idx = indexer.build_index(d, client, sample_rate=1.0, max_leaves=512,
                                  fuse_transcript=True, prompt_preset="general", model_label="fake-vlm")
        check("node_count = 2n-1", idx["node_count"] == 15, str(idx["node_count"]))
        check("complete flag", idx.get("complete") is True)
        check("index.json written", (d / "index.json").is_file())
        check("index_summary.txt written", (d / "index_summary.txt").is_file())
        check("root has summary", bool(idx["root_summary"]))

        nodes = {nd["id"]: nd for nd in idx["nodes"]}
        leaves_n = [nd for nd in idx["nodes"] if not nd["children"]]
        internal = [nd for nd in idx["nodes"] if nd["children"]]
        check("8 leaves captioned", len(leaves_n) == 8 and all(nd["vision_caption"] for nd in leaves_n),
              f"{len(leaves_n)} leaves")
        check("vision only at leaves", all(nd["vision_caption"] is None for nd in internal))
        check("internal combines fused transcript", all("[+tx]" in nd["summary"] for nd in internal))
        check("raw transcript_slice kept", all(nd["transcript_slice"] for nd in leaves_n))
        check("parents linked", nodes["0-3"]["parent"] == "0-7" and nodes["0-7"]["parent"] is None)
        check("call counts: 8 vision + 7 text", _calls["caption"] == 8 and _calls["combine"] == 7,
              f"v={_calls['caption']} t={_calls['combine']}")

        # --- checkpoint resume: drop the root summary, rebuild → only 1 new combine call.
        v0, t0 = _calls["caption"], _calls["combine"]
        data = json.loads((d / "index.json").read_text())
        for nd in data["nodes"]:
            if nd["id"] == data["root_id"]:
                nd["summary"] = ""  # simulate the root not yet done
        data["complete"] = False
        (d / "index.json").write_text(json.dumps(data))
        idx2 = indexer.build_index(d, client, sample_rate=1.0, max_leaves=512,
                                   fuse_transcript=True, prompt_preset="general", model_label="fake-vlm")
        check("resume skips done nodes (0 new captions)", _calls["caption"] == v0)
        check("resume recomputes only the missing node", _calls["combine"] == t0 + 1,
              f"combine delta {_calls['combine'] - t0}")
        check("resumed index complete", idx2.get("complete") is True and bool(idx2["root_summary"]))

    srv.shutdown()
    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    if passed == len(results):
        print("ALL PASSED")
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
