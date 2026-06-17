"""Hierarchical multimodal index of a session's screenshots (feature #44).

Builds a balanced **binary tree** over the timeline: split the leaf frames at their
midpoint recursively, **caption each leaf** with the remote vision model (descent), then
**combine children up to a root summary** (conquer), fusing the time-aligned transcript at
each combine. Every node keeps its raw artifacts (vision caption, transcript slice) beside
the fused summary, so the index is inspectable and re-combinable. The build is
checkpointed to ``index.json`` after every node, so a dropped LAN connection resumes.

Design + decisions: docs/specs/indexing.md. Vision runs only at leaves (D2); the
transcript is fused into combines (D3) but capped in length so the root combine stays
bounded; the full slice is stored raw regardless.
"""

from __future__ import annotations

import json
import logging
import os
import re
from pathlib import Path

from . import frames as frames_mod
from .util import iso, now

log = logging.getLogger(__name__)

INDEX_VERSION = 1
TRANSCRIPT_FEED_CAP = 1500  # max chars of transcript fed to a single combine call

# What's good for a meeting is wrong for a lecture or gameplay, so the per-frame DESCRIPTION
# prompt is chosen per content type. In "auto" mode the indexer first CLASSIFIES each frame
# (structured output, enum content_type) and routes to the matching prompt below; a fixed
# preset (e.g. "meeting") skips classification. Each entry pairs a leaf prompt with a
# `combine_focus` line that steers the range summaries. Tune with tools/index_prompt_eval.py.
def _schema(props: dict) -> dict:
    """A leaf-extraction json_schema: always a ``summary`` string + the type-specific fields."""
    return {
        "type": "object",
        "properties": {"summary": {"type": "string"}, **props},
        "required": ["summary"],
    }


_STR = {"type": "string"}
_STRS = {"type": "array", "items": {"type": "string"}}

# Each content type pairs an EXTRACTION prompt + json_schema (structured output — every schema
# has a `summary` plus type-specific structured fields) with a `combine_focus` that steers the
# range summaries. In "auto" mode the indexer first CLASSIFIES each frame (structured, enum
# content_type) and routes to the matching schema below; a fixed preset skips classification.
# All extraction calls use reasoning_effort=none (the LM Studio structured-output fix).
CONTENT_PROMPTS: dict[str, dict] = {
    "general": {
        "label": "General",
        "prompt": "Describe this screenshot. Put a 1-2 sentence factual description in `summary`; the app/site "
                  "in `app`; and any salient on-screen text/names you can read verbatim in `on_screen_text`.",
        "schema": _schema({"app": _STR, "on_screen_text": _STRS}),
        "combine_focus": "what happened, in order, and the salient topics, entities, and on-screen text",
    },
    "meeting": {
        "label": "Meeting / call",
        "prompt": "This is a video meeting (Google Meet / Zoom / Teams), often with a SHARED SCREEN (doc, "
                  "slide, task board). Read the participant NAMES verbatim from the tile name-labels into "
                  "`participants`. Set `active_speaker` to the name of the person whose tile is highlighted/"
                  "outlined/enlarged or shows a speaking indicator (empty string if no cue is visible — do NOT "
                  "guess). Put the shared screen/slide/board text verbatim in `shared_content`. Capture the WORK "
                  "CONTENT: `task_assignments` = any '<owner>: <task>' assignments visible on a board/doc or shown "
                  "in this frame; `data_points` = concrete data (ticket refs, dates/deadlines, project/initiative "
                  "names, statuses, metrics); `decisions` = any decisions or action items evident. Leave a field "
                  "empty if nothing supports it. `summary` = a 1-2 sentence description naming who is speaking. "
                  "Do not invent names, owners, or tasks.",
        "schema": _schema({"participants": _STRS, "active_speaker": _STR, "shared_content": _STR,
                           "task_assignments": _STRS, "data_points": _STRS, "decisions": _STRS}),
        "combine_focus": "WHO said or did WHAT (attribute to the named speakers), task assignments (owner→task), "
                         "decisions, action items, ticket refs/dates, and topics, in order",
    },
    "lecture": {
        "label": "Lecture / tutorial",
        "prompt": "This is an educational video / screencast / tutorial / explainer (often inside a video player). "
                  "`summary` = 1-2 sentences on what is being taught. `topic` = the slide title or current topic. "
                  "`key_points` = key terms, definitions, or takeaways (verbatim where shown). `code` = any source "
                  "code on screen transcribed verbatim (else \"\"). `formulas` = any equations/formulas shown "
                  "verbatim (else []).",
        "schema": _schema({"topic": _STR, "key_points": _STRS, "code": _STR, "formulas": _STRS}),
        "combine_focus": "the concepts taught and how the material progresses, with key terms, code, formulas, and definitions",
    },
    "coding": {
        "label": "Coding / IDE",
        "prompt": "This is a code editor / IDE (possibly shown inside a video player). `summary` = 1-2 sentences on "
                  "the task. `language` = the programming language. `file` = the open file name (read from the tab/"
                  "title bar). `code` = the visible source code transcribed VERBATIM — preserve identifiers, "
                  "signatures, and structure exactly; do not paraphrase or invent; leave \"\" if illegible. "
                  "`symbols` = key function/class/identifier names or errors (verbatim).",
        "schema": _schema({"language": _STR, "file": _STR, "code": _STR, "symbols": _STRS}),
        "combine_focus": "the coding task, the files and the actual code involved, and the changes or problems",
    },
    "terminal": {
        "label": "Terminal",
        "prompt": "This is a terminal / console. `summary` = 1-2 sentences on the task. `commands` = the commands "
                  "run and any salient output/errors (verbatim).",
        "schema": _schema({"commands": _STRS}),
        "combine_focus": "the commands run and their outcomes",
    },
    "browsing": {
        "label": "Web browsing",
        "prompt": "This is a web browser (not a video call). `summary` = 1-2 sentences on the page/content. "
                  "`site` = the site or page title (and URL if visible). `headings` = visible headings (verbatim).",
        "schema": _schema({"site": _STR, "headings": _STRS}),
        "combine_focus": "the pages/sites visited and what was read or done",
    },
    "video": {
        "label": "Video / media",
        "prompt": "This is a video / media player (e.g. YouTube). `summary` = 1-2 sentences on what's on screen. "
                  "`title` = the video title (verbatim). `channel` = the channel/uploader if shown.",
        "schema": _schema({"title": _STR, "channel": _STR}),
        "combine_focus": "what the video showed, in order, and its topics",
    },
    "gameplay": {
        "label": "Game",
        "prompt": "This is a video game frame. `summary` = 1-2 sentences on what's happening. `game` = the game "
                  "name if identifiable. `scene` = the scene/level/mode and any salient HUD text.",
        "schema": _schema({"game": _STR, "scene": _STR}),
        "combine_focus": "the gameplay progression, objectives, and events",
    },
    "document": {
        "label": "Document",
        "prompt": "This is a document / text editor (Docs, Word, PDF, Notion). `summary` = 1-2 sentences on the "
                  "content. `title` = the document title. `section` = the visible heading/section (verbatim).",
        "schema": _schema({"title": _STR, "section": _STR}),
        "combine_focus": "the document's content and any edits",
    },
    "design": {
        "label": "Design tool",
        "prompt": "This is a design / creative tool (Figma, Sketch, Photoshop). `summary` = 1-2 sentences on what "
                  "is being designed. `tool` = the app. `elements` = salient layers/elements/labels visible.",
        "schema": _schema({"tool": _STR, "elements": _STRS}),
        "combine_focus": "the design work and its elements",
    },
}
#: The enum the classifier picks from (everything but the "general" fallback).
CONTENT_TYPES = [k for k in CONTENT_PROMPTS if k != "general"] + ["other"]
DEFAULT_PRESET = "auto"

#: Content types whose small, dense text benefits from a higher-resolution extraction pass.
#: The daf420 study (controlled: same frames, only max_px) showed code_fidelity 0.42→0.88 and
#: filename accuracy 0.30→0.95 going from 1024 to 2048 — resolution, not sampling, fixes the
#: systematic small-font OCR errors. Classification + non-code stay at the base max_px (cheaper).
CODE_TYPES = {"coding", "terminal"}


def _code_max_px() -> int:
    """Longest-edge downscale to use for code/terminal extraction (default 2048; base is 1024)."""
    try:
        return int(os.environ.get("CAPTURE_INDEX_CODE_MAX_PX", "2048"))
    except ValueError:
        return 2048


#: Content types whose leaves carry code worth reliability-flagging (#51).
_CODE_RELIABILITY_TYPES = CODE_TYPES | {"custom", "lecture"}
_NUM_RE = re.compile(r"-?\d+(?:\.\d+)?")
#: CamelCase / ALL_CAPS / dotted identifiers spoken in narration (likely dictated tokens).
_IDENT_RE = re.compile(r"\b(?:[A-Z][a-z0-9]+){2,}\b|\b[A-Z][A-Z0-9_]{2,}\b|\b\w+\.\w+\b")


def _narration_values(text: str) -> list[str]:
    """Candidate DICTATED tokens spoken in the narration over a frame — numbers + identifier-like
    words — that a consumer should PREFER over OCR'd code (the 1c0c0d finding: a narrator who speaks
    note values/names lets you recover tokens the OCR garbled). Heuristic, deterministic."""
    if not text:
        return []
    out, seen = [], set()
    for v in _NUM_RE.findall(text)[:14] + _IDENT_RE.findall(text)[:10]:
        if v not in seen:
            seen.add(v)
            out.append(v)
    return out[:18]


def _flag_code_reliability(nodes: dict) -> int:
    """#51 (flag-now half): on code leaves, attach `narration_values` (dictated tokens from the
    transcript) and set `ocr_uncertain` where the OCR'd file name DISAGREES across frames — a
    file/asset seen only once amid several differently-named code leaves is the local model's
    confabulation signature (the eval study: it invented a different class every frame). The flagged
    `repr_frame` paths are exactly what a frontier consumer should re-read (the auto-re-read is the
    deferred second half). Returns the count flagged uncertain."""
    from collections import Counter

    code_leaves = [nd for nd in nodes.values()
                   if not nd["children"] and nd.get("content_type") in _CODE_RELIABILITY_TYPES]
    if not code_leaves:
        return 0
    files = Counter()
    for nd in code_leaves:
        d = nd.get("data") or {}
        f = (d.get("file") or d.get("file_or_asset") or "").strip().lower()
        if f:
            files[f] += 1
    flagged = 0
    for nd in code_leaves:
        d = nd.get("data") or {}
        nv = _narration_values(nd.get("transcript_slice") or "")
        if nv:
            d["narration_values"] = nv
        f = (d.get("file") or d.get("file_or_asset") or "").strip().lower()
        # A singleton file name amid ≥3 distinct names across ≥4 code leaves ⇒ likely confabulated.
        if f and files[f] <= 1 and len(files) >= 3 and len(code_leaves) >= 4:
            d["ocr_uncertain"] = True
            flagged += 1
        nd["data"] = d
    return flagged

CLASSIFY_PROMPT = (
    "Classify what this screenshot PRIMARILY shows: set `content_type` to the single best fit and `app` to the "
    "application/site in focus. Classify by the CONTENT on screen, NOT the window around it — a screen recording "
    "or a YouTube/Twitch video OF an IDE or code is `coding`, OF a slide-based tutorial/explainer is `lecture`, "
    "OF a video call is `meeting`, OF a document/spreadsheet is `document`. Use `video` ONLY when the media itself "
    "is the subject (a film, vlog, music or gameplay footage) with nothing to read or extract. "
    "(meeting = a video call/conference; lecture = anything that teaches — a tutorial/explainer/screencast, even "
    "inside a video player; coding = an IDE/code editor, even inside a video player; terminal = a console; video = "
    "entertainment media with no code/slides/meeting/document to extract; browsing = a web page that is NOT a call; "
    "document = docs/notes/PDF; design = Figma/Photoshop.)"
)


def _classify_schema() -> dict:
    return {
        "type": "object",
        "properties": {"content_type": {"type": "string", "enum": CONTENT_TYPES}, "app": _STR},
        "required": ["content_type"],
    }


def _content_prompts(content_type: str) -> dict:
    """The extraction prompt + schema + combine focus for a content type ("other"/unknown → general)."""
    return CONTENT_PROMPTS.get(content_type if content_type in CONTENT_PROMPTS else "general")


def _combine_prompt(left: str, right: str, transcript: str, focus: str) -> str:
    return (
        "You are building a hierarchical summary of a screen-recording session. Below are "
        "summaries of two consecutive time ranges, plus the transcript of what was said during "
        f"the combined range. Write a concise summary (2-4 sentences) of the COMBINED range, "
        f"capturing {focus}.\n\n"
        f"EARLIER RANGE:\n{left}\n\nLATER RANGE:\n{right}\n\n"
        f"TRANSCRIPT (may be empty):\n{transcript or '(none)'}\n"
    )


def _load_transcript(session_dir: Path) -> list[dict]:
    """Transcript segments with offsets: ``[{start_offset, end_offset, text}]``."""
    out: list[dict] = []
    for name in ("transcript.jsonl",):
        p = session_dir / name
        if not p.is_file():
            continue
        for ln in p.read_text(encoding="utf-8").splitlines():
            try:
                rec = json.loads(ln)
            except Exception:
                continue
            if "start_offset" in rec and "text" in rec:
                out.append({
                    "start_offset": float(rec.get("start_offset", 0.0)),
                    "end_offset": float(rec.get("end_offset", rec.get("start_offset", 0.0))),
                    "text": str(rec.get("text", "")).strip(),
                })
    return out


def _transcript_slice(segments: list[dict], lo: float, hi: float) -> str:
    """Concatenate the transcript text of segments overlapping ``[lo, hi)`` (offsets)."""
    parts = [s["text"] for s in segments if s["text"] and s["end_offset"] > lo and s["start_offset"] < hi]
    return " ".join(parts).strip()


def build_index(
    session_dir: "str | Path",
    client,
    *,
    sample_rate: float = 0.5,
    max_leaves: int = 512,
    fuse_transcript: bool = True,
    prompt_preset: str | None = None,
    leaf_prompt: str | None = None,
    leaf_schema: dict | None = None,
    classify_prompt: str | None = None,
    code_max_px: int | None = None,
    model_label: str | None = None,
    on_progress=None,
) -> dict:
    """Build (or resume) the index for ``session_dir``; returns the index dict (also
    written to ``index.json`` + ``index_summary.txt``).

    ``client`` is a ``vision_client.VisionClient`` (``caption_image`` / ``combine``).
    ``on_progress(phase, done, total, t_range)`` is called per node — phase ``caption``
    (a leaf) or ``combine`` (an internal node). Raises ``ValueError`` if there are no
    screenshots to index."""
    d = Path(session_dir)
    all_frames = frames_mod.list_frames(d)
    leaves = frames_mod.select_leaves(all_frames, sample_rate, max_leaves)
    if not leaves:
        raise ValueError("no screenshots to index")

    segments = _load_transcript(d) if fuse_transcript else []
    n = len(leaves)
    total_nodes = 2 * n - 1
    import os

    preset = prompt_preset or DEFAULT_PRESET
    # "auto": CLASSIFY each frame (structured enum) then run that type's STRUCTURED extraction.
    # A fixed preset (e.g. "meeting") skips classification. Custom prompts (typically crafted by a
    # frontier model calling capture_index, executed cheaply by the LOCAL model):
    #   • custom_leaf + leaf_schema → a custom STRUCTURED extractor (one schema for every frame).
    #   • custom_leaf alone        → a custom free-text caption.
    #   • classify_prompt          → overrides the auto classifier's prompt.
    custom_leaf = (leaf_prompt.strip() if leaf_prompt and leaf_prompt.strip() else "") \
        or os.environ.get("CAPTURE_INDEX_LEAF_PROMPT", "").strip() or None
    custom_struct = bool(custom_leaf and leaf_schema)
    classify_prompt_used = (classify_prompt or "").strip() or CLASSIFY_PROMPT
    auto = preset == "auto" and not custom_leaf
    cmpx = code_max_px or _code_max_px()  # higher-res extraction for code/terminal leaves
    fixed_focus = None if (auto or custom_leaf) else _content_prompts(preset)["combine_focus"]
    params = {
        "sample_rate": sample_rate, "max_leaves": max_leaves, "fuse_transcript": fuse_transcript,
        "prompt_preset": preset, "leaf_prompt": custom_leaf, "leaf_schema": leaf_schema,
    }

    # Per-leaf end offset (the span a leaf represents): the next leaf's offset, or +inf
    # for the last, so trailing transcript is still captured at the right edge.
    leaf_end = [leaves[i + 1].offset for i in range(n - 1)] + [float("inf")]

    existing = _load_checkpoint(d, params, model_label)  # id -> prior node (reused if it has a summary)
    nodes: dict[str, dict] = {}
    done = [0]
    backup_done = [False]

    def progress(phase: str, t_lo: float, t_hi: float) -> None:
        done[0] += 1
        if on_progress:
            try:
                on_progress(phase, done[0], total_nodes, [t_lo, t_hi])
            except Exception:
                pass

    def visit(lo: int, hi: int, depth: int) -> dict:
        nid = f"{lo}-{hi}"
        mid = (lo + hi) // 2
        repr_leaf = leaves[mid]
        t_lo = leaves[lo].offset
        t_hi = leaf_end[hi]
        cached = existing.get(nid)
        if lo == hi:  # leaf — classify (auto), then STRUCTURED extraction
            ctype = (cached or {}).get("content_type")
            caption = (cached or {}).get("vision_caption")
            data = (cached or {}).get("data")
            if not caption:
                if custom_struct:  # a custom STRUCTURED extractor (prompt + schema), no classify
                    ctype = "custom"
                    data = client.structured_image(repr_leaf.path, custom_leaf, leaf_schema)
                    caption = (data.get("summary") or "").strip() or json.dumps(data, ensure_ascii=False)
                elif custom_leaf:  # a custom free-text prompt → caption, no structured fields
                    ctype, data = preset, None
                    caption = client.caption_image(repr_leaf.path, custom_leaf)
                else:
                    if auto:
                        cls = client.structured_image(repr_leaf.path, classify_prompt_used, _classify_schema())
                        ctype = cls.get("content_type") or "other"
                    else:
                        ctype = preset
                    cp = _content_prompts(ctype)
                    # Code/terminal frames carry small dense text → extract at higher resolution.
                    mpx = cmpx if ctype in CODE_TYPES else None
                    data = client.structured_image(repr_leaf.path, cp["prompt"], cp["schema"], max_px=mpx)
                    caption = (data.get("summary") or "").strip()
            tslice = _transcript_slice(segments, t_lo, t_hi)
            node = _node(nid, depth, lo, hi, repr_leaf, t_lo, t_hi, n_frames=1, content_type=ctype,
                         vision_caption=caption, transcript_slice=tslice, summary=caption, children=[], data=data)
            nodes[nid] = node
            progress("caption", t_lo, t_hi)
        else:
            left = visit(lo, mid, depth + 1)
            right = visit(mid + 1, hi, depth + 1)
            # A range's type is its children's if they agree, else "mixed" → general focus.
            lt, rt = left.get("content_type"), right.get("content_type")
            ctype = lt if lt == rt else "mixed"
            focus = fixed_focus or _content_prompts(ctype if ctype != "mixed" else "general")["combine_focus"]
            tslice = _transcript_slice(segments, t_lo, t_hi)
            summary = (cached or {}).get("summary") or client.combine(
                _combine_prompt(left["summary"], right["summary"], tslice[:TRANSCRIPT_FEED_CAP], focus)
            )
            node = _node(nid, depth, lo, hi, repr_leaf, t_lo, t_hi, n_frames=hi - lo + 1, content_type=ctype,
                         vision_caption=None, transcript_slice=tslice, summary=summary,
                         children=[left["id"], right["id"]], data=None)
            nodes[nid] = node
            progress("combine", t_lo, t_hi)
        # Checkpoint after each node so a crash/network drop resumes (skip done nodes).
        _save_checkpoint(d, params, model_label, nodes, root_id=f"0-{n - 1}",
                         leaf_count=n, node_count=total_nodes, backup_once=backup_done)
        return node

    root = visit(0, n - 1, 0)
    # Stamp parents (children carry ids; set the reverse link in one pass).
    for node in nodes.values():
        for cid in node["children"]:
            if cid in nodes:
                nodes[cid]["parent"] = node["id"]

    _flag_code_reliability(nodes)  # #51: mark cross-frame-disagreeing code + surface dictated tokens
    index = _assemble(params, model_label, nodes, root_id=root["id"], leaf_count=n, node_count=total_nodes)
    _write_index(d, index)
    _write_prompts_record(
        d, model_label, preset, nodes, n,
        classify_prompt=(classify_prompt_used if auto else None),
        custom={"classify_prompt": classify_prompt, "leaf_prompt": custom_leaf, "leaf_schema": leaf_schema},
    )
    _write_agents_md(d, index, model_label, nodes)
    log.info("indexed %s: %d leaves, %d nodes", d.name, n, total_nodes)
    return index


def _write_prompts_record(d: Path, model_label, preset, nodes: dict, leaf_count, *, classify_prompt, custom):
    """Persist the prompts/schemas this index used to ``<session>/index_prompts.json`` — the corpus
    the tuning skill ingests to improve the default classifier + extractors. Records the per-type
    content distribution, the (default or overridden) classify prompt, and any caller-supplied custom
    prompts/schemas (e.g. crafted by a frontier model via capture_index)."""
    from collections import Counter

    counts = Counter(nd["content_type"] for nd in nodes.values() if not nd["children"] and nd.get("content_type"))
    extract_used = {t: {"prompt": v["prompt"], "schema": v["schema"]}
                    for t, v in CONTENT_PROMPTS.items() if t in counts}
    record = {
        "index_version": INDEX_VERSION,
        "model": model_label,
        "preset": preset,
        "created_at": iso(now()),
        "leaf_count": leaf_count,
        "type_counts": dict(counts),
        "classify": {"prompt": classify_prompt, "enum": CONTENT_TYPES} if classify_prompt else None,
        # The default extractors that actually fired (so the skill diffs them against the customs).
        "extract_defaults": extract_used,
        # Caller overrides (custom prompts/schemas — typically frontier-model-crafted).
        "custom": {k: v for k, v in custom.items() if v},
    }
    try:
        (d / "index_prompts.json").write_text(json.dumps(record, indent=2, ensure_ascii=False), encoding="utf-8")
    except Exception:
        log.exception("failed to write index_prompts.json")


def _write_agents_md(d: Path, index: dict, model_label, nodes: dict) -> None:
    """Write ``<session>/AGENTS.md`` — a trust-calibration + usage guide for any agent that later
    consumes this capture (#57). The structured ``data`` (especially verbatim ``code``) is a CHEAP
    LOCAL-MODEL scaffold and is hallucination-prone (the eval study saw the 9B confabulate code), so
    this tells the consumer which fields to trust vs verify. Content-aware via the leaf type mix."""
    from collections import Counter

    leaves = [nd for nd in nodes.values() if not nd["children"]]
    counts = Counter(nd.get("content_type") for nd in leaves if nd.get("content_type"))
    mix = ", ".join(f"{c} {t}" for t, c in counts.most_common()) or "mixed"
    has_code = any(t in counts for t in ("coding", "terminal", "lecture", "custom"))
    has_meeting = "meeting" in counts
    uncertain = [nd for nd in leaves if (nd.get("data") or {}).get("ocr_uncertain")]  # #51
    has_nv = any((nd.get("data") or {}).get("narration_values") for nd in leaves)
    title = d.name
    recorded = ""
    try:
        sm = (json.loads((d / "session.json").read_text()).get("summary") or {})
        title = sm.get("window_title") or sm.get("app_name") or d.name
        recorded = sm.get("started_at") or ""
    except Exception:
        pass

    out = [f"# Capture: {title}", ""]
    if recorded:
        out.append(f"_Recorded {recorded}_\n")
    if index.get("root_summary"):
        out += [index["root_summary"], ""]
    out += [
        "## Artifacts",
        "- `index.json` — hierarchical index: per-frame leaf captions → range summaries → a root summary. Each",
        "  leaf node carries `repr_frame.path` (the source screenshot), `content_type`, `data` (the structured",
        "  extraction), and `transcript_slice` (the narration over that span).",
        "- `transcript.jsonl` — the time-aligned spoken audio. **Authoritative.**",
        "- `screenshots/` — the full-resolution source frames. Re-read these for anything you must trust verbatim.",
        "- `index_prompts.json` — the model + prompts/schemas this index was built with.",
        "",
        "## How to trust this index (read first)",
        f"The structured `data` was extracted by a small LOCAL vision model (`{model_label or 'local VLM'}`). Treat",
        "it as a cheap **scaffold for navigation, not ground truth**:",
        "- **Transcript = reliable.** Where the narration states a value (a name, number, command, note), prefer it",
        "  over the on-screen OCR.",
        "- **Cross-frame disagreement = a red flag.** When the same on-screen content is captured differently across",
        "  nearby leaves, that region is OCR-unreliable — verify it against the source frame.",
        "- **Summaries / topics = directionally reliable** for locating things; exact details need the frame.",
    ]
    if has_code:
        out += [
            "- **Verbatim `code` is OCR and hallucination-prone** — the model can misread an identifier (e.g. drop a",
            "  leading letter, `AActor`→`Actor`) or confabulate whole snippets. Before reproducing any code:",
            "  cross-check the transcript, and **re-read the frame at `repr_frame.path`** (full resolution) for the",
            "  exact tokens. Do not ship the index's `code` verbatim without verifying it against the frame.",
        ]
        if has_nv:
            out.append("- **`data.narration_values`** holds tokens (numbers/identifiers) SPOKEN over a code frame — "
                       "prefer these over the OCR'd `code` when they conflict (the narrator is more reliable than the OCR).")
        if uncertain:
            out.append(f"- **`data.ocr_uncertain: true`** marks the {len(uncertain)} code frame(s) whose file name "
                       "disagreed across frames (a confabulation signature) — re-read those FIRST (listed below).")
    if has_meeting:
        out += [
            "- **Meeting fields** — participant names, task assignments, and decisions are reliable when the",
            "  transcript corroborates them; small-font shared-board text (ticket IDs, dates) may be misread — verify",
            "  from the frame.",
        ]
    if uncertain:
        out += ["", "## Frames flagged for verification (#51)",
                "These code frames disagreed with their neighbours (likely OCR confabulation) — re-read them first:"]
        for nd in uncertain[:20]:
            ld = nd.get("data") or {}
            fp = (nd.get("repr_frame") or {}).get("path", "?")
            out.append(f"- `{fp}` — claimed `{ld.get('file') or ld.get('file_or_asset') or '?'}`")
    out += ["", "## This capture", f"- Content mix: {mix}",
            f"- {index.get('leaf_count', '?')} leaves / {index.get('node_count', '?')} nodes", ""]
    try:
        (d / "AGENTS.md").write_text("\n".join(out), encoding="utf-8")
    except Exception:
        log.exception("failed to write AGENTS.md")


def _node(nid, depth, lo, hi, repr_leaf, t_lo, t_hi, *, n_frames, content_type, vision_caption,
          transcript_slice, summary, children, data=None) -> dict:
    return {
        "id": nid,
        "depth": depth,
        "lo_idx": lo,
        "hi_idx": hi,
        "t_lo": round(t_lo, 3),
        "t_hi": None if t_hi == float("inf") else round(t_hi, 3),
        "repr_frame": {"path": str(repr_leaf.path), "iso": repr_leaf.iso},
        "represents_n_frames": n_frames,
        "content_type": content_type,  # classified type (auto) or the preset; "mixed" for ranges
        "data": data,  # leaves: the STRUCTURED extraction (participants, active_speaker, …)
        "vision_caption": vision_caption,
        "transcript_slice": transcript_slice,
        "summary": summary,
        "children": children,
        "parent": None,
    }


def _assemble(params, model_label, nodes: dict, *, root_id, leaf_count, node_count) -> dict:
    ordered = [nodes[k] for k in sorted(nodes, key=_id_sort_key)]
    return {
        "index_version": INDEX_VERSION,
        "model": model_label,
        "params": params,
        "created_at": iso(now()),
        "leaf_count": leaf_count,
        "node_count": node_count,
        "complete": len(nodes) == node_count,
        "root_id": root_id,
        "root_summary": nodes.get(root_id, {}).get("summary", ""),
        "nodes": ordered,
    }


def _id_sort_key(nid: str):
    try:
        lo, hi = nid.split("-")
        return (int(lo), int(hi))
    except Exception:
        return (1 << 30, nid)


# -- checkpoint / output ------------------------------------------------------

def _index_path(session_dir: Path) -> Path:
    return session_dir / "index.json"


def _load_checkpoint(session_dir: Path, params: dict, model_label: str | None) -> dict:
    """Prior nodes from a matching, incomplete ``index.json`` (same params + model), keyed
    by id — so a resumed build reuses captions/summaries instead of re-calling the model."""
    p = _index_path(session_dir)
    if not p.is_file():
        return {}
    try:
        prev = json.loads(p.read_text(encoding="utf-8"))
    except Exception:
        return {}
    if prev.get("params") != params or prev.get("model") != model_label:
        return {}  # different settings ⇒ a fresh build (the old index is overwritten)
    return {node["id"]: node for node in prev.get("nodes", []) if node.get("summary")}


def _save_checkpoint(session_dir: Path, params, model_label, nodes: dict, *, root_id,
                     leaf_count, node_count, backup_once: list) -> None:
    # Back up a prior COMPLETE index once, before the first checkpoint overwrites it.
    p = _index_path(session_dir)
    if not backup_once[0]:
        backup_once[0] = True
        try:
            if p.is_file() and json.loads(p.read_text()).get("complete"):
                p.replace(session_dir / "index.prev.json")
        except Exception:
            pass
    try:
        idx = _assemble(params, model_label, nodes, root_id=root_id,
                        leaf_count=leaf_count, node_count=node_count)
        p.write_text(json.dumps(idx, indent=2, ensure_ascii=False), encoding="utf-8")
    except Exception:
        log.exception("failed to checkpoint index.json")


def _write_index(session_dir: Path, index: dict) -> None:
    _index_path(session_dir).write_text(json.dumps(index, indent=2, ensure_ascii=False), encoding="utf-8")
    try:
        (session_dir / "index_summary.txt").write_text(
            (index.get("root_summary") or "").strip() + "\n", encoding="utf-8"
        )
    except Exception:
        pass


def load_index(session_dir: "str | Path") -> dict | None:
    """The built index for a session, or None if not indexed."""
    p = _index_path(Path(session_dir))
    if not p.is_file():
        return None
    try:
        return json.loads(p.read_text(encoding="utf-8"))
    except Exception:
        return None
