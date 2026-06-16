"""CaptureSession: orchestrates process launch, screenshots, and audio/ASR.

Layout of a session directory::

    <output_dir>/capture-<stamp>-<id>/
        session.json        metadata + final summary
        stdout.log          raw stdout            (launch mode only)
        stderr.log          raw stderr            (launch mode only)
        output.log          merged, timestamped   (launch mode only)
        screenshots/        <iso-stamp>.png, one per interval
        transcript.jsonl    {start,end,offset,text} per recognized segment
        transcript.txt      human-readable, timestamped
        audio.s16le         raw captured audio (16 kHz mono s16le)
        events.jsonl        state transitions + periodic counter snapshots
"""

from __future__ import annotations

import json
import logging
import os
import secrets
import threading
from pathlib import Path

from . import platform as _platform
from .audio import AudioCapture
from .events import EventBus, EventsFileWriter
from .proc import ProcessCapture
from .screenshots import Screenshotter, parse_resolution
from .util import fs_stamp, iso, now

log = logging.getLogger(__name__)


class CaptureSession:
    def __init__(
        self,
        output_dir: str,
        *,
        command: str | None = None,
        pid: int | None = None,
        window_id: int | None = None,
        app_name: str | None = None,
        bundle_id: str | None = None,
        screenshot_interval: float = 1.0,
        screenshot_format: str = "png",
        screenshot_resolution: str | None = None,
        screenshot_jpeg_quality: int | None = None,
        capture_screenshots: bool = True,
        capture_audio: bool = True,
        audio_source: str = "auto",
        mic_device: str | None = None,
        audio_chunk_seconds: float = 8.0,
        asr_backend: str = "auto",
        cwd: str | None = None,
    ) -> None:
        stamp = fs_stamp()
        self.id = f"{stamp}-{secrets.token_hex(3)}"
        self.dir = Path(output_dir).expanduser().resolve() / f"capture-{self.id}"

        self.command = command
        self.req_pid = pid
        self.window_id = window_id  # exact picker window for screenshots (None = primary)
        self.app_name = app_name
        self.bundle_id = bundle_id
        self.screenshot_interval = screenshot_interval
        # A "WxH/fmt" resolution spec may override screenshot_format.
        parsed = parse_resolution(screenshot_resolution)
        self.screenshot_resolution = (parsed[0], parsed[1]) if parsed else None
        self.screenshot_format = (parsed[2] if parsed and parsed[2] else screenshot_format).lower()
        self.screenshot_resolution_spec = screenshot_resolution
        self.screenshot_jpeg_quality = screenshot_jpeg_quality
        self.capture_screenshots = capture_screenshots
        self.capture_audio = capture_audio
        self.audio_source = audio_source
        self.mic_device = mic_device  # if set, also record this input device as a separate mic track
        self.audio_chunk_seconds = audio_chunk_seconds
        self.asr_backend = asr_backend
        self.cwd = cwd

        self.t0: float | None = None
        self.t1: float | None = None
        self.pid: int | None = None
        self.window_title: str | None = None
        self.state = "created"
        self.notes: list[str] = []

        self._proc: ProcessCapture | None = None
        self._shots: Screenshotter | None = None
        self._audio: AudioCapture | None = None
        self._mic: AudioCapture | None = None  # optional separate mic track
        self._lock = threading.Lock()

        # Event surface (M0b): components publish to the bus; the writer tails
        # it into <dir>/events.jsonl (state transitions + counter snapshots).
        self.events = EventBus()
        self._events_writer: EventsFileWriter | None = None

    # -- public api -----------------------------------------------------------

    def start(self) -> dict:
        with self._lock:
            if self.state != "created":
                raise RuntimeError(f"session already {self.state}")
            self.dir.mkdir(parents=True, exist_ok=True)
            self.t0 = now()
            self.state = "starting"
            self._write_metadata()
            self._events_writer = EventsFileWriter(
                self.dir / "events.jsonl",
                self.events,
                self.summary,
                interval=float(os.environ.get("CAPTURE_EVENTS_SNAPSHOT_SECONDS", "5.0")),
            )
            self._events_writer.start()
            self.events.publish("state", state="starting", session_id=self.id)

        # Component startup can be slow (subprocess launch, ASR model load) and
        # runs OUTSIDE the lock — mirroring stop()'s teardown — so concurrent
        # stop()/state reads return immediately, observing "starting". stop()
        # is a documented no-op until the session reaches "running".
        try:
            self._resolve_target()
            # A launch-mode session whose command never started has captured
            # nothing — fail loudly rather than report a phantom 'running'.
            if self.command and self._proc is None:
                raise RuntimeError(self.notes[-1] if self.notes else "could not launch command")
            if self.capture_screenshots:
                self._start_screenshots()
            if self.capture_audio:
                self._start_audio()
        except Exception:
            self._stop_components()
            with self._lock:
                self.state = "error"
                self._write_metadata()
            self.events.publish("state", state="error", session_id=self.id)
            self._close_events()
            raise

        with self._lock:
            self.state = "running"
            self._write_metadata()
        self.events.publish("state", state="running", session_id=self.id)
        log.info("session %s started in %s (pid=%s)", self.id, self.dir, self.pid)
        return self.summary()

    def stop(self) -> dict:
        with self._lock:
            if self.state not in ("running",):
                return self.summary()
            self.state = "stopping"
        self.events.publish("state", state="stopping", session_id=self.id)

        # Heavy component teardown runs outside the lock so it never blocks
        # status queries; the final state transition re-takes the lock.
        rc = self._stop_components()

        with self._lock:
            if self._proc is not None:
                self.notes.append(f"process exit code: {rc}")
            self.t1 = now()
            self.state = "stopped"
            self._write_metadata()
        self.events.publish("state", state="stopped", session_id=self.id)
        self._close_events()
        log.info("session %s stopped", self.id)
        return self.summary()

    def _close_events(self) -> None:
        if self._events_writer:
            try:
                self._events_writer.stop()
            except Exception:
                log.exception("events writer stop failed")
            self._events_writer = None

    def _stop_components(self) -> int | None:
        """Stop any started capture components, best-effort. Returns proc rc."""
        if self._shots:
            try:
                self._shots.stop()
            except Exception:
                log.exception("screenshotter stop failed")
        if self._audio:
            try:
                self._audio.stop()
            except Exception:
                log.exception("audio stop failed")
        if self._mic:
            try:
                self._mic.stop()
            except Exception:
                log.exception("mic stop failed")
        if self._proc:
            try:
                return self._proc.stop()
            except Exception:
                log.exception("process stop failed")
        return None

    # -- setup helpers --------------------------------------------------------

    def _resolve_target(self) -> None:
        if self.command:
            self._proc = ProcessCapture(self.command, self.dir, cwd=self.cwd, emit=self.events.publish)
            try:
                self.pid = self._proc.start()
            except Exception as e:
                self._proc = None
                self.notes.append(f"launch failed: {e}")
                log.exception("failed to launch command")
            return

        if self.req_pid is not None:
            self.pid = self.req_pid
        elif self.app_name:
            w = _platform.current().window_finder.primary(app_name=self.app_name)
            if w:
                self.pid = w.pid
                self.window_title = w.title or w.app_name
            else:
                self.notes.append(f"no on-screen window found for app {self.app_name!r}")

        # If a specific window was picked, label the session with ITS title (not the
        # process's primary window) so the summary matches what's being screenshotted.
        if self.window_id is not None:
            match = next(
                (
                    w
                    for w in _platform.current().window_finder.find(pid=self.pid, app_name=self.app_name)
                    if w.window_id == self.window_id
                ),
                None,
            )
            if match:
                self.window_title = match.title or match.app_name

    def _start_screenshots(self) -> None:
        # In attach-by-app mode, pass app_name so the window is re-resolved each
        # tick even before/after the pid is known.
        self._shots = Screenshotter(
            self.dir / "screenshots",
            pid=self.pid,
            window_id=self.window_id,
            app_name=None if self.pid else self.app_name,
            interval=self.screenshot_interval,
            fmt=self.screenshot_format,
            resolution=self.screenshot_resolution,
            jpeg_quality=self.screenshot_jpeg_quality,
            emit=self.events.publish,
        )
        self._shots.start()

    def _start_audio(self) -> None:
        self._audio = AudioCapture(
            self.dir,
            pid=self.pid,
            bundle_id=self.bundle_id,
            source=self.audio_source,
            chunk_seconds=self.audio_chunk_seconds,
            asr_backend=self.asr_backend,
            t0=self.t0,
            emit=self.events.publish,
        )
        self._audio.start()
        if self._audio.status.startswith("asr-unavailable") or self._audio.status == "no-audio-source":
            self.notes.append(f"audio: {self._audio.status}")

        # Optional SEPARATE microphone track: a second AudioCapture that writes
        # mic.s16le / mic_transcript.* alongside the app audio (never mixed). Only
        # the session the GUI assigns the mic to gets this.
        if self.mic_device is not None:
            self._mic = AudioCapture(
                self.dir,
                source="mic",
                mic_device=self.mic_device,
                track="mic",
                chunk_seconds=self.audio_chunk_seconds,
                asr_backend=self.asr_backend,
                t0=self.t0,
                emit=self.events.publish,
            )
            self._mic.start()
            if self._mic.status.startswith("asr-unavailable") or self._mic.status == "no-audio-source":
                self.notes.append(f"mic: {self._mic.status}")

    # -- reporting ------------------------------------------------------------

    def summary(self) -> dict:
        return {
            "session_id": self.id,
            "state": self.state,
            "dir": str(self.dir),
            "pid": self.pid,
            "window_title": self.window_title,
            "started_at": iso(self.t0) if self.t0 else None,
            "stopped_at": iso(self.t1) if self.t1 else None,
            "screenshots": self._shots.count if self._shots else 0,
            "screenshot_errors": self._shots.errors if self._shots else 0,
            "log_lines": self._proc.lines if self._proc else 0,
            "process_running": (self._proc.poll() is None) if self._proc else None,
            "audio_mode": self._audio.mode if self._audio else "off",
            "audio_status": self._audio.status if self._audio else "off",
            "transcript_segments": self._audio.segments if self._audio else 0,
            "asr_errors": self._audio.asr_errors if self._audio else 0,
            "mic_status": self._mic.status if self._mic else ("off" if self.mic_device is None else "init"),
            "mic_segments": self._mic.segments if self._mic else 0,
            "notes": list(self.notes),  # snapshot; notes may be appended concurrently
        }

    def _write_metadata(self) -> None:
        meta = {
            "config": {
                "command": self.command,
                "pid": self.req_pid,
                "app_name": self.app_name,
                "bundle_id": self.bundle_id,
                "screenshot_interval": self.screenshot_interval,
                "screenshot_format": self.screenshot_format,
                "screenshot_resolution": self.screenshot_resolution_spec,
                "screenshot_jpeg_quality": self.screenshot_jpeg_quality,
                "capture_screenshots": self.capture_screenshots,
                "capture_audio": self.capture_audio,
                "audio_source": self.audio_source,
                "mic_device": self.mic_device,
                "audio_chunk_seconds": self.audio_chunk_seconds,
                "asr_backend": self.asr_backend,
                "cwd": self.cwd,
            },
            "summary": self.summary(),
        }
        try:
            (self.dir / "session.json").write_text(json.dumps(meta, indent=2, ensure_ascii=False))
        except Exception:
            log.exception("failed to write session.json")
