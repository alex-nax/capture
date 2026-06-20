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

This talks to the LM Studio vision endpoint DIRECTLY (NOT via the daemon). The v3 cutover
removed the `capture_mcp` Python package, so the three things this used to import from it
(frame listing, transcript loading, the CONTENT_PROMPTS presets, and the vision client)
are inlined below as small pure-stdlib replacements that mirror the Rust ports in
`crates/core/src/{time,frames,transcript}.rs` and `crates/index/src/{prompts,vision}.rs`.
"""

from __future__ import annotations

import argparse
import base64
import html
import json
import os
import sys
import time
import tomllib
import urllib.error
import urllib.request
import webbrowser
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# ---------------------------------------------------------------------------
# Frame listing — inline port of `crates/core/src/{time,frames}.rs`.
#
# Screenshots are `<session>/screenshots/<fs_stamp>.{png,jpg,jpeg}`, where the fs_stamp is
# an ISO-8601 UTC timestamp (millisecond precision) with `:` replaced by `-`, e.g.
# `2026-06-19T10-25-46.741Z`. Each frame's `.offset` is seconds from the session epoch
# (so frames line up with the transcript offsets); the epoch is recovered the same way the
# Rust does (transcript first record → session.json started_at → 0).
# ---------------------------------------------------------------------------

_IMAGE_EXTS = {"png", "jpg", "jpeg"}


class Frame:
    """A screenshot with its timeline position. `.path` is a str (hashable, used as a dict
    key by the eval), `.offset` is seconds from the first/epoch, `.iso` is the display ISO."""

    __slots__ = ("path", "stamp", "offset", "iso")

    def __init__(self, path: str, stamp: float, offset: float, iso: str):
        self.path = path
        self.stamp = stamp
        self.offset = offset
        self.iso = iso


def _parse_iso(s: str) -> float | None:
    """Parse an ISO-8601 `...Z` timestamp to a unix timestamp. Mirrors `retranscribe._parse_iso`
    / `frames::parse_iso` (`fromisoformat(s.replace("Z", "+00:00"))`). None on failure."""
    try:
        return datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()
    except (ValueError, TypeError):
        return None


def _parse_fs_stamp(stem: str) -> float | None:
    """Parse a `2026-06-16T22-01-13.146Z` screenshot stem back to a unix timestamp.
    Mirrors `time::parse_fs_stamp`. None on parse failure. The stem ends in a literal `Z`
    (UTC); parse the date+time with the `-`-separated time and attach UTC explicitly so
    `.timestamp()` is computed in UTC, not local time."""
    try:
        dt = datetime.strptime(stem, "%Y-%m-%dT%H-%M-%S.%fZ").replace(tzinfo=timezone.utc)
        return dt.timestamp()
    except ValueError:
        return None


def _display_iso(stem: str) -> str:
    """`2026-06-16T22-01-13.146Z` -> `2026-06-16T22:01:13.146Z` (only the time half).
    Mirrors `time::display_iso`: split on the first `T`, replace `-`→`:` in the time half."""
    if "T" not in stem:
        return stem
    date, _, t = stem.partition("T")
    return f"{date}T{t.replace('-', ':')}"


def _recover_epoch(session_dir: Path) -> float:
    """Audio first-byte wall-clock so frames align with the transcript. Port of
    `frames::recover_epoch`: transcript first record (start - start_offset) → session.json
    summary.started_at → 0."""
    tx = session_dir / "transcript.jsonl"
    if tx.is_file():
        for ln in tx.read_text().splitlines():
            try:
                rec = json.loads(ln)
            except (json.JSONDecodeError, ValueError):
                continue
            st = _parse_iso(rec["start"]) if isinstance(rec.get("start"), str) else None
            off = rec.get("start_offset")
            if st is not None and isinstance(off, (int, float)):
                return st - off
    meta = session_dir / "session.json"
    if meta.is_file():
        try:
            data = json.loads(meta.read_text())
            st = data.get("summary", {}).get("started_at")
            if isinstance(st, str):
                v = _parse_iso(st)
                if v is not None:
                    return v
        except (json.JSONDecodeError, ValueError, OSError):
            pass
    return 0.0


def list_frames(session_dir: Path) -> list[Frame]:
    """All screenshots in `session_dir`, oldest first, with timeline offsets. Frames whose
    name doesn't parse are skipped. Empty if there are no screenshots. Port of `list_frames`."""
    shots = session_dir / "screenshots"
    if not shots.is_dir():
        return []
    epoch = _recover_epoch(session_dir)
    out: list[Frame] = []
    for f in shots.iterdir():
        if not f.is_file():
            continue
        if f.suffix.lower().lstrip(".") not in _IMAGE_EXTS:
            continue
        ts = _parse_fs_stamp(f.stem)
        if ts is None:
            continue
        out.append(Frame(path=str(f), stamp=ts, offset=round(ts - epoch, 3), iso=_display_iso(f.stem)))
    out.sort(key=lambda fr: fr.stamp)
    return out


# ---------------------------------------------------------------------------
# Transcript loading — inline port of `crates/core/src/transcript.rs` (`load_transcript`).
# `<session>/transcript.jsonl`: one JSON object per line with `start_offset`, `end_offset`,
# and `text` (offsets are seconds on the session timeline). Returns a list of segment dicts.
# ---------------------------------------------------------------------------

def _load_transcript(session_dir: Path) -> list[dict]:
    """Transcript segments with offsets. Reads `<dir>/transcript.jsonl`; keeps records that
    have both `start_offset` and `text`; `end_offset` defaults to `start_offset`; text is
    trimmed. Malformed lines skipped. Empty if no file. Port of `load_transcript`."""
    out: list[dict] = []
    tx = session_dir / "transcript.jsonl"
    if not tx.is_file():
        return out
    for ln in tx.read_text().splitlines():
        try:
            rec = json.loads(ln)
        except (json.JSONDecodeError, ValueError):
            continue
        if "start_offset" not in rec or "text" not in rec:
            continue
        start_offset = float(rec.get("start_offset") or 0.0)
        end_offset = float(rec["end_offset"]) if isinstance(rec.get("end_offset"), (int, float)) else start_offset
        out.append({
            "start_offset": start_offset,
            "end_offset": end_offset,
            "text": str(rec.get("text") or "").strip(),
        })
    return out


# ---------------------------------------------------------------------------
# CONTENT_PROMPTS — inline build from `crates/index/src/prompts.toml`, mirroring how
# `crates/index/src/prompts.rs` builds the per-type (prompt, schema). Each `[[content]]`
# entry has `key`, `prompt`, and `fields = [[name, "str"|"strs"], ...]`. The json_schema is
# `{"type":"object","properties":{summary:str, <fields...>},"required":["summary"]}`.
# ---------------------------------------------------------------------------

_PROMPTS_TOML = ROOT / "crates" / "index" / "src" / "prompts.toml"


def _field_schema(kind: str) -> dict:
    """`"str"` → a plain string property; `"strs"` → an array-of-strings. Mirrors
    `prompts::str_field`/`strs_field` (any unexpected kind → plain string)."""
    if kind == "strs":
        return {"type": "array", "items": {"type": "string"}}
    return {"type": "string"}


def _schema_for(fields: list) -> dict:
    """Leaf-extraction json_schema: always a `summary` string first, then each field.
    Mirrors `prompts::schema_for`."""
    properties = {"summary": {"type": "string"}}
    for name, kind in fields:
        properties[name] = _field_schema(kind)
    return {"type": "object", "properties": properties, "required": ["summary"]}


def _load_content_prompts() -> dict:
    """Build `{key: {"prompt": str, "schema": dict}}` from `prompts.toml`. Trims the prompt
    of the leading/trailing newline the `'''…'''` block carries, matching `prompts.rs`."""
    with _PROMPTS_TOML.open("rb") as fh:
        cfg = tomllib.load(fh)
    out: dict[str, dict] = {}
    for entry in cfg["content"]:
        out[entry["key"]] = {
            "prompt": entry["prompt"].strip(),
            "schema": _schema_for(entry["fields"]),
        }
    return out


CONTENT_PROMPTS = _load_content_prompts()


# ---------------------------------------------------------------------------
# Vision client — inline pure-stdlib (urllib) port of `crates/index/src/vision.rs`.
#
# OpenAI-compatible `/v1/chat/completions` client pointed at LM Studio. The request shape
# MUST match the Rust: `model`, `messages=[{role:"user", content:[{type:text},
# {type:image_url, image_url:{url:"data:image/<mime>;base64,..."}}]}]`, `max_tokens`, and
# CRUCIALLY `reasoning_effort:"none"` (required for LM Studio structured output, which uses
# grammar-constrained sampling that forbids a reasoning `<think>` block). Structured calls
# add `response_format={"type":"json_schema","json_schema":{name,strict:true,schema}}`.
# ---------------------------------------------------------------------------

class VisionClient:
    def __init__(self, url: str, model: str | None, api_key: str | None,
                 timeout: float, max_tokens: int):
        self.url = url
        self.model = model
        self.api_key = api_key
        self.timeout = timeout
        self.max_tokens = max_tokens

    # -- availability ---------------------------------------------------------

    def _models_url(self) -> str:
        for suffix in ("/chat/completions", "/completions"):
            if self.url.endswith(suffix):
                return self.url[: -len(suffix)] + "/models"
        return self.url

    def available(self) -> bool:
        """True iff the endpoint answers `GET /v1/models`. Never raises — false on any failure."""
        req = urllib.request.Request(self._models_url(), method="GET")
        if self.api_key:
            req.add_header("Authorization", f"Bearer {self.api_key}")
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                return 200 <= resp.status < 300
        except Exception:
            return False

    # -- calls ----------------------------------------------------------------

    def _image_content(self, image_path, prompt: str) -> list:
        path = Path(image_path)
        ext = path.suffix.lower().lstrip(".")
        mime = "image/jpeg" if ext in ("jpg", "jpeg") else "image/png"
        b64 = base64.b64encode(path.read_bytes()).decode("ascii")
        return [
            {"type": "text", "text": prompt},
            {"type": "image_url", "image_url": {"url": f"data:{mime};base64,{b64}"}},
        ]

    def caption_image(self, image_path, prompt: str) -> str:
        """Vision call: describe `image_path` per `prompt`. Returns the model text."""
        content = self._image_content(image_path, prompt)
        return self._chat([{"role": "user", "content": content}], None)

    def structured_image(self, image_path, prompt: str, schema: dict) -> dict:
        """STRUCTURED vision call: returns a JSON object validated against `schema`
        (an OpenAI `json_schema` response_format). Returns `{}` on a non-JSON reply.
        `reasoning_effort:"none"` is REQUIRED for LM Studio structured output."""
        extra = {
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "extract", "strict": True, "schema": schema},
            },
            "reasoning_effort": "none",
        }
        content = self._image_content(image_path, prompt)
        text = self._chat([{"role": "user", "content": content}], extra)
        try:
            v = json.loads(text)
        except (json.JSONDecodeError, ValueError):
            return {}
        return v if isinstance(v, dict) else {}

    def _chat(self, messages: list, extra: dict | None) -> str:
        payload = {
            "model": self.model or "local",
            "messages": messages,
            "temperature": 0.2,
            "stream": False,
            "max_tokens": self.max_tokens,
        }
        if extra:
            payload.update(extra)
        data = json.dumps(payload).encode("utf-8")
        req = urllib.request.Request(self.url, data=data, method="POST")
        req.add_header("Content-Type", "application/json")
        if self.api_key:
            req.add_header("Authorization", f"Bearer {self.api_key}")
        with urllib.request.urlopen(req, timeout=self.timeout) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        return _extract_text(body)


def _extract_text(payload: dict) -> str:
    """Pull the assistant text out of an OpenAI chat-completions response. `content` may be a
    string or a list of parts (`[{type,text}]`); join the parts' `text`. "" on shape error.
    Mirrors `vision::extract_text`."""
    try:
        content = payload["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError):
        return ""
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        return "".join(p.get("text", "") for p in content if isinstance(p, dict)).strip()
    return ""


def load(endpoint: str | None, model: str | None) -> VisionClient:
    """Build a vision client from explicit args falling back to env. Endpoint/model come from
    `--endpoint`/`--model` or `CAPTURE_INDEX_URL`/`CAPTURE_INDEX_MODEL`; key from
    `CAPTURE_INDEX_KEY` (optional Bearer). Mirrors `vision::load`."""
    url = (endpoint or os.environ.get("CAPTURE_INDEX_URL") or "").strip()
    if not url:
        raise RuntimeError(
            "CAPTURE_INDEX_URL is not set (e.g. http://192.168.31.217:1234/v1/chat/completions)"
        )
    model = model or os.environ.get("CAPTURE_INDEX_MODEL") or None
    api_key = os.environ.get("CAPTURE_INDEX_KEY") or None
    timeout = float(os.environ.get("CAPTURE_INDEX_TIMEOUT") or 120.0)
    max_tokens = int(os.environ.get("CAPTURE_INDEX_MAX_TOKENS") or 2048)
    return VisionClient(url, model, api_key, timeout, max_tokens)


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
    ap.add_argument("--preset", help=f"One preset: {', '.join(CONTENT_PROMPTS)}")
    ap.add_argument("--prompt", help="A custom leaf prompt (inline text, or @path to a file)")
    ap.add_argument("--compare", action="store_true", help="Run ALL presets side by side")
    ap.add_argument("--endpoint", help="LM Studio chat URL (else CAPTURE_INDEX_URL)")
    ap.add_argument("--model", help="model id (else CAPTURE_INDEX_MODEL)")
    ap.add_argument("--out", help="HTML report path (default: alongside the session)")
    ap.add_argument("--no-open", action="store_true", help="Don't open the report in a browser")
    args = ap.parse_args()

    session = Path(args.session).expanduser()
    frames = list_frames(session)
    if not frames:
        print(f"no screenshots in {session}", file=sys.stderr)
        return 2
    segments = _load_transcript(session)
    # End-offset of each frame (next frame's start, or +5s for the last), keyed by path.
    leaf_end = {frames[i].path: (frames[i + 1].offset if i + 1 < len(frames) else frames[i].offset + 5)
                for i in range(len(frames))}
    picked = sample_frames(frames, args.n)

    # Which prompt(s)/schema(s) to test. Each candidate is (name, prompt, schema_or_None).
    if args.compare:
        candidates = [(k, v["prompt"], v["schema"]) for k, v in CONTENT_PROMPTS.items()]
    elif args.prompt:
        text = args.prompt
        if text.startswith("@"):
            text = Path(text[1:]).expanduser().read_text()
        candidates = [("custom", text, None)]  # free-text caption (no structured schema)
    else:
        preset = args.preset or "general"
        p = CONTENT_PROMPTS[preset]
        candidates = [(preset, p["prompt"], p["schema"])]

    client = load(args.endpoint, args.model)
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
