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
  POST /v1/sessions/{id}/delete          -> delete a finished capture (dir + record)
  GET  /v1/sessions/{id}/transcript?tail=N -> last N transcript segments
  GET  /v1/asr/models                    -> Whisper model catalog + downloaded/active
  POST /v1/asr/models/download {repo}    -> download a model (progress via /v1/events)
  POST /v1/asr/models/delete {repo}      -> remove a downloaded model's weights
  GET  /v1/audio/mics                    -> input devices [{id,name,default}] for the mic selector
  POST /v1/asr/model {repo}              -> set the active Whisper model
  GET  /v1/permissions                   -> macOS TCC status (screen_recording)
  POST /v1/permissions/request {kind}    -> trigger the Screen Recording prompt
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
import shutil
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from pydantic import ValidationError

from .. import __version__
from ..core import list_windows
from ..core import permissions as perms
from ..core.asr import manager as asr_manager
from ..core.registry import SessionRegistry
from ..core.session import CaptureSession
from .models import AsrModelRequest, StartSessionRequest, v1_schema

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
        # ASR model downloads in flight (repo -> True), so a duplicate request is a
        # no-op and the model list can show "downloading". Progress is fanned out
        # over /v1/events; the GUI watches those.
        self._asr_lock = threading.Lock()
        self._asr_downloading: set[str] = set()

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

    # -- ASR model downloads ---------------------------------------------------

    def start_asr_download(self, repo: str) -> dict:
        """Download a Whisper model in the background, fanning progress to SSE.

        Returns ``{repo, started}`` immediately; the GUI watches ``/v1/events`` for
        ``asr_download`` (progress), ``asr_download_done`` / ``asr_download_error``.
        A repo already downloading is a no-op (``started: False``).
        """
        with self._asr_lock:
            if repo in self._asr_downloading:
                return {"repo": repo, "started": False, "reason": "already downloading"}
            self._asr_downloading.add(repo)

        def run() -> None:
            last = -1.0  # throttle: only emit when the percent advances by ≥1

            def on_progress(done: int, total: int, fname: str) -> None:
                nonlocal last
                frac = done / total if total else 0.0
                if frac - last < 0.01 and frac < 1.0:
                    return
                last = frac
                self.sse_broadcast(
                    {
                        "type": "asr_download",
                        "repo": repo,
                        "file": fname,
                        "downloaded": done,
                        "total": total,
                        "fraction": round(frac, 4),
                    }
                )

            try:
                asr_manager.download(repo, on_progress=on_progress)
                self.sse_broadcast({"type": "asr_download_done", "repo": repo})
            except Exception as e:
                log.warning("asr model download failed (%s): %s", repo, e)
                self.sse_broadcast(
                    {"type": "asr_download_error", "repo": repo, "error": f"{type(e).__name__}: {e}"}
                )
            finally:
                with self._asr_lock:
                    self._asr_downloading.discard(repo)

        threading.Thread(target=run, name=f"asr-dl-{repo}", daemon=True).start()
        return {"repo": repo, "started": True}


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
            if method == "POST" and rest[2:] == ["delete"]:
                return 200, self._delete_session(sid)
            if method == "GET" and rest[2:] == ["transcript"]:
                tail = int(q["tail"][0]) if "tail" in q else None
                return 200, self._transcript(sid, tail)
        if rest[:1] == ["asr"]:
            return self._route_asr(method, rest, q)
        if method == "GET" and rest == ["audio", "mics"]:
            from ..core import platform as _platform

            return 200, {"devices": _platform.current().audio_source.list_input_devices()}
        if method == "GET" and rest == ["permissions"]:
            return 200, perms.status()
        if method == "POST" and rest == ["permissions", "request"]:
            kind = self._read_json().get("kind") or "screen_recording"
            try:
                return 200, perms.request(kind)
            except ValueError as e:
                raise _ApiError(400, str(e))
        if method == "POST" and rest == ["admin", "shutdown"]:
            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return 200, {"shutdown": True}
        raise _ApiError(404, "not found")

    def _route_asr(self, method: str, rest: list[str], q: dict) -> tuple[int, dict]:
        """ASR model manager: list / download / select the active Whisper model."""
        if method == "GET" and rest == ["asr", "models"]:
            with self.server._asr_lock:
                downloading = set(self.server._asr_downloading)
            return 200, asr_manager.catalog_status(downloading)
        if method == "POST" and rest == ["asr", "models", "download"]:
            return 202, self.server.start_asr_download(self._asr_repo())
        if method == "POST" and rest == ["asr", "models", "delete"]:
            repo = self._asr_repo()
            with self.server._asr_lock:
                if repo in self.server._asr_downloading:
                    raise _ApiError(409, "model is downloading; cannot delete")
            try:
                return 200, asr_manager.delete(repo)
            except ValueError as e:
                raise _ApiError(400, str(e))
        if method == "POST" and rest == ["asr", "model"]:
            try:
                repo = asr_manager.set_active_model(self._asr_repo())
            except ValueError as e:
                raise _ApiError(400, str(e))
            return 200, {"active": repo}
        raise _ApiError(404, "not found")

    def _asr_repo(self) -> str:
        """Validated ``repo`` from a JSON body (AsrModelRequest)."""
        try:
            req = AsrModelRequest.model_validate(self._read_json())
        except ValidationError as e:
            errs = e.errors()
            raise _ApiError(400, errs[0].get("msg", "invalid request") if errs else "invalid request")
        return req.repo

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
        # macOS captures audio per-APPLICATION (ScreenCaptureKit's SCContentFilter is
        # app-scoped, never window-scoped — there is no per-window audio API). If
        # another live session already captures this same process's audio, BOTH
        # transcripts record the identical app-wide stream — e.g. two windows of one
        # browser. Surface that as a note rather than let the duplication look like a
        # bug; screenshots are per-window, but audio can't be split this way.
        if req.capture_audio and req.audio_source in ("auto", "app") and req.pid is not None:
            clash = next(
                (
                    s
                    for s in self.server.registry.running()
                    if s is not session
                    and s.capture_audio
                    and s.audio_source != "mic"
                    and getattr(s, "pid", None) == req.pid
                ),
                None,
            )
            if clash is not None:
                session.notes.append(
                    f"audio: app pid {req.pid} is already captured by session {clash.id}; "
                    "macOS captures audio per-app (not per-window), so both sessions record "
                    "the same audio. Capture from separate processes for distinct audio."
                )
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

    def _delete_session(self, sid: str) -> dict:
        """Delete a finished capture: remove its dir from disk + forget it. 404 if
        unknown, 400 if still live (stop it first)."""
        reg = self.server.registry
        summary = reg.summary(sid)
        if summary is None:
            raise _ApiError(404, f"unknown session_id {sid!r}")
        if summary.get("state") in ("starting", "running", "stopping"):
            raise _ApiError(400, "stop the capture before deleting it")
        # Remove the on-disk dir — but only if it really is a capture dir (has a
        # session.json), never some arbitrary path from a malformed record.
        d = summary.get("dir")
        if d:
            path = Path(d)
            if path.is_dir() and (path / "session.json").exists():
                shutil.rmtree(path, ignore_errors=True)
        try:
            reg.delete(sid)
        except ValueError as e:
            raise _ApiError(400, str(e))
        return {"deleted": True, "session_id": sid}

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
