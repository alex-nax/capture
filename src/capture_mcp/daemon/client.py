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

    def delete(self, sid: str) -> dict:
        return self._request("POST", f"/v1/sessions/{sid}/delete", body={})

    def prune(self, sid: str, parts: list[str]) -> dict:
        return self._request("POST", f"/v1/sessions/{sid}/prune", body={"parts": parts})

    def retranscribe(self, sid: str, asr_backend: str | None = None, model: str | None = None,
                     language: str | None = None, chunk_seconds: float | None = None) -> dict:
        body: dict = {}
        if asr_backend:
            body["asr_backend"] = asr_backend
        if model:
            body["model"] = model
        if language is not None:
            body["language"] = language
        if chunk_seconds is not None:
            body["chunk_seconds"] = chunk_seconds
        return self._request("POST", f"/v1/sessions/{sid}/retranscribe", body=body)

    def set_mic(self, sid: str, device: str | None) -> dict:
        return self._request("POST", f"/v1/sessions/{sid}/mic", body={"device": device})

    def asr_set_language(self, language: str | None) -> dict:
        return self._request("POST", "/v1/asr/language", body={"language": language or ""})

    def asr_set_chunk(self, seconds: float) -> dict:
        return self._request("POST", "/v1/asr/chunk", body={"seconds": seconds})

    def import_media(self, path: str, output_dir: str | None = None,
                     asr_backend: str | None = None, screenshot_interval: float | None = None) -> dict:
        body: dict = {"path": path}
        if output_dir:
            body["output_dir"] = output_dir
        if asr_backend:
            body["asr_backend"] = asr_backend
        if screenshot_interval is not None:
            body["screenshot_interval"] = screenshot_interval
        return self._request("POST", "/v1/sessions/import", body=body)

    def index(self, sid: str, endpoint: str | None = None, model: str | None = None,
              sample_rate: float | None = None, max_leaves: int | None = None,
              fuse_transcript: bool | None = None, prompt_preset: str | None = None,
              leaf_prompt: str | None = None, leaf_schema: dict | None = None,
              classify_prompt: str | None = None, max_px: int | None = None,
              provider: str | None = None, host: str | None = None, port: int | None = None) -> dict:
        body: dict = {}
        if provider:
            body["provider"] = provider
        if host:
            body["host"] = host
        if port is not None:
            body["port"] = port
        if endpoint:
            body["endpoint"] = endpoint
        if model:
            body["model"] = model
        if sample_rate is not None:
            body["sample_rate"] = sample_rate
        if max_leaves is not None:
            body["max_leaves"] = max_leaves
        if fuse_transcript is not None:
            body["fuse_transcript"] = fuse_transcript
        if prompt_preset:
            body["prompt_preset"] = prompt_preset
        if leaf_prompt:
            body["leaf_prompt"] = leaf_prompt
        if leaf_schema:
            body["leaf_schema"] = leaf_schema
        if classify_prompt:
            body["classify_prompt"] = classify_prompt
        if max_px is not None:
            body["max_px"] = max_px
        return self._request("POST", f"/v1/sessions/{sid}/index", body=body)

    def get_index(self, sid: str) -> dict:
        return self._request("GET", f"/v1/sessions/{sid}/index")

    def index_providers(self) -> dict:
        return self._request("GET", "/v1/index/providers")

    def index_models(self, provider: str | None = None, host: str | None = None,
                     port: int | None = None, key: str | None = None, url: str | None = None) -> dict:
        params = {"provider": provider, "host": host, "port": port, "key": key, "url": url}
        return self._request("GET", "/v1/index/models", params=params)

    def index_status(self, url: str | None = None, model: str | None = None) -> dict:
        params: dict = {}
        if url:
            params["url"] = url
        if model:
            params["model"] = model
        return self._request("GET", "/v1/index/status", params=params)

    def transcript(self, sid: str, tail: int | None = None) -> dict:
        return self._request("GET", f"/v1/sessions/{sid}/transcript", params={"tail": tail})

    def audio_mics(self) -> dict:
        return self._request("GET", "/v1/audio/mics")

    def asr_models(self) -> dict:
        return self._request("GET", "/v1/asr/models")

    def asr_download(self, repo: str) -> dict:
        return self._request("POST", "/v1/asr/models/download", body={"repo": repo})

    def asr_set_model(self, repo: str) -> dict:
        return self._request("POST", "/v1/asr/model", body={"repo": repo})

    def permissions(self) -> dict:
        return self._request("GET", "/v1/permissions")

    def request_permission(self, kind: str = "screen_recording") -> dict:
        return self._request("POST", "/v1/permissions/request", body={"kind": kind})

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
