"""TCC-attribution spike daemon (feature #30) — a stand-in for `captured`.

PyInstaller-frozen, launched by a launchd agent from inside CaptureSpike.app.
It does exactly one thing: spawn the bundled `audiocap --system` helper,
count PCM bytes, scan stderr for the helper-contract status lines
(READY / -3801 / -3803 / -3805 — see docs/specs/helper-contract.md), and
write ~/CaptureSpike/status.json every 2s so the spike scripts (and a human)
can see whether audio is flowing WITHOUT any terminal involvement.

If the helper exits (e.g. permission error before the grant), it respawns it
after a short delay — so once the user flips the Screen Recording toggle,
audio starts flowing on the next respawn with no manual restart.

The whole point: the LAUNCHD-SPAWNED, SIGNED-BUNDLE daemon must be the
TCC-responsible process, not any terminal.
"""

from __future__ import annotations

import datetime
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

WORK = Path.home() / "CaptureSpike"
STATUS = WORK / "status.json"
RESPAWN_DELAY = 3.0


def _now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _helper_path() -> Path:
    # PyInstaller .app: sys.executable is CaptureSpike.app/Contents/MacOS/CaptureSpike;
    # audiocap is staged next to it (a Mach-O is legal and signable in MacOS/).
    return Path(sys.executable).resolve().parent / "audiocap"


def _version() -> str:
    try:  # --add-data version.txt lands in the PyInstaller runtime dir
        base = getattr(sys, "_MEIPASS", str(Path(sys.executable).resolve().parent))
        return (Path(base) / "version.txt").read_text().strip()
    except Exception:
        return "unknown"


class State:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.started = _now()
        self.bytes = 0
        self.ready = False
        self.ready_line = ""
        self.last_stderr = ""
        self.permission_error = False
        self.transient_3805 = 0
        self.helper_rc: int | None = None
        self.helper_pid: int | None = None
        self.respawns = 0
        self.stopping = False

    def to_dict(self) -> dict:
        with self.lock:
            return {
                "updated": _now(),
                "daemon_started": self.started,
                "daemon_pid": os.getpid(),
                "daemon_version": _version(),
                "helper_pid": self.helper_pid,
                "helper_rc": self.helper_rc,
                "ready": self.ready,
                "ready_line": self.ready_line,
                "audio_bytes": self.bytes,
                "audio_flowing": self.bytes > 0,
                "permission_error": self.permission_error,
                "transient_3805_count": self.transient_3805,
                "last_stderr": self.last_stderr,
                "respawns": self.respawns,
                "verdict": (
                    "AUDIO FLOWING — attribution works" if self.bytes > 0
                    else "PERMISSION ERROR — grant Screen Recording to this app in System Settings"
                    if self.permission_error
                    else "waiting for helper / grant"
                ),
            }


def write_status(st: State) -> None:
    WORK.mkdir(parents=True, exist_ok=True)
    tmp = STATUS.with_suffix(".tmp")
    tmp.write_text(json.dumps(st.to_dict(), indent=2))
    tmp.replace(STATUS)


def pump_stdout(proc: subprocess.Popen, st: State) -> None:
    while True:
        data = proc.stdout.read(4096)
        if not data:
            return
        with st.lock:
            st.bytes += len(data)


def pump_stderr(proc: subprocess.Popen, st: State) -> None:
    for raw in proc.stderr:
        line = raw.decode(errors="replace").rstrip()
        if not line:
            continue
        with st.lock:
            st.last_stderr = line
            if line.startswith("READY "):
                st.ready = True
                st.ready_line = line
            if re.search(r"-380[13]\b", line) or "permission error" in line:
                st.permission_error = True
            if "-3805" in line:
                st.transient_3805 += 1


def main() -> int:
    st = State()
    helper = _helper_path()

    def stop(*_a):
        with st.lock:
            st.stopping = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)

    proc: subprocess.Popen | None = None
    last_spawn = 0.0
    while True:
        with st.lock:
            stopping = st.stopping
        if stopping:
            break

        if proc is None or proc.poll() is not None:
            if proc is not None:
                with st.lock:
                    st.helper_rc = proc.returncode
                    st.respawns += 1
            if time.time() - last_spawn >= RESPAWN_DELAY:
                last_spawn = time.time()
                try:
                    proc = subprocess.Popen(
                        [str(helper), "--system"],
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                    )
                    with st.lock:
                        st.helper_pid = proc.pid
                    threading.Thread(target=pump_stdout, args=(proc, st), daemon=True).start()
                    threading.Thread(target=pump_stderr, args=(proc, st), daemon=True).start()
                except Exception as e:
                    with st.lock:
                        st.last_stderr = f"spawn failed: {e}"
                    proc = None

        write_status(st)
        time.sleep(2.0)

    if proc is not None and proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    write_status(st)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
