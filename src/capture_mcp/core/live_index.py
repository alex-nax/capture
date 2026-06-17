"""Live/online incremental indexing (#55).

Builds the multimodal index AS a session captures, one leaf at a time, so a navigable index exists
in near-real-time instead of only after a post-capture batch build. The tree is a **binary merge-tree**:
appending a frame extracts a leaf, then merges it with equal-sized right-edge subtrees (a binary
counter), so each append is O(log n) NEW combines and NEVER recomputes existing summaries. The forest
of power-of-2 subtrees collapses into a single root at `finalize()`.

Runs only when a vision endpoint is reachable (a daemon worker drives `add_frame` off the capture hot
path). No endpoint → the session falls back to the existing post-capture `indexer.build_index`. The
output is shape-identical to a batch index (same `_node`/`_combine_prompt`/`_assemble`/`AGENTS.md`), so
everything downstream (the GUI tree, the eval/tuning skills) works unchanged.
"""

from __future__ import annotations

import logging
import threading
import time
from pathlib import Path

from . import frames as frames_mod
from . import indexer

log = logging.getLogger("capture.live_index")


class _Repr:
    """Minimal stand-in for a `Frame` (just `.path`/`.iso`) so `indexer._node` can build a
    `repr_frame` for an internal node from a child's stored `repr_frame` dict."""

    __slots__ = ("path", "iso")

    def __init__(self, repr_frame: dict):
        self.path = repr_frame.get("path", "")
        self.iso = repr_frame.get("iso", "")


def _extract_leaf(client, frame_path, *, preset: str, code_max_px: int):
    """Classify (auto) → type-specific structured extraction for one frame → (ctype, caption, data).
    Mirrors the auto-path leaf step in `indexer.build_index`, including the #49 code-resolution bump."""
    if preset and preset not in ("auto", "general"):
        ctype = preset
    else:
        cls = client.structured_image(frame_path, indexer.CLASSIFY_PROMPT, indexer._classify_schema())
        ctype = cls.get("content_type") or "other"
    cp = indexer._content_prompts(ctype)
    mpx = code_max_px if ctype in indexer.CODE_TYPES else None
    data = client.structured_image(frame_path, cp["prompt"], cp["schema"], max_px=mpx)
    caption = (data.get("summary") or "").strip() or "(no caption)"
    return ctype, caption, data


class LiveIndex:
    """Incremental binary merge-tree index, appended leaf-by-leaf. Thread-safe (`add_frame`,
    `finalize`, `checkpoint` take the lock); cheap `checkpoint()` writes a partial tree during
    capture, `finalize()` does the one real root-combine at stop."""

    def __init__(self, session_dir, client, *, preset: str = "auto", fuse_transcript: bool = True,
                 model_label: str | None = None, on_progress=None):
        self.d = Path(session_dir)
        self.client = client
        self.preset = preset or "auto"
        self.fuse_transcript = fuse_transcript
        self.model_label = model_label
        self.on_progress = on_progress
        self.code_max_px = indexer._code_max_px()
        self.nodes: dict = {}
        self.forest: list[dict] = []  # completed subtrees, strictly DECREASING span left→right
        self.n = 0
        self.params = {"sample_rate": None, "max_leaves": None, "fuse_transcript": fuse_transcript,
                       "prompt_preset": self.preset, "live": True}
        self._lock = threading.RLock()

    # -- building --------------------------------------------------------------

    def _segments(self):
        return indexer._load_transcript(self.d) if self.fuse_transcript else []

    def add_frame(self, frame, frame_end: float) -> None:
        """Extract one frame into a leaf and merge it into the tree (one combine per power-of-2 carry)."""
        with self._lock:
            ctype, caption, data = _extract_leaf(self.client, frame.path, preset=self.preset,
                                                 code_max_px=self.code_max_px)
            i = self.n
            tslice = indexer._transcript_slice(self._segments(), frame.offset, frame_end)
            leaf = indexer._node(f"{i}-{i}", 0, i, i, frame, frame.offset, frame_end, n_frames=1,
                                 content_type=ctype, vision_caption=caption, transcript_slice=tslice,
                                 summary=caption, children=[], data=data)
            self.nodes[leaf["id"]] = leaf
            self.n += 1
            carry = leaf
            while self.forest and self._span(self.forest[-1]) == self._span(carry):
                carry = self._combine(self.forest.pop(), carry)
            self.forest.append(carry)
            if self.on_progress:
                self.on_progress(self.n)

    def _span(self, node) -> int:
        return node["hi_idx"] - node["lo_idx"] + 1

    def _combine(self, left, right):
        lo, hi = left["lo_idx"], right["hi_idx"]
        t_lo, t_hi = left["t_lo"], right["t_hi"] if right["t_hi"] is not None else right["t_lo"]
        tslice = indexer._transcript_slice(self._segments(), t_lo, t_hi)
        lt, rt = left.get("content_type"), right.get("content_type")
        ctype = lt if lt == rt else "mixed"
        focus = indexer._content_prompts(ctype if ctype != "mixed" else "general")["combine_focus"]
        summary = self.client.combine(indexer._combine_prompt(
            left["summary"], right["summary"], tslice[:indexer.TRANSCRIPT_FEED_CAP], focus))
        node = indexer._node(f"{lo}-{hi}", 0, lo, hi, _Repr(left["repr_frame"]), t_lo, t_hi,
                             n_frames=hi - lo + 1, content_type=ctype, vision_caption=None,
                             transcript_slice=tslice, summary=summary,
                             children=[left["id"], right["id"]], data=None)
        self.nodes[node["id"]] = node
        return node

    # -- snapshots -------------------------------------------------------------

    def _materialize_root(self, *, real: bool):
        """Return (root_id, nodes_copy). `real=True` LLM-combines the forest into one root (finalize);
        `real=False` makes a CHEAP synthetic root (text join, no model call) for live checkpoints —
        neither pollutes the live `forest`/`nodes` used by ongoing merges."""
        if not self.forest:
            return None, {}
        nodes = dict(self.nodes)
        if len(self.forest) == 1:
            return self.forest[0]["id"], nodes
        if real:
            root = self.forest[0]
            for nxt in self.forest[1:]:
                root = self._combine(root, nxt)  # adds real nodes to self.nodes (kept — they're the spine)
            return root["id"], dict(self.nodes)
        # cheap synthetic root over the current forest (no model call)
        lo = self.forest[0]["lo_idx"]
        hi = self.forest[-1]["hi_idx"]
        summary = "Live index (in progress): " + " · ".join(
            (f["summary"] or "")[:120] for f in self.forest)
        rid = f"{lo}-{hi}~live"
        nodes[rid] = indexer._node(rid, 0, lo, hi, _Repr(self.forest[0]["repr_frame"]),
                                   self.forest[0]["t_lo"], self.forest[-1]["t_hi"] or self.forest[-1]["t_lo"],
                                   n_frames=hi - lo + 1, content_type="mixed", vision_caption=None,
                                   transcript_slice="", summary=summary,
                                   children=[f["id"] for f in self.forest], data=None)
        return rid, nodes

    def _write(self, root_id: str, nodes: dict) -> dict:
        for nd in nodes.values():  # stamp parents
            nd["parent"] = None
        for nd in nodes.values():
            for cid in nd["children"]:
                if cid in nodes:
                    nodes[cid]["parent"] = nd["id"]
        indexer._flag_code_reliability(nodes)
        index = indexer._assemble(self.params, self.model_label, nodes, root_id=root_id,
                                  leaf_count=self.n, node_count=len(nodes))
        indexer._write_index(self.d, index)
        indexer._write_agents_md(self.d, index, self.model_label, nodes)
        return index

    def checkpoint(self) -> dict | None:
        """Write a partial index.json + AGENTS.md mid-capture (cheap synthetic root)."""
        with self._lock:
            root_id, nodes = self._materialize_root(real=False)
            return self._write(root_id, nodes) if root_id else None

    def finalize(self) -> dict | None:
        """One real root-combine + a final index.json/AGENTS.md (the navigable, complete tree)."""
        with self._lock:
            if not self.nodes:
                return None
            root_id, nodes = self._materialize_root(real=True)
            idx = self._write(root_id, nodes)
            indexer._write_prompts_record(
                self.d, self.model_label, self.preset, nodes, self.n,
                classify_prompt=(indexer.CLASSIFY_PROMPT if self.preset in ("auto", "general") else None),
                custom={})
            log.info("live-indexed %s: %d leaves, %d nodes", self.d.name, self.n, len(nodes))
            return idx


def run_worker(session_dir, client, *, preset: str = "auto", sample_rate: float = 0.5,
               fuse_transcript: bool = True, model_label: str | None = None,
               stop_event: threading.Event, on_progress=None, poll_seconds: float = 4.0,
               checkpoint_every: int = 8) -> "LiveIndex | None":
    """Drive a LiveIndex from a session's growing screenshots dir until `stop_event` is set, then
    finalize. Samples every ``round(1/sample_rate)``-th NEW frame (aligning with `select_leaves`).
    Returns the LiveIndex (finalized) or None if there were no frames. Never raises out — logs and
    finalizes what it has so a flaky endpoint can't break capture."""
    live = LiveIndex(session_dir, client, preset=preset, fuse_transcript=fuse_transcript,
                     model_label=model_label, on_progress=on_progress)
    step = max(1, round(1.0 / min(1.0, max(1e-3, sample_rate))))
    consumed = 0          # frames examined (sampled by `step`)
    since_ckpt = 0
    try:
        while True:
            stopping = stop_event.is_set()
            all_frames = frames_mod.list_frames(session_dir)
            # Sample the not-yet-consumed tail; keep one behind the live edge so `frame_end` is known.
            limit = len(all_frames) if stopping else max(0, len(all_frames) - 1)
            while consumed < limit:
                frame = all_frames[consumed]
                if consumed % step == 0:
                    frame_end = all_frames[consumed + 1].offset if consumed + 1 < len(all_frames) else float("inf")
                    try:
                        live.add_frame(frame, frame_end)
                        since_ckpt += 1
                    except Exception:
                        log.exception("live add_frame failed (continuing)")
                consumed += 1
                if since_ckpt >= checkpoint_every:
                    since_ckpt = 0
                    try:
                        live.checkpoint()
                    except Exception:
                        log.exception("live checkpoint failed")
            if stopping:
                break
            time.sleep(poll_seconds)
    except Exception:
        log.exception("live index worker error")
    return live.finalize() and live if live.n else None
