"""Launch a target process and tee its stdout/stderr to timestamped log files.

Two log views are written:
  * ``stdout.log`` / ``stderr.log`` - raw streams, one line per source line.
  * ``output.log``                  - merged, each line prefixed with an ISO
                                      timestamp and stream tag, e.g.
                                      ``2026-06-07T09:47:01.250Z [out] hello``.

Only available in *launch* mode. When attaching to an already-running pid the
kernel gives us no handle on its existing stdout/stderr, so log capture is
skipped (screenshots + audio still work).
"""

from __future__ import annotations

import logging
import subprocess
import threading
from pathlib import Path

from .util import iso, now, split_command

log = logging.getLogger(__name__)


class ProcessCapture:
    def __init__(self, command: str | list[str], out_dir: Path, *, cwd: str | None = None) -> None:
        self.command = command
        self.out_dir = out_dir
        self.cwd = cwd

        self.proc: subprocess.Popen | None = None
        self._threads: list[threading.Thread] = []
        self._merged_lock = threading.Lock()
        self._merged = None  # type: ignore[assignment]
        self._closed = False
        self.lines = 0

    @property
    def pid(self) -> int | None:
        return self.proc.pid if self.proc else None

    def start(self) -> int:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        args = split_command(self.command) if isinstance(self.command, str) else list(self.command)

        self.proc = subprocess.Popen(
            args,
            cwd=self.cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=1,
            text=True,
        )
        # Past this point a failure must tear the child down, or it can deadlock
        # on a full pipe with nobody draining it.
        try:
            self._merged = open(self.out_dir / "output.log", "w", buffering=1)
            self._spawn_pump(self.proc.stdout, self.out_dir / "stdout.log", "out")
            self._spawn_pump(self.proc.stderr, self.out_dir / "stderr.log", "err")
        except Exception:
            log.exception("ProcessCapture.start failed after launch; tearing down child")
            self._teardown_child()
            if self._merged:
                self._merged.close()
                self._merged = None
            raise
        log.info("launched pid=%s: %s", self.proc.pid, args)
        return self.proc.pid

    def _teardown_child(self) -> None:
        if not self.proc:
            return
        if self.proc.poll() is None:
            self.proc.kill()
            try:
                self.proc.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                pass
        for s in (self.proc.stdout, self.proc.stderr):
            try:
                if s:
                    s.close()
            except Exception:
                pass

    def _spawn_pump(self, stream, raw_path: Path, tag: str) -> None:
        def pump() -> None:
            try:
                raw = open(raw_path, "w", buffering=1)
            except Exception:
                # Still drain the stream so the child never blocks on a full pipe.
                log.exception("could not open %s; draining %s without raw log", raw_path, tag)
                for _ in stream:
                    pass
                return
            try:
                for line in stream:
                    raw.write(line)
                    stamp = f"{iso(now())} [{tag}] {line if line.endswith(chr(10)) else line + chr(10)}"
                    with self._merged_lock:
                        if self._merged and not self._closed:
                            self._merged.write(stamp)
                            self.lines += 1
            finally:
                raw.close()

        t = threading.Thread(target=pump, name=f"pump-{tag}", daemon=True)
        t.start()
        self._threads.append(t)

    def poll(self) -> int | None:
        return self.proc.poll() if self.proc else None

    def stop(self, timeout: float = 5.0) -> int | None:
        if not self.proc:
            return None
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                try:
                    self.proc.wait(timeout=2.0)
                except subprocess.TimeoutExpired:
                    pass
        rc = self.proc.poll()
        # Closing the PIPE fds forces the pumps to EOF so their joins return.
        for s in (self.proc.stdout, self.proc.stderr):
            try:
                if s and not s.closed:
                    s.close()
            except Exception:
                pass
        for t in self._threads:
            t.join(timeout=2.0)
        with self._merged_lock:
            self._closed = True
            if self._merged:
                self._merged.flush()
                self._merged.close()
                self._merged = None
        return rc
