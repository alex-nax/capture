#!/usr/bin/env python3
"""Drive a session index through the *daemon* /v1 index endpoint (the MCP tool's backend)
and wait on the SSE stream for completion. Copies the resulting index.json (and
index_prompts.json) out to a named path so successive runs don't clobber each other.

This is the build engine for every arm of the capture-index-eval routine: the same
script builds the `basic` (--preset auto), `custom` (--custom-json), and `dense`
(lower --sample-rate) indexes. `select_leaves` is deterministic: the SAME --sample-rate
yields the SAME leaf frames, so the baseline / basic / custom arms all line up frame-for-frame
and the scores are comparable.

Repo root is discovered robustly (env CAPTURE_REPO, else walk up to the dir holding
tools/capture_v1.py), so this travels to other machines / clones unchanged. The daemon is
now the native Rust `captured`; this proxies its /v1 index endpoint (no in-process engine).

Usage:
  drive_index.py --session SID --session-dir DIR --out OUT.json
                 --endpoint URL --model M --sample-rate R
                 [--max-leaves N] [--preset auto] [--custom-json custom_prompt.json]

  # basic auto index:
  drive_index.py --session SID --session-dir DIR --out basic/index.json \
                 --endpoint $CAPTURE_INDEX_URL --model qwen/qwen3.5-9b \
                 --sample-rate 0.5 --preset auto

  # custom prompt+schema index (content_type becomes "custom"):
  drive_index.py --session SID --session-dir DIR --out custom/index.json \
                 --endpoint $CAPTURE_INDEX_URL --model qwen/qwen3.5-9b \
                 --sample-rate 0.5 --custom-json custom_prompt.json
"""
from __future__ import annotations
import argparse, json, os, shutil, sys, time
from pathlib import Path


def find_repo() -> Path:
    """Locate the capture repo root (holds tools/capture_v1.py) so we can import the /v1 client.
    Priority: $CAPTURE_REPO -> walk up from this file -> walk up from cwd."""
    env = os.environ.get("CAPTURE_REPO")
    if env and (Path(env) / "tools" / "capture_v1.py").is_file():
        return Path(env)
    for start in (Path(__file__).resolve(), Path.cwd().resolve()):
        for p in (start, *start.parents):
            if (p / "tools" / "capture_v1.py").is_file():
                return p
    print("could not locate the capture repo root (no tools/capture_v1.py found); "
          "set CAPTURE_REPO=/path/to/capture", file=sys.stderr)
    raise SystemExit(2)


REPO = find_repo()
sys.path.insert(0, str(REPO / "tools"))
from capture_v1 import Daemon  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--session", required=True, help="FULL daemon session_id (the stamped one, not the short suffix)")
    ap.add_argument("--session-dir", required=True, help="that session's run dir (index.json is written there)")
    ap.add_argument("--out", required=True, help="where to copy the built index.json")
    ap.add_argument("--endpoint", required=True, help="vision model chat endpoint, e.g. $CAPTURE_INDEX_URL")
    ap.add_argument("--model", required=True, help="model id served at the endpoint, e.g. qwen/qwen3.5-9b")
    ap.add_argument("--sample-rate", type=float, required=True,
                    help="frames/sec to sample; LOWER = denser. Keep equal across arms for aligned leaves.")
    ap.add_argument("--max-leaves", type=int, default=512)
    ap.add_argument("--preset", default=None, help='built-in extractor, e.g. "auto"')
    ap.add_argument("--custom-json", default=None,
                    help='JSON file with {"leaf_prompt", "leaf_schema"} — overrides --preset')
    args = ap.parse_args()

    d = Daemon.discover()
    if not d or not d.available():
        print("daemon not available (is `captured` running? check ~/.capture/daemon.json)", file=sys.stderr)
        return 2

    leaf_prompt = leaf_schema = None
    preset = args.preset
    if args.custom_json:
        c = json.loads(Path(args.custom_json).read_text())
        leaf_prompt = c["leaf_prompt"]
        leaf_schema = c["leaf_schema"]
        preset = None  # custom prompt+schema takes precedence; content_type becomes "custom"

    # Kick off the build (returns immediately; 202 + SSE).
    ack = d.index(args.session, endpoint=args.endpoint, model=args.model,
                  sample_rate=args.sample_rate, max_leaves=args.max_leaves,
                  prompt_preset=preset, leaf_prompt=leaf_prompt, leaf_schema=leaf_schema)
    print(f"index started: {ack}", flush=True)
    if not ack.get("started"):
        print("not started (already indexing?)", file=sys.stderr)

    t0 = time.time()
    rc = 1
    try:
        for ev in d.events(timeout=1800):
            if ev.get("session_id") != args.session:
                continue
            t = ev.get("type")
            if t == "index":
                print(f"  [{time.time()-t0:5.0f}s] {ev.get('phase'):8} "
                      f"{ev.get('done')}/{ev.get('total')} ({ev.get('fraction')})", flush=True)
            elif t == "index_done":
                print(f"index_done in {time.time()-t0:.0f}s: "
                      f"{ev.get('node_count')} nodes / {ev.get('leaf_count')} leaves", flush=True)
                rc = 0
                break
            elif t == "index_error":
                print(f"index_error: {ev.get('error')}", file=sys.stderr, flush=True)
                rc = 1
                break
    except Exception as e:
        print(f"event stream ended: {e}", file=sys.stderr)

    # Copy artifacts out before the next run overwrites them in the session dir.
    sd = Path(args.session_dir)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    if (sd / "index.json").exists():
        shutil.copy2(sd / "index.json", out)
        print(f"copied index.json -> {out}")
    if (sd / "index_prompts.json").exists():
        dst = out.with_name(out.stem + "_prompts.json")
        shutil.copy2(sd / "index_prompts.json", dst)
        print(f"copied index_prompts.json -> {dst}")
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
