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

from .session import CaptureSession
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
        self._append_index(session)

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
            return dict(rec) if rec is not None else None

    def summaries(self) -> list[dict]:
        """All known sessions, oldest first (ids are timestamp-prefixed)."""
        with self._lock:
            merged = dict(self._history)
            for sid, s in self._live.items():
                merged[sid] = s.summary()
            return [merged[sid] for sid in sorted(merged)]

    def history_record(self, session_id: str) -> dict | None:
        with self._lock:
            rec = self._history.get(session_id)
            return dict(rec) if rec is not None else None

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

    def _append_index(self, session: CaptureSession) -> None:
        """Best-effort append; never breaks a capture (mirrors session.json writes)."""
        try:
            self.index_path.parent.mkdir(parents=True, exist_ok=True)
            line = json.dumps({"id": session.id, "dir": str(session.dir), "created_at": iso(now())})
            with self.index_path.open("a", encoding="utf-8") as f:
                f.write(line + "\n")
        except Exception:
            log.exception("failed to append session index %s", self.index_path)

    def _load_history(self) -> None:
        """Rebuild finished-session records from the index + each session.json."""
        try:
            text = self.index_path.read_text(encoding="utf-8")
        except FileNotFoundError:
            return
        except Exception:
            log.exception("failed to read session index %s", self.index_path)
            return

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
