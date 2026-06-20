#!/usr/bin/env python3
"""Tiny stdlib client for the capture daemon's ``/v1`` API.

The capture daemon is now the native Rust ``captured`` (v3 cutover, #67) — the Python
``capture_mcp`` package is gone. The dev/eval utilities no longer run an in-process engine;
they **proxy the running daemon** over ``/v1``. This is that proxy: it reads
``~/.capture/daemon.json`` (or ``$CAPTURE_DAEMON_JSON``) for the endpoint + bearer token and
does GET/POST + the SSE event stream. Pure stdlib — runs under any ``python3`` (no venv,
no deps).
"""
from __future__ import annotations

import json
import os
import urllib.request
from pathlib import Path
from typing import Iterator, Optional


def daemon_json_path() -> Path:
    env = os.environ.get("CAPTURE_DAEMON_JSON")
    return Path(env) if env else Path.home() / ".capture" / "daemon.json"


class Daemon:
    """A thin ``/v1`` client (mirrors the routes the GUI/MCP use)."""

    def __init__(self, endpoint: str, token: str):
        self.endpoint = endpoint.rstrip("/")
        self.token = token

    @classmethod
    def discover(cls) -> "Optional[Daemon]":
        """Read the daemon's 0600 discovery file; ``None`` if no daemon has written one."""
        try:
            d = json.loads(daemon_json_path().read_text())
            return cls(d["endpoint"], d["token"])
        except Exception:
            return None

    # -- core verbs --
    def _req(self, method: str, path: str, body=None, timeout: float = 30):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.endpoint + path, data=data, method=method)
        req.add_header("Authorization", "Bearer " + self.token)
        if data is not None:
            req.add_header("Content-Type", "application/json")
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read().decode()
        return json.loads(raw) if raw.strip() else {}

    def get(self, path: str, timeout: float = 30):
        return self._req("GET", path, None, timeout)

    def post(self, path: str, body=None, timeout: float = 30):
        return self._req("POST", path, body if body is not None else {}, timeout)

    # -- convenience --
    def available(self) -> bool:
        try:
            return bool(self.get("/v1/health", timeout=3).get("ok"))
        except Exception:
            return False

    def sessions(self) -> list:
        return self.get("/v1/sessions").get("sessions", [])

    def find_session(self, suffix: str) -> Optional[dict]:
        """Resolve a FULL session_id from a short suffix (sessions are keyed by the full stamp)."""
        return next((s for s in self.sessions() if suffix in s["session_id"]), None)

    def index(self, session_id: str, **body):
        """POST /v1/sessions/{id}/index. Pass endpoint/model/sample_rate/max_leaves/
        prompt_preset/leaf_prompt/leaf_schema/classify_prompt/provider/host/port/… ; ``None``
        values are dropped (the daemon applies its defaults)."""
        clean = {k: v for k, v in body.items() if v is not None}
        return self.post(f"/v1/sessions/{session_id}/index", clean)

    def asr_set_model(self, repo: str):
        return self.post("/v1/asr/model", {"repo": repo})

    def asr_download(self, repo: str):
        return self.post("/v1/asr/models/download", {"repo": repo})

    def asr_models(self):
        return self.get("/v1/asr/models")

    def events(self, timeout: float = 60) -> "Iterator[dict]":
        """Yield parsed JSON objects from the ``/v1/events`` SSE stream. The daemon sends a
        keep-alive comment every ~15 s, so ``timeout`` (the socket read budget) need only
        exceed that. The caller breaks on its terminal event (e.g. ``index_done``)."""
        req = urllib.request.Request(self.endpoint + "/v1/events")
        req.add_header("Authorization", "Bearer " + self.token)
        resp = urllib.request.urlopen(req, timeout=timeout)
        for raw in resp:
            line = raw.decode("utf-8", "replace").strip()
            if line.startswith("data:"):
                payload = line[5:].strip()
                if payload:
                    try:
                        yield json.loads(payload)
                    except Exception:
                        continue
