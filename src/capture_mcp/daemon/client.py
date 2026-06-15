"""Thin stdlib client for the `captured` daemon `/v1` API.

Used by the `capture` CLI now, and intended for the MCP server's daemon-first
mode later. Reads `~/.capture/daemon.json` for the endpoint + bearer token; all
calls are plain urllib (no deps). `available()` is a cheap liveness check so a
client can transparently fall back to the embedded engine when no daemon runs.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlencode

from .server import daemon_json_path


class DaemonError(Exception):
    def __init__(self, status: int, message: str) -> None:
        super().__init__(f"daemon {status}: {message}")
        self.status = status
        self.message = message


class DaemonClient:
    def __init__(self, info: dict) -> None:
        self.endpoint = info["endpoint"].rstrip("/")
        self.token = info.get("token", "")
        self.api_version = info.get("api_version")

    # -- construction ----------------------------------------------------------

    @classmethod
    def from_discovery(cls, path: Path | None = None) -> "DaemonClient | None":
        path = path or daemon_json_path()
        try:
            return cls(json.loads(path.read_text()))
        except Exception:
            return None

    # -- transport -------------------------------------------------------------

    def _request(self, method: str, route: str, body: dict | None = None,
                 params: dict | None = None, timeout: float = 30.0) -> dict:
        url = self.endpoint + route
        if params:
            clean = {k: v for k, v in params.items() if v is not None}
            if clean:
                url += "?" + urlencode(clean)
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("Authorization", f"Bearer {self.token}")
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            try:
                msg = json.load(e).get("error", e.reason)
            except Exception:
                msg = e.reason
            raise DaemonError(e.code, msg) from e

    # -- API -------------------------------------------------------------------

    def available(self) -> bool:
        try:
            return self._request("GET", "/v1/health", timeout=2.0).get("ok") is True
        except Exception:
            return False

    def health(self) -> dict:
        return self._request("GET", "/v1/health", timeout=2.0)

    def windows(self, app_name: str | None = None, pid: int | None = None) -> dict:
        return self._request("GET", "/v1/windows", params={"app_name": app_name, "pid": pid})

    def start(self, **kwargs) -> dict:
        # Long timeout: start blocks on ASR model load on first use.
        return self._request("POST", "/v1/sessions", body=kwargs, timeout=600.0)

    def sessions(self) -> dict:
        return self._request("GET", "/v1/sessions")

    def session(self, sid: str) -> dict:
        return self._request("GET", f"/v1/sessions/{sid}")

    def stop(self, sid: str) -> dict:
        return self._request("POST", f"/v1/sessions/{sid}/stop", body={}, timeout=120.0)

    def transcript(self, sid: str, tail: int | None = None) -> dict:
        return self._request("GET", f"/v1/sessions/{sid}/transcript", params={"tail": tail})

    def asr_models(self) -> dict:
        return self._request("GET", "/v1/asr/models")

    def asr_download(self, repo: str) -> dict:
        return self._request("POST", "/v1/asr/models/download", body={"repo": repo})

    def asr_set_model(self, repo: str) -> dict:
        return self._request("POST", "/v1/asr/model", body={"repo": repo})

    def shutdown(self) -> dict:
        return self._request("POST", "/v1/admin/shutdown", body={}, timeout=5.0)

    def events(self, timeout: float | None = None):
        """Yield daemon events from the `/v1/events` SSE stream (blocking generator).

        Live-only: events from the moment of connection onward. Heartbeat
        comments (`: ping`) are skipped. Raises on connection error.
        """
        req = urllib.request.Request(self.endpoint + "/v1/events")
        req.add_header("Authorization", f"Bearer {self.token}")
        resp = urllib.request.urlopen(req, timeout=timeout)
        for raw in resp:
            line = raw.decode(errors="replace").rstrip("\n")
            if line.startswith("data: "):
                yield json.loads(line[6:])
