"""SessionRegistry: bounded live-session tracking + disk-backed history.

Extracted from server.py so every frontend (MCP today; daemon/CLI/GUI per
docs/specs/product-architecture.md) shares one registry implementation.

Live sessions are `CaptureSession` objects owned by this process. History is
rebuilt at construction from an append-only index file (one JSON line per
session ever started: ``{"id", "dir", "created_at"}``) by re-reading each
session dir's ``session.json`` — so a server restart no longer loses session
history. The index lives at ``~/.capture/sessions.jsonl`` unless overridden
via ``CAPTURE_SESSION_INDEX`` (tests point it at a temp file to stay hermetic).
"""

from __future__ import annotations

import json
import logging
import os
import threading
from pathlib import Path

from .session import CaptureSession, prune_session_dir, session_capabilities


def _rewrite_session_json_summary(session_dir: str, updates: dict) -> None:
    """Best-effort: merge ``updates`` into the on-disk session.json summary so a daemon
    restart sees post-prune counts/flags (capabilities are also recomputed live on read)."""
    p = Path(session_dir) / "session.json"
    try:
        meta = json.loads(p.read_text())
        meta.setdefault("summary", {}).update(updates)
        p.write_text(json.dumps(meta, indent=2, ensure_ascii=False))
    except Exception:
        pass


def _with_caps(rec: dict) -> dict:
    """A history-record summary refreshed with current on-disk capability flags (so a
    pruned session reports the truth, not the flags frozen into session.json)."""
    d = dict(rec)
    if d.get("dir"):
        d.update(session_capabilities(d["dir"]))
    return d
from .util import iso, now

log = logging.getLogger(__name__)

MAX_SESSIONS = 100

#: Live states: a session in one of these is never evicted and is "this
#: process's business"; anything else found on disk is finished or stale.
_LIVE_STATES = ("starting", "running", "stopping")


def default_index_path() -> Path:
    env = os.environ.get("CAPTURE_SESSION_INDEX")
    if env:
        return Path(env).expanduser()
    return Path.home() / ".capture" / "sessions.jsonl"


class SessionRegistry:
    """Thread-safe registry of live sessions plus recovered on-disk history."""

    def __init__(self, index_path: str | Path | None = None, max_sessions: int = MAX_SESSIONS) -> None:
        self.index_path = Path(index_path) if index_path else default_index_path()
        self.max_sessions = max_sessions
        self._lock = threading.Lock()
        self._live: dict[str, CaptureSession] = {}
        # id -> final summary dict recovered from disk (read-only records).
        self._history: dict[str, dict] = {}
        self._load_history()

    # -- live sessions ---------------------------------------------------------

    def add(self, session: CaptureSession) -> None:
        with self._lock:
            self._live[session.id] = session
            self._history.pop(session.id, None)
            self._prune_locked()
        self._append_index(session.id, str(session.dir))

    def add_recovered(self, session_id: str, session_dir: str | Path) -> dict | None:
        """Register a session whose dir already exists on disk (e.g. an import), so it
        appears in ``summaries()`` immediately without a daemon restart. Appends the index
        and builds a read-only history record from its ``session.json``. Returns that record
        (``None`` if no session.json is there to recover)."""
        d = str(session_dir)
        if not (Path(d) / "session.json").exists():
            return None
        self._append_index(session_id, d)
        with self._lock:
            rec = self._recover(session_id, d)
            self._history[session_id] = rec
            self._prune_locked()
        return rec

    def get(self, session_id: str) -> CaptureSession | None:
        with self._lock:
            return self._live.get(session_id)

    def running(self) -> list[CaptureSession]:
        with self._lock:
            return [s for s in self._live.values() if s.state == "running"]

    # -- summaries (live + history) --------------------------------------------

    def summary(self, session_id: str) -> dict | None:
        """One session's summary: live if owned by this process, else history."""
        with self._lock:
            s = self._live.get(session_id)
            if s is not None:
                return s.summary()
            rec = self._history.get(session_id)
            return _with_caps(rec) if rec is not None else None

    def summaries(self) -> list[dict]:
        """All known sessions, oldest first (ids are timestamp-prefixed)."""
        with self._lock:
            merged = {sid: _with_caps(rec) for sid, rec in self._history.items()}
            for sid, s in self._live.items():
                merged[sid] = s.summary()
            return [merged[sid] for sid in sorted(merged)]

    def history_record(self, session_id: str) -> dict | None:
        with self._lock:
            rec = self._history.get(session_id)
            return dict(rec) if rec is not None else None

    def delete(self, session_id: str) -> bool:
        """Forget a *finished* session: drop its history/live record and rewrite the
        index without it. Returns True if it was known. Raises ValueError for a live
        (starting/running/stopping) session — stop it first. Does NOT touch the
        session's on-disk dir; the caller removes that."""
        with self._lock:
            live = self._live.get(session_id)
            if live is not None and live.state in _LIVE_STATES:
                raise ValueError("session is live; stop it before deleting")
            existed = session_id in self._history or session_id in self._live
            self._history.pop(session_id, None)
            self._live.pop(session_id, None)
            self._rewrite_index_locked(session_id)
            return existed

    def prune_session(self, session_id: str, parts: list[str]) -> dict:
        """Prune on-disk artifacts of a *finished* session (``session.prune_session_dir``).
        Returns ``{pruned, freed_bytes, screenshots, <capability flags>}``. Raises ValueError
        for a live session or an unknown id."""
        with self._lock:
            live = self._live.get(session_id)
            if live is not None and live.state in _LIVE_STATES:
                raise ValueError("session is live; stop it before pruning")
            rec = self._history.get(session_id) or (live.summary() if live else None)
            if rec is None:
                raise ValueError(f"unknown session_id {session_id!r}")
            d = rec.get("dir")
        if not d or not (Path(d) / "session.json").exists():
            raise ValueError("session dir is missing or not a capture dir")
        freed, count = prune_session_dir(d, parts)
        caps = session_capabilities(d)
        with self._lock:
            if session_id in self._history:
                self._history[session_id]["screenshots"] = count
                self._history[session_id].update(caps)
        _rewrite_session_json_summary(d, {"screenshots": count, **caps})
        return {"pruned": list(parts), "freed_bytes": freed, "screenshots": count, **caps}

    def update_summary(self, session_id: str, updates: dict) -> None:
        """Merge ``updates`` into a finished session's stored summary (in-memory history
        record + its session.json), e.g. a new transcript_segments count after re-transcribe."""
        with self._lock:
            rec = self._history.get(session_id)
            d = rec.get("dir") if rec else None
            if rec is not None:
                rec.update(updates)
        if d:
            _rewrite_session_json_summary(d, updates)

    # -- internals --------------------------------------------------------------

    def _prune_locked(self) -> None:
        """Bound retained history. Caller holds ``self._lock``.

        Live (starting/running/stopping) sessions are never evicted, so this
        bounds retained *finished* sessions rather than the absolute size —
        same tradeoff as the pre-extraction server registry.
        """
        total = len(self._live) + len(self._history)
        if total <= self.max_sessions:
            return
        # ids are timestamp-prefixed, so lexical order == chronological order.
        finished = sorted(
            [sid for sid, s in self._live.items() if s.state not in _LIVE_STATES]
            + list(self._history)
        )
        for sid in finished[: total - self.max_sessions]:
            self._live.pop(sid, None)
            self._history.pop(sid, None)

    def _append_index(self, session_id: str, session_dir: str) -> None:
        """Best-effort append; never breaks a capture (mirrors session.json writes)."""
        try:
            self.index_path.parent.mkdir(parents=True, exist_ok=True)
            line = json.dumps({"id": session_id, "dir": session_dir, "created_at": iso(now())})
            with self.index_path.open("a", encoding="utf-8") as f:
                f.write(line + "\n")
        except Exception:
            log.exception("failed to append session index %s", self.index_path)

    def _rewrite_index_locked(self, drop_id: str) -> None:
        """Rewrite the append-only index without ``drop_id`` (best-effort; atomic).

        Unparseable lines are kept verbatim (same tolerance as ``_load_history``).
        Caller holds ``self._lock``.
        """
        try:
            text = self.index_path.read_text(encoding="utf-8")
        except FileNotFoundError:
            return
        except Exception:
            log.exception("failed to read session index for rewrite %s", self.index_path)
            return
        kept: list[str] = []
        for ln in text.splitlines():
            s = ln.strip()
            if not s:
                continue
            try:
                if json.loads(s).get("id") == drop_id:
                    continue
            except Exception:
                kept.append(ln)  # keep torn/corrupt lines as-is
                continue
            kept.append(ln)
        try:
            tmp = self.index_path.with_suffix(".jsonl.tmp")
            tmp.write_text("".join(k + "\n" for k in kept), encoding="utf-8")
            tmp.replace(self.index_path)
        except Exception:
            log.exception("failed to rewrite session index %s", self.index_path)

    def _load_history(self) -> None:
        """Rebuild finished-session records from the index, then also scan the runs
        dir so on-disk captures whose index entry was lost still appear."""
        try:
            text = self.index_path.read_text(encoding="utf-8")
        except FileNotFoundError:
            text = ""
        except Exception:
            log.exception("failed to read session index %s", self.index_path)
            text = ""

        entries: dict[str, dict] = {}
        for ln in text.splitlines():
            ln = ln.strip()
            if not ln:
                continue
            try:
                e = json.loads(ln)
                entries[e["id"]] = e  # later lines win (same id re-indexed)
            except Exception:
                continue  # tolerate torn/corrupt lines in an append-only log

        # Newest max_sessions only; ids are timestamp-prefixed (chronological).
        for sid in sorted(entries)[-self.max_sessions :]:
            self._history[sid] = self._recover(sid, entries[sid].get("dir", ""))

        self._scan_runs_dir()

    def _scan_runs_dir(self) -> None:
        """Recover any `capture-*` session folders on disk that the index doesn't
        cover (the index can be lost/reset while the folders remain). Idempotent;
        index entries win. Scans ``$CAPTURE_RUNS_DIR`` else ``~/.capture/runs``."""
        runs = os.environ.get("CAPTURE_RUNS_DIR")
        runs_dir = Path(runs).expanduser() if runs else Path.home() / ".capture" / "runs"
        try:
            candidates = [d for d in runs_dir.glob("capture-*") if d.is_dir()]
        except OSError:
            return
        prefix = "capture-"
        for d in candidates:
            sid = d.name[len(prefix) :] if d.name.startswith(prefix) else d.name
            if not sid or sid in self._history or sid in self._live:
                continue
            if not (d / "session.json").exists():
                continue
            self._history[sid] = self._recover(sid, str(d))
        # Bound retained history (live sessions aren't in _history yet at load time).
        if len(self._history) > self.max_sessions:
            for sid in sorted(self._history)[: len(self._history) - self.max_sessions]:
                self._history.pop(sid, None)

    @staticmethod
    def _template(session_id: str, session_dir: str) -> dict:
        """A full-shaped summary (CaptureSession.summary() keys, defaults filled).

        Every record the registry returns — live or recovered — is merged onto
        this so clients get ONE uniform shape regardless of what an old/partial
        session.json recorded (the /v1 contract pins it; see daemon/models.py).
        """
        return {
            "session_id": session_id, "state": "unknown", "dir": session_dir,
            "pid": None, "window_title": None, "started_at": None, "stopped_at": None,
            "screenshots": 0, "screenshot_errors": 0, "log_lines": 0,
            "process_running": None, "audio_mode": "unknown", "audio_status": "unknown",
            "transcript_segments": 0, "asr_errors": 0, "notes": [],
        }

    @staticmethod
    def _recover(session_id: str, session_dir: str) -> dict:
        """Build a read-only, full-shaped record for one indexed session."""
        tmpl = SessionRegistry._template(session_id, session_dir)
        try:
            recorded = dict(json.loads((Path(session_dir) / "session.json").read_text(encoding="utf-8"))["summary"])
        except Exception:
            # Dir deleted, or the process died before/while writing session.json.
            tmpl["notes"] = ["recovered from index; session.json missing or unreadable"]
            return tmpl
        rec = {**tmpl, **recorded, "session_id": session_id, "dir": session_dir}
        if rec.get("state") in _LIVE_STATES:
            # Recorded as live by a process that is gone: it was interrupted.
            rec["state"] = "interrupted"
            rec["notes"] = list(rec.get("notes", [])) + [
                "recovered from disk; the capturing process exited while this session was live"
            ]
        return rec
