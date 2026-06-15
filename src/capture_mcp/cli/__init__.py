"""`capture` — a thin CLI client of the `captured` daemon.

A peer of the MCP server and (later) the GPUI app: it drives captures through
the daemon's `/v1` API so everything shares one live session registry. Output is
JSON (scriptable). Commands:

  capture daemon start|stop|status   manage the local daemon process
  capture status [SESSION_ID]        health + sessions, or one session
  capture windows [--app N] [--pid P] list capture targets
  capture start --out DIR (--command C | --pid P | --app N) [opts]
  capture stop [SESSION_ID]          stop a capture (the only running one if omitted)
  capture tail SESSION_ID [-n N]     last N transcript segments
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time

from .. import __version__
from ..daemon.client import DaemonClient, DaemonError
from ..daemon.server import daemon_json_path


def _out(obj) -> int:
    print(json.dumps(obj, indent=2, ensure_ascii=False))
    return 0


def _err(msg: str) -> int:
    print(json.dumps({"error": msg}), file=sys.stderr)
    return 1


def _client_or_die() -> DaemonClient:
    c = DaemonClient.from_discovery()
    if c is None or not c.available():
        raise SystemExit(_err("no daemon running; start it with `capture daemon start`"))
    return c


def _daemon_start(_args) -> int:
    existing = DaemonClient.from_discovery()
    if existing is not None and existing.available():
        return _out({"already_running": True, **existing.health()})
    # Detach so the daemon outlives this CLI invocation.
    proc = subprocess.Popen(
        [sys.executable, "-m", "capture_mcp.daemon"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    for _ in range(100):  # up to ~10s for it to come up
        time.sleep(0.1)
        c = DaemonClient.from_discovery()
        if c is not None and c.available():
            return _out({"started": True, "pid": proc.pid, **c.health()})
        if proc.poll() is not None:
            return _err(f"daemon exited immediately (rc={proc.returncode})")
    return _err("daemon did not become ready within 10s")


def _daemon_stop(_args) -> int:
    c = DaemonClient.from_discovery()
    if c is None or not c.available():
        return _out({"stopped": False, "note": "no daemon running"})
    try:
        c.shutdown()
    except DaemonError as e:
        return _err(str(e))
    return _out({"stopped": True})


def _daemon_status(_args) -> int:
    c = DaemonClient.from_discovery()
    if c is None or not c.available():
        return _out({"running": False, "daemon_json": str(daemon_json_path())})
    return _out({"running": True, **c.health()})


def _status(args) -> int:
    c = _client_or_die()
    try:
        return _out(c.session(args.session_id) if args.session_id else c.sessions())
    except DaemonError as e:
        return _err(str(e))


def _windows(args) -> int:
    c = _client_or_die()
    return _out(c.windows(app_name=args.app, pid=args.pid))


def _start(args) -> int:
    c = _client_or_die()
    kwargs: dict = {"output_dir": args.out}
    if args.command is not None:
        kwargs["command"] = args.command
    if args.pid is not None:
        kwargs["pid"] = args.pid
    if args.app is not None:
        kwargs["app_name"] = args.app
    if args.interval is not None:
        kwargs["screenshot_interval"] = args.interval
    if args.no_screenshots:
        kwargs["capture_screenshots"] = False
    if args.no_audio:
        kwargs["capture_audio"] = False
    if args.audio_source is not None:
        kwargs["audio_source"] = args.audio_source
    if args.asr is not None:
        kwargs["asr_backend"] = args.asr
    try:
        return _out(c.start(**kwargs))
    except DaemonError as e:
        return _err(str(e))


def _stop(args) -> int:
    c = _client_or_die()
    sid = args.session_id
    if sid is None:  # stop the unique running session
        running = [s for s in c.sessions()["sessions"] if s.get("state") == "running"]
        if not running:
            return _out({"stopped": [], "note": "no running captures"})
        if len(running) > 1:
            return _err("multiple captures running; pass a session_id: "
                        + ", ".join(s["session_id"] for s in running))
        sid = running[0]["session_id"]
    try:
        return _out(c.stop(sid))
    except DaemonError as e:
        return _err(str(e))


def _tail(args) -> int:
    c = _client_or_die()
    try:
        return _out(c.transcript(args.session_id, tail=args.n))
    except DaemonError as e:
        return _err(str(e))


def _watch(args) -> int:
    c = _client_or_die()
    try:
        for ev in c.events():
            if args.session_id and ev.get("session_id") != args.session_id:
                continue
            print(json.dumps(ev, ensure_ascii=False), flush=True)
    except KeyboardInterrupt:
        pass
    except Exception as e:
        return _err(str(e))
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="capture", description="capture-mcp CLI (daemon client)")
    p.add_argument("--version", action="version", version=f"capture {__version__}")
    sub = p.add_subparsers(dest="cmd", required=True)

    d = sub.add_parser("daemon", help="manage the local daemon").add_subparsers(dest="dcmd", required=True)
    d.add_parser("start", help="start the daemon").set_defaults(func=_daemon_start)
    d.add_parser("stop", help="stop the daemon").set_defaults(func=_daemon_stop)
    d.add_parser("status", help="is the daemon running?").set_defaults(func=_daemon_status)

    s = sub.add_parser("status", help="session status")
    s.add_argument("session_id", nargs="?")
    s.set_defaults(func=_status)

    w = sub.add_parser("windows", help="list capture targets")
    w.add_argument("--app"); w.add_argument("--pid", type=int)
    w.set_defaults(func=_windows)

    st = sub.add_parser("start", help="start a capture")
    st.add_argument("--out", required=True, help="output directory")
    g = st.add_mutually_exclusive_group(required=True)
    g.add_argument("--command"); g.add_argument("--pid", type=int); g.add_argument("--app")
    st.add_argument("--interval", type=float)
    st.add_argument("--no-screenshots", action="store_true")
    st.add_argument("--no-audio", action="store_true")
    st.add_argument("--audio-source", choices=["auto", "app", "mic"])
    st.add_argument("--asr")
    st.set_defaults(func=_start)

    sp = sub.add_parser("stop", help="stop a capture")
    sp.add_argument("session_id", nargs="?")
    sp.set_defaults(func=_stop)

    t = sub.add_parser("tail", help="tail a transcript")
    t.add_argument("session_id")
    t.add_argument("-n", type=int, default=10, help="number of segments (default 10)")
    t.set_defaults(func=_tail)

    wt = sub.add_parser("watch", help="stream live events (Ctrl-C to stop)")
    wt.add_argument("session_id", nargs="?", help="only this session's events")
    wt.set_defaults(func=_watch)
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
