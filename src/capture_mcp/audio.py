"""Audio capture → chunking → ASR → timestamped transcript.

Source of PCM, in priority order:
  * the compiled ``audiocap`` ScreenCaptureKit helper (per-app audio), or
  * ``ffmpeg`` capturing the default microphone (fallback / when no helper or
    when ``source="mic"`` is requested).

Either source yields raw signed-16-bit-LE mono PCM at 16 kHz on its stdout. We
buffer it into fixed windows and transcribe each window, appending results to
``transcript.jsonl`` (machine) and ``transcript.txt`` (human).

Timestamps: the audio timeline is anchored to the wall-clock arrival of the
*first* PCM bytes (not session start), which corrects for capture-startup
latency. Each segment's stamp is therefore the best estimate of when it was
spoken; it can still drift if the source inserts silence gaps, and recognition
runs on fixed offline windows rather than true streaming (see README).
"""

from __future__ import annotations

import json
import logging
import shutil
import subprocess
import threading
from pathlib import Path

import numpy as np

from . import asr as asr_pkg
from .util import iso, now

log = logging.getLogger(__name__)

SAMPLE_RATE = 16000
BYTES_PER_SAMPLE = 2
MIN_TAIL_BYTES = BYTES_PER_SAMPLE * SAMPLE_RATE // 10  # transcribe tails >= 0.1s


def helper_path() -> Path | None:
    """Path to the built ScreenCaptureKit helper, if present."""
    p = Path(__file__).resolve().parent.parent.parent / "helper" / "audiocap"
    return p if p.exists() else None


class AudioCapture:
    def __init__(
        self,
        out_dir: Path,
        *,
        pid: int | None = None,
        bundle_id: str | None = None,
        source: str = "auto",  # "auto" | "app" | "mic"
        chunk_seconds: float = 8.0,
        asr_backend: str = "auto",
        t0: float | None = None,
    ) -> None:
        self.out_dir = out_dir
        self.pid = pid
        self.bundle_id = bundle_id
        self.source = source
        self.chunk_seconds = max(1.0, float(chunk_seconds))
        self.asr_name = asr_backend
        self.t0 = t0 if t0 is not None else now()

        self._proc: subprocess.Popen | None = None
        self._reader: threading.Thread | None = None
        self._stop = threading.Event()
        self._buf = bytearray()
        self._samples_consumed = 0  # total samples handed to ASR (for offsets)
        self._audio_epoch: float | None = None  # wall clock of first PCM bytes

        self._asr: asr_pkg.ASRBackend | None = None
        self._jsonl = None  # type: ignore[assignment]
        self._txt = None  # type: ignore[assignment]
        self._raw = None  # type: ignore[assignment]

        self.segments = 0
        self.asr_errors = 0
        self.status = "init"
        self.mode = "none"
        self._asr_error: str | None = None
        self._last_stderr = ""
        self._bytes_in = 0

    # -- lifecycle ------------------------------------------------------------

    def start(self) -> None:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        try:
            self._asr = asr_pkg.create(self.asr_name)
        except Exception as e:
            self._asr_error = str(e)
            log.warning("ASR backend unavailable; audio will be recorded but not transcribed: %s", e)
            self._asr = None

        cmd, mode = self._build_command()
        if cmd is None:
            self.status = "no-audio-source"
            log.warning("no audio source available (no helper, no ffmpeg)")
            return
        self.mode = mode

        # Open outputs and launch; roll everything back on any failure so we
        # never leak file handles or an undrained subprocess.
        try:
            self._jsonl = open(self.out_dir / "transcript.jsonl", "w", buffering=1)
            self._txt = open(self.out_dir / "transcript.txt", "w", buffering=1)
            self._raw = open(self.out_dir / "audio.s16le", "wb")
            self._proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        except Exception as e:
            self.status = f"audio-start-failed: {e}"
            log.exception("audio capture failed to start")
            self._teardown_proc()
            self._close_files()
            return

        self._spawn_stderr_logger()
        self._reader = threading.Thread(target=self._read_loop, name="audio-reader", daemon=True)
        self._reader.start()
        # Keep the no-ASR condition visible rather than clobbering it with "running".
        self.status = "running" if self._asr else f"running (asr-unavailable: {self._asr_error})"
        log.info("audio capture started (mode=%s, asr=%s)", self.mode, self._asr.name if self._asr else "none")

    def _build_command(self) -> tuple[list[str] | None, str]:
        want_app = self.source in ("auto", "app")
        hp = helper_path()
        if want_app and hp and (self.pid is not None or self.bundle_id is not None):
            cmd = [str(hp), "--rate", str(SAMPLE_RATE)]
            if self.pid is not None:
                cmd += ["--pid", str(self.pid)]
            elif self.bundle_id is not None:
                cmd += ["--bundle", str(self.bundle_id)]
            return cmd, "app"

        if self.source == "app":
            return None, "none"  # explicitly wanted app audio but can't

        # Microphone fallback via ffmpeg avfoundation.
        if shutil.which("ffmpeg"):
            cmd = [
                "ffmpeg", "-hide_banner", "-loglevel", "warning",
                "-f", "avfoundation", "-i", ":default",
                "-ac", "1", "-ar", str(SAMPLE_RATE),
                "-f", "s16le", "-",
            ]
            return cmd, "mic"
        return None, "none"

    def stop(self) -> None:
        self._stop.set()
        # Kill first so the child's stdout reaches EOF and _read_loop returns; we
        # join the reader BEFORE closing the fd so we never close a stream the
        # reader is still blocked inside (a close+read race on the same object).
        self._kill_proc()

        reader_done = True
        if self._reader:
            self._reader.join(timeout=5.0)
            reader_done = not self._reader.is_alive()
        self._close_proc_stdout()

        # Only touch the buffer / transcript files once the reader is provably
        # gone, else we race its writes. A wedged reader is rare (child is dead).
        if reader_done:
            try:
                self._flush_chunk(final=True)
            except Exception:
                log.exception("final chunk flush failed")
            self._close_files()
        else:
            log.warning("audio reader did not exit; leaving files for GC to avoid a write race")

        if self._asr:
            try:
                self._asr.close()
            except Exception:
                log.exception("asr.close() failed")
        # Preserve a terminal failure reason; a healthy run becomes "stopped",
        # but a non-zero asr_errors count stays visible.
        if not any(k in self.status for k in ("failed", "unavailable", "no-audio-source")):
            self.status = f"stopped (asr-errors={self.asr_errors})" if self.asr_errors else "stopped"

    def _teardown_proc(self) -> None:
        self._kill_proc()
        self._close_proc_stdout()

    def _kill_proc(self) -> None:
        if not self._proc:
            return
        if self._proc.poll() is None:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=3.0)
            except subprocess.TimeoutExpired:
                self._proc.kill()
                try:
                    self._proc.wait(timeout=2.0)
                except subprocess.TimeoutExpired:
                    pass

    def _close_proc_stdout(self) -> None:
        try:
            if self._proc and self._proc.stdout and not self._proc.stdout.closed:
                self._proc.stdout.close()
        except Exception:
            pass

    def _close_files(self) -> None:
        for f in (self._raw, self._jsonl, self._txt):
            try:
                if f:
                    f.flush()
                    f.close()
            except Exception:
                pass
        self._raw = self._jsonl = self._txt = None

    # -- internals ------------------------------------------------------------

    def _spawn_stderr_logger(self) -> None:
        def pump() -> None:
            assert self._proc and self._proc.stderr
            for raw in self._proc.stderr:
                line = raw.decode(errors="replace").rstrip()
                if line:
                    self._last_stderr = line
                    log.info("[audiocap] %s", line)

        threading.Thread(target=pump, name="audio-stderr", daemon=True).start()

    @property
    def _chunk_bytes(self) -> int:
        return int(self.chunk_seconds * SAMPLE_RATE) * BYTES_PER_SAMPLE

    def _read_loop(self) -> None:
        assert self._proc and self._proc.stdout
        stdout = self._proc.stdout
        while not self._stop.is_set():
            try:
                data = stdout.read(4096)
            except (ValueError, OSError):
                break  # stdout closed by stop()
            if not data:
                break
            if self._audio_epoch is None:
                self._audio_epoch = now()
            self._bytes_in += len(data)
            if self._raw:
                self._raw.write(data)
            self._buf.extend(data)
            while len(self._buf) >= self._chunk_bytes:
                chunk = bytes(self._buf[: self._chunk_bytes])
                del self._buf[: self._chunk_bytes]
                self._transcribe(chunk)

        # The source ended. If it exited abnormally before producing any audio,
        # surface why (e.g. the ScreenCaptureKit helper hitting a permission /
        # -3805 connection error) instead of silently reporting an empty capture.
        if not self._stop.is_set() and self._bytes_in == 0 and self._proc:
            rc = self._proc.poll()
            detail = self._last_stderr or "no output"
            self.status = f"{self.mode}-audio-failed (rc={rc}): {detail}"
            if self.mode == "app":
                self.status += (
                    "  [per-app audio needs Screen Recording permission for the "
                    "helper; -3805 means the grant is missing/stale — see README]"
                )
            log.warning("audio source produced no data: %s", self.status)

    def _flush_chunk(self, final: bool = False) -> None:
        if final and len(self._buf) >= MIN_TAIL_BYTES:
            chunk = bytes(self._buf)
            self._buf.clear()
            self._transcribe(chunk)

    def _transcribe(self, pcm_bytes: bytes) -> None:
        extra = len(pcm_bytes) % BYTES_PER_SAMPLE
        if extra:  # guard against an odd trailing byte from a truncated read
            pcm_bytes = pcm_bytes[: len(pcm_bytes) - extra]
        n_samples = len(pcm_bytes) // BYTES_PER_SAMPLE
        if n_samples == 0:
            return
        chunk_offset = self._samples_consumed / SAMPLE_RATE
        self._samples_consumed += n_samples
        if not self._asr:
            return
        epoch = self._audio_epoch if self._audio_epoch is not None else self.t0
        pcm = np.frombuffer(pcm_bytes, dtype="<i2").astype(np.float32) / 32768.0
        try:
            segments = self._asr.transcribe(pcm, SAMPLE_RATE)
        except Exception:
            self.asr_errors += 1
            if self.asr_errors == 1 or self.asr_errors % 10 == 0:
                log.exception("ASR transcribe failed (#%d) at offset %.2fs", self.asr_errors, chunk_offset)
            self.status = f"running (asr-errors={self.asr_errors})"
            return
        for seg in segments:
            abs_start = epoch + chunk_offset + seg.start
            abs_end = epoch + chunk_offset + seg.end
            rec = {
                "start": iso(abs_start),
                "end": iso(abs_end),
                "start_offset": round(chunk_offset + seg.start, 3),
                "end_offset": round(chunk_offset + seg.end, 3),
                "text": seg.text,
            }
            if self._jsonl:
                self._jsonl.write(json.dumps(rec, ensure_ascii=False) + "\n")
            if self._txt:
                self._txt.write(f"[{rec['start']}] {seg.text}\n")
            self.segments += 1
