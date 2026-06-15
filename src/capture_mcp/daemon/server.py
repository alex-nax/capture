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
import queue
import secrets
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from pydantic import ValidationError

from .. import __version__
from ..core import list_windows
from ..core.registry import SessionRegistry
from ..core.session import CaptureSession
from .models import StartSessionRequest, v1_schema

log = logging.getLogger("capture_mcp.daemon")

API_VERSION = "1.0"


def daemon_json_path() -> Path:
    env = os.environ.get("CAPTURE_DAEMON_JSON")
    return Path(env).expanduser() if env else Path.home() / ".capture" / "daemon.json"


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
        # Connected /v1/events (SSE) clients: one bounded queue each. Session
        # forwarder threads fan events in from each session's EventBus; the SSE
        # handlers fan them back out. Live-only (no replay) — late joiners read
        # events.jsonl for history.
        self._sse_lock = threading.Lock()
        self._sse_queues: set[queue.Queue] = set()

    @property
    def endpoint(self) -> str:
        host, port = self.server_address[:2]
        return f"http://{host}:{port}"

    # -- SSE fan-out -----------------------------------------------------------

    def sse_register(self) -> queue.Queue:
        q: queue.Queue = queue.Queue(maxsize=1000)
        with self._sse_lock:
            self._sse_queues.add(q)
        return q

    def sse_unregister(self, q: queue.Queue) -> None:
        with self._sse_lock:
            self._sse_queues.discard(q)

    def sse_broadcast(self, event: dict) -> None:
        with self._sse_lock:
            qs = list(self._sse_queues)
        for q in qs:
            try:
                q.put_nowait(event)
            except queue.Full:
                pass  # slow client drops events; never block a capture

    def attach_stream(self, session: CaptureSession) -> None:
        """Forward one session's EventBus into the daemon-wide SSE fan-out.

        Subscribe BEFORE the session starts so the starting/running events are
        carried; tag every event with session_id; end after the terminal state.
        """
        sub = session.events.subscribe()

        def forward() -> None:
            try:
                while True:
                    try:
                        ev = sub.get(timeout=2.0)
                    except queue.Empty:
                        if session.state in ("stopped", "error"):
                            break
                        continue
                    self.sse_broadcast({**ev, "session_id": session.id})
                    if ev.get("type") == "state" and ev.get("state") in ("stopped", "error"):
                        break
            finally:
                sub.close()

        threading.Thread(target=forward, name=f"sse-fwd-{session.id}", daemon=True).start()


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
            # /v1/events is a long-lived SSE stream, not a one-shot JSON response.
            if method == "GET" and rest == ["events"]:
                return self._serve_sse()
            self._send(*self._route(method, rest, q))
        except _ApiError as e:
            self._send(e.status, {"error": e.message})
        except Exception as e:  # never leak a stack trace to the client
            log.exception("daemon handler error")
            self._send(500, {"error": f"{type(e).__name__}: {e}"})

    def _route(self, method: str, rest: list[str], q: dict) -> tuple[int, dict]:
        reg = self.server.registry
        if method == "GET" and rest == ["schema"]:
            return 200, v1_schema(API_VERSION)
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

    def _serve_sse(self) -> None:
        """Stream daemon events as Server-Sent Events until the client disconnects.

        Heartbeats (`: ping`) every CAPTURE_SSE_HEARTBEAT_SECONDS keep the
        connection alive and let the writer notice a dead client.
        """
        hb = float(os.environ.get("CAPTURE_SSE_HEARTBEAT_SECONDS", "15"))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        q = self.server.sse_register()
        try:
            self.wfile.write(b": connected\n\n")
            self.wfile.flush()
            while True:
                try:
                    ev = q.get(timeout=hb)
                    payload = ("data: " + json.dumps(ev) + "\n\n").encode()
                except queue.Empty:
                    payload = b": ping\n\n"
                self.wfile.write(payload)
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError, ValueError):
            pass  # client went away
        finally:
            self.server.sse_unregister(q)

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
        # The /v1 contract (pydantic) validates the body: unknown fields, types,
        # exactly-one-target, and output_dir are all enforced here.
        try:
            req = StartSessionRequest.model_validate(body)
        except ValidationError as e:
            errs = e.errors()
            msg = errs[0].get("msg", "invalid request") if errs else "invalid request"
            raise _ApiError(400, msg.removeprefix("Value error, "))
        session = CaptureSession(req.output_dir, **req.session_kwargs())
        # Register + attach the event stream BEFORE start so a slow/failed start
        # is visible (state "starting"/"error") and the starting/running events
        # reach /v1/events subscribers.
        self.server.registry.add(session)
        self.server.attach_stream(session)
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
