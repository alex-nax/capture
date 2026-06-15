"""The `captured` daemon: a stdlib HTTP `/v1` API over the capture engine.

Transport (this slice): HTTP/1.1 on 127.0.0.1:<ephemeral>, bearer-token auth on
every route except `/v1/health`. Discovery: `~/.capture/daemon.json` (mode 0600)
holds `{endpoint, token, pid, api_version, version}`. One daemon per
`daemon.json` (single-instance guard on start).

Routes:
  GET  /v1/health                        -> liveness + versions + counts (no auth)
  GET  /v1/windows[?app_name=&pid=]      -> window picker (core.list_windows)
  POST /v1/sessions                      -> start a capture (capture_start args)
  GET  /v1/sessions                      -> all sessions (live + recovered)
  GET  /v1/sessions/{id}                 -> one session summary
  POST /v1/sessions/{id}/stop            -> stop a capture
  GET  /v1/sessions/{id}/transcript?tail=N -> last N transcript segments
  POST /v1/admin/shutdown                -> stop the daemon

The engine runs blocking work in the handler thread (ThreadingHTTPServer);
`SessionRegistry` is thread-safe, so concurrent clients are fine. stdout is not
special here (unlike the MCP server) — logs go to stderr.
"""

from __future__ import annotations

import json
import logging
import os
import secrets
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from .. import __version__
from ..core import list_windows
from ..core.registry import SessionRegistry
from ..core.session import CaptureSession

log = logging.getLogger("capture_mcp.daemon")

API_VERSION = "1.0"

# capture_start args forwarded verbatim to CaptureSession (mirrors server.py).
_SESSION_ARGS = (
    "command", "pid", "app_name", "bundle_id", "screenshot_interval",
    "screenshot_format", "screenshot_resolution", "screenshot_jpeg_quality",
    "capture_screenshots", "capture_audio", "audio_source", "audio_chunk_seconds",
    "asr_backend", "cwd",
)


def daemon_json_path() -> Path:
    env = os.environ.get("CAPTURE_DAEMON_JSON")
    return Path(env).expanduser() if env else Path.home() / ".capture" / "daemon.json"


def _present(v: object) -> bool:
    """Exactly-one-target predicate, identical to the MCP server's."""
    if v is None:
        return False
    if isinstance(v, str):
        return bool(v.strip())
    return True


class _ApiError(Exception):
    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status
        self.message = message


class CaptureDaemon(ThreadingHTTPServer):
    """ThreadingHTTPServer carrying the shared registry + auth token."""

    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, host: str = "127.0.0.1", port: int = 0, token: str | None = None) -> None:
        super().__init__((host, port), _Handler)
        self.registry = SessionRegistry()
        self.token = token or secrets.token_urlsafe(24)
        self.started = threading.Event()

    @property
    def endpoint(self) -> str:
        host, port = self.server_address[:2]
        return f"http://{host}:{port}"


class _Handler(BaseHTTPRequestHandler):
    server_version = "captured/" + __version__
    protocol_version = "HTTP/1.1"

    # -- plumbing --------------------------------------------------------------

    def log_message(self, fmt: str, *args) -> None:  # route to stderr logger
        log.info("%s - %s", self.address_string(), fmt % args)

    def _send(self, status: int, obj: dict) -> None:
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_json(self) -> dict:
        n = int(self.headers.get("Content-Length", 0) or 0)
        if not n:
            return {}
        try:
            return json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            raise _ApiError(400, "invalid JSON body")

    def _authed(self) -> bool:
        want = f"Bearer {self.server.token}"
        return secrets.compare_digest(self.headers.get("Authorization", ""), want)

    # -- dispatch --------------------------------------------------------------

    def do_GET(self) -> None:
        self._dispatch("GET")

    def do_POST(self) -> None:
        self._dispatch("POST")

    def _dispatch(self, method: str) -> None:
        url = urlparse(self.path)
        parts = [p for p in url.path.split("/") if p]  # e.g. ["v1","sessions","abc","stop"]
        q = parse_qs(url.query)
        try:
            if parts[:1] != ["v1"]:
                raise _ApiError(404, "not found")
            rest = parts[1:]
            # /v1/health is the only unauthenticated route (liveness probe).
            if method == "GET" and rest == ["health"]:
                return self._send(200, self._health())
            if not self._authed():
                raise _ApiError(401, "missing or invalid bearer token")
            self._send(*self._route(method, rest, q))
        except _ApiError as e:
            self._send(e.status, {"error": e.message})
        except Exception as e:  # never leak a stack trace to the client
            log.exception("daemon handler error")
            self._send(500, {"error": f"{type(e).__name__}: {e}"})

    def _route(self, method: str, rest: list[str], q: dict) -> tuple[int, dict]:
        reg = self.server.registry
        if method == "GET" and rest == ["windows"]:
            pid = int(q["pid"][0]) if "pid" in q else None
            app = q.get("app_name", [None])[0]
            wins = list_windows(pid=pid, app_name=app)
            return 200, {"windows": wins, "count": len(wins)}
        if method == "POST" and rest == ["sessions"]:
            return 201, self._start_session(self._read_json())
        if method == "GET" and rest == ["sessions"]:
            return 200, {"sessions": reg.summaries()}
        if rest[:1] == ["sessions"] and len(rest) >= 2:
            sid = rest[1]
            if method == "GET" and len(rest) == 2:
                s = reg.summary(sid)
                if s is None:
                    raise _ApiError(404, f"unknown session_id {sid!r}")
                return 200, s
            if method == "POST" and rest[2:] == ["stop"]:
                return 200, self._stop_session(sid)
            if method == "GET" and rest[2:] == ["transcript"]:
                tail = int(q["tail"][0]) if "tail" in q else None
                return 200, self._transcript(sid, tail)
        if method == "POST" and rest == ["admin", "shutdown"]:
            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return 200, {"shutdown": True}
        raise _ApiError(404, "not found")

    # -- handlers --------------------------------------------------------------

    def _health(self) -> dict:
        reg = self.server.registry
        with reg._lock:  # cheap snapshot of counts
            live = len(reg._live)
            history = len(reg._history)
        return {
            "ok": True,
            "version": __version__,
            "api_version": API_VERSION,
            "pid": os.getpid(),
            "platform": __import__("sys").platform,
            "sessions": {"live": live, "history": history},
        }

    def _start_session(self, body: dict) -> dict:
        unknown = set(body) - set(_SESSION_ARGS) - {"output_dir"}
        if unknown:
            raise _ApiError(400, f"unknown field(s): {', '.join(sorted(unknown))}")
        output_dir = body.get("output_dir")
        if not output_dir or not str(output_dir).strip():
            raise _ApiError(400, "output_dir is required")
        provided = [n for n in ("command", "pid", "app_name") if _present(body.get(n))]
        if len(provided) != 1:
            raise _ApiError(400, "specify exactly one target: command, pid, or app_name")
        kwargs = {k: body[k] for k in _SESSION_ARGS if k in body}
        session = CaptureSession(output_dir, **kwargs)
        # Register BEFORE start so a slow/failed start is still visible (state
        # "starting"/"error"), same contract as the MCP server.
        self.server.registry.add(session)
        try:
            return session.start()
        except Exception as e:
            raise _ApiError(400, f"capture failed to start: {e}")

    def _stop_session(self, sid: str) -> dict:
        reg = self.server.registry
        s = reg.get(sid)
        if s is not None:
            return s.stop()
        rec = reg.history_record(sid)
        if rec is not None:
            return rec  # already finished (recovered)
        raise _ApiError(404, f"unknown session_id {sid!r}")

    def _transcript(self, sid: str, tail: int | None) -> dict:
        reg = self.server.registry
        summary = reg.summary(sid)
        if summary is None:
            raise _ApiError(404, f"unknown session_id {sid!r}")
        path = Path(summary["dir"]) / "transcript.jsonl"
        segs: list[dict] = []
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
            for ln in lines:
                ln = ln.strip()
                if ln:
                    segs.append(json.loads(ln))
        except FileNotFoundError:
            segs = []
        if tail is not None and tail >= 0:
            segs = segs[-tail:]
        return {"session_id": sid, "segments": segs, "count": len(segs)}


def write_daemon_json(daemon: CaptureDaemon, path: Path | None = None) -> Path:
    path = path or daemon_json_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    data = {
        "endpoint": daemon.endpoint,
        "token": daemon.token,
        "pid": os.getpid(),
        "api_version": API_VERSION,
        "version": __version__,
    }
    # Write 0600 (token is a secret) — create the file restricted, then write.
    fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(data, f)
    return path


def _existing_daemon_alive(path: Path) -> bool:
    """True iff daemon.json points at a daemon that answers /v1/health."""
    try:
        info = json.loads(path.read_text())
    except Exception:
        return False
    try:
        import urllib.request
        with urllib.request.urlopen(info["endpoint"] + "/v1/health", timeout=1.5) as r:
            return json.load(r).get("ok") is True
    except Exception:
        return False


def run_daemon(host: str = "127.0.0.1") -> None:
    """Start the daemon, write discovery JSON, serve until shutdown/signal."""
    path = daemon_json_path()
    if path.exists() and _existing_daemon_alive(path):
        log.error("a daemon is already running (%s); refusing to start a second", path)
        raise SystemExit(3)

    daemon = CaptureDaemon(host=host)
    write_daemon_json(daemon, path)
    log.info("captured %s listening on %s (api %s)", __version__, daemon.endpoint, API_VERSION)
    daemon.started.set()
    try:
        daemon.serve_forever()
    finally:
        try:
            if path.exists():
                path.unlink()
        except Exception:
            pass
        daemon.server_close()
        log.info("captured stopped")


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        stream=__import__("sys").stderr,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    run_daemon()


if __name__ == "__main__":
    main()
