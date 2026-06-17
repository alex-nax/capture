#!/usr/bin/env python3
"""Eval harness for the multimodal-index PROMPTS (#44/#32).

A universal per-frame prompt doesn't work — what extracts speaker names + the active
speaker in a *meeting* is the wrong prompt for a *lecture* or *gameplay*. This tool lets
you tune prompts empirically: it samples frames from a real session, captions each with
one or more candidate prompts (presets or custom), and writes an HTML report laying the
frame, its transcript slice, and the caption(s) side by side so you can judge whether the
prompt pulls out what you want. Iterate the prompt, re-run, compare.

Usage:
  # compare all built-in presets on 8 frames of a session
  python tools/index_prompt_eval.py --session ~/.capture/runs/capture-… --compare --n 8

  # test one preset
  python tools/index_prompt_eval.py --session <dir> --preset meeting --n 10

  # test a custom prompt (inline or @file)
  python tools/index_prompt_eval.py --session <dir> --prompt "List every name you can read." --n 6

Endpoint/model come from --endpoint/--model or the CAPTURE_INDEX_* env. Output: an HTML
report (opened in the browser unless --no-open) + a JSON dump for diffing across runs.
"""

from __future__ import annotations

import argparse
import html
import json
import os
import sys
import time
import webbrowser
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from capture_mcp.core import frames as frames_mod  # noqa: E402
from capture_mcp.core import indexer, vision_client  # noqa: E402


def sample_frames(frames, n):
    if n >= len(frames):
        return frames
    return [frames[round(i * (len(frames) - 1) / (n - 1))] for i in range(n)]


def transcript_slice(segments, lo, hi):
    return " ".join(s["text"] for s in segments if s["text"] and s["end_offset"] > lo and s["start_offset"] < hi)


def main() -> int:
    ap = argparse.ArgumentParser(description="Tune multimodal-index prompts on real frames.")
    ap.add_argument("--session", required=True, help="A capture session dir (has screenshots/ + transcript)")
    ap.add_argument("--n", type=int, default=8, help="Number of frames to sample")
    ap.add_argument("--preset", help=f"One preset: {', '.join(indexer.CONTENT_PROMPTS)}")
    ap.add_argument("--prompt", help="A custom leaf prompt (inline text, or @path to a file)")
    ap.add_argument("--compare", action="store_true", help="Run ALL presets side by side")
    ap.add_argument("--endpoint", help="LM Studio chat URL (else CAPTURE_INDEX_URL)")
    ap.add_argument("--model", help="model id (else CAPTURE_INDEX_MODEL)")
    ap.add_argument("--out", help="HTML report path (default: alongside the session)")
    ap.add_argument("--no-open", action="store_true", help="Don't open the report in a browser")
    args = ap.parse_args()

    session = Path(args.session).expanduser()
    frames = frames_mod.list_frames(session)
    if not frames:
        print(f"no screenshots in {session}", file=sys.stderr)
        return 2
    segments = indexer._load_transcript(session)
    # End-offset of each frame (next frame's start, or +5s for the last), keyed by path.
    leaf_end = {frames[i].path: (frames[i + 1].offset if i + 1 < len(frames) else frames[i].offset + 5)
                for i in range(len(frames))}
    picked = sample_frames(frames, args.n)

    # Which prompt(s)/schema(s) to test. Each candidate is (name, prompt, schema_or_None).
    if args.compare:
        candidates = [(k, v["prompt"], v["schema"]) for k, v in indexer.CONTENT_PROMPTS.items()]
    elif args.prompt:
        text = args.prompt
        if text.startswith("@"):
            text = Path(text[1:]).expanduser().read_text()
        candidates = [("custom", text, None)]  # free-text caption (no structured schema)
    else:
        preset = args.preset or "general"
        p = indexer.CONTENT_PROMPTS[preset]
        candidates = [(preset, p["prompt"], p["schema"])]

    client = vision_client.load(args.endpoint, args.model)
    if not client.available():
        print("index endpoint not reachable — set --endpoint / CAPTURE_INDEX_URL", file=sys.stderr)
        return 3
    print(f"endpoint ok (model={client.model}); extracting {len(picked)} frames × {len(candidates)} prompt(s)…")

    rows = []
    for fr in picked:
        tslice = transcript_slice(segments, fr.offset, leaf_end[fr.path])
        caps = {}
        for name, prompt, schema in candidates:
            t = time.time()
            try:
                if schema is not None:
                    out = client.structured_image(fr.path, prompt, schema)
                    cap = json.dumps(out, ensure_ascii=False, indent=2) if out else "(EMPTY)"
                else:
                    cap = client.caption_image(fr.path, prompt)
            except Exception as e:
                cap = f"(error: {e})"
            caps[name] = {"caption": cap, "secs": round(time.time() - t, 1)}
            print(f"  [{fr.offset:+.0f}s] {name} ({caps[name]['secs']}s): {cap[:80].replace(chr(10), ' ')}")
        rows.append({"offset": fr.offset, "iso": fr.iso, "path": str(fr.path), "transcript": tslice, "caps": caps})

    out = Path(args.out) if args.out else session.parent / f"index-prompt-eval-{session.name}.html"
    out.write_text(_render_html(session.name, [c[0] for c in candidates], rows))
    (out.with_suffix(".json")).write_text(json.dumps({"session": session.name, "candidates": [c[0] for c in candidates], "rows": rows}, indent=2, ensure_ascii=False))
    print(f"\nreport: {out}")
    empties = sum(1 for r in rows for c in r["caps"].values() if not c["caption"].strip())
    if empties:
        print(f"⚠ {empties} empty caption(s) — raise CAPTURE_INDEX_MAX_TOKENS or check the model")
    if not args.no_open:
        webbrowser.open(f"file://{out}")
    return 0


def _render_html(session, names, rows) -> str:
    head = (
        "<!doctype html><meta charset=utf-8><title>index prompt eval</title>"
        "<style>body{font:14px -apple-system,sans-serif;margin:24px;background:#16181c;color:#e0e0e0}"
        "h1{font-size:18px}.row{display:flex;gap:16px;margin:18px 0;padding:14px;background:#1e2127;border-radius:10px}"
        "img{width:320px;height:auto;border-radius:6px;border:1px solid #2a2a2a}"
        ".col{flex:1}.tx{color:#8ab4f8;font-size:13px;margin:6px 0;white-space:pre-wrap}"
        ".cap{background:#23262b;border-radius:6px;padding:8px;margin:6px 0;white-space:pre-wrap;font-family:ui-monospace,monospace;font-size:12px}.name{color:#e0c063;font-weight:600}"
        ".t{color:#6a6a6a;font-size:12px}</style>"
        f"<h1>Index prompt eval — {html.escape(session)} · prompts: {', '.join(html.escape(n) for n in names)}</h1>"
    )
    body = []
    for r in rows:
        caps = "".join(
            f"<div class=cap><span class=name>{html.escape(n)}</span> <span class=t>{r['caps'][n]['secs']}s</span>"
            f"<div>{html.escape(r['caps'][n]['caption']) or '<i>(empty)</i>'}</div></div>"
            for n in names
        )
        body.append(
            f"<div class=row><div><img src='file://{html.escape(r['path'])}'><div class=t>{html.escape(r['iso'])} ({r['offset']:+.0f}s)</div></div>"
            f"<div class=col><div class=tx><b>transcript:</b> {html.escape(r['transcript']) or '<i>(none)</i>'}</div>{caps}</div></div>"
        )
    return head + "".join(body)


if __name__ == "__main__":
    raise SystemExit(main())
