"""Import an existing audio/video file as a capture session.

Turns a file the user already has (a meeting recording, a screen grab, a voice memo)
into a normal session: extract its audio to ``audio.s16le`` and, for video, sample
frames into ``screenshots/`` named on the SAME wall-clock timeline as the audio, write
``session.json``, then run ASR over the audio (reusing
:func:`retranscribe.retranscribe_session`). The result shows up in the session list and
the playback scrubber exactly like a live capture — only ``audio_source="import"`` in
its config marks the origin.

Extraction goes through the bundled ``audiocap`` Swift helper (AVFoundation:
``--extract-audio`` / ``--extract-frames`` — no ffmpeg). Audio-only files simply get no
screenshots (``capture_screenshots=False``). The whole flow is offline and needs no
capture permission.
"""

from __future__ import annotations

import json
import logging
import secrets
import subprocess
import sys
from pathlib import Path

from . import retranscribe
from .session import session_capabilities
from .util import fs_stamp, iso, now

log = logging.getLogger(__name__)

SAMPLE_RATE = 16000


def _helper_bin() -> Path:
    """The bundled audiocap helper (same resolution as the live audio path)."""
    from .platform.macos import helper_path  # macOS-only; import lazily

    hp = helper_path()
    if hp is None:
        raise RuntimeError("audiocap helper not found; build it with scripts/build_helper.sh")
    return hp


def import_file(
    path: "str | Path",
    output_dir: "str | Path",
    *,
    asr_backend: str = "auto",
    screenshot_interval: float = 2.0,
    on_progress=None,
) -> dict:
    """Import ``path`` into a new session dir under ``output_dir``; returns its summary.

    ``on_progress(phase, fraction)`` is called as it advances — phases ``extract-audio``,
    ``extract-frames``, ``transcribe``. Raises ``ValueError`` if the file is missing or
    yields no audio. The active Whisper model is used for ASR (set it first via the model
    manager)."""
    src = Path(path).expanduser()
    if not src.is_file():
        raise ValueError(f"file not found: {src}")
    if sys.platform != "darwin":
        # Extraction goes through the macOS AVFoundation audiocap helper; a Windows
        # ffmpeg-based path is planned (docs/specs/windows-release.md). Fail with a clear
        # message instead of a confusing ImportError from the lazy macOS helper import.
        raise NotImplementedError(
            "capture_import is macOS-only for now (uses the AVFoundation audiocap helper); "
            "a cross-platform import path is planned — see docs/specs/windows-release.md."
        )

    helper = _helper_bin()
    # Anchor the timeline at import time: the session sorts as the newest (ids are
    # timestamp-prefixed) and frames + subtitles share this same epoch.
    base = now()
    sid = f"{fs_stamp(base)}-{secrets.token_hex(3)}"
    d = Path(output_dir).expanduser().resolve() / f"capture-{sid}"
    d.mkdir(parents=True, exist_ok=True)

    def progress(phase: str, frac: float) -> None:
        if on_progress:
            try:
                on_progress(phase, max(0.0, min(1.0, frac)))
            except Exception:
                pass

    # 1. Audio → audio.s16le (the helper writes s16le 16k mono to stdout). Exit 3 means
    #    "no audio track" — a silent video still imports as a frames-only session, so we
    #    drop the empty file and continue; any other nonzero code is a real failure.
    progress("extract-audio", 0.0)
    audio_path = d / "audio.s16le"
    with audio_path.open("wb") as af:
        proc = subprocess.run(
            [str(helper), "--extract-audio", str(src), "--rate", str(SAMPLE_RATE)],
            stdout=af,
            stderr=subprocess.PIPE,
        )
    has_audio = proc.returncode == 0 and audio_path.stat().st_size > 0
    if not has_audio:
        audio_path.unlink(missing_ok=True)
        if proc.returncode not in (0, 3):
            detail = proc.stderr.decode("utf-8", "replace").strip() or f"exit {proc.returncode}"
            raise ValueError(f"could not extract audio from {src.name}: {detail}")
    progress("extract-audio", 1.0)

    # 2. Frames → screenshots/<offset_ms>.png, renamed to fs_stamp(base + ms/1000) so
    #    they sit on the same epoch as the transcript (subtitles line up in playback).
    #    Audio-only files write nothing here (helper exits 0 with no video track).
    progress("extract-frames", 0.0)
    shots = d / "screenshots"
    subprocess.run(
        [str(helper), "--extract-frames", str(src), "--interval", str(screenshot_interval), "--out", str(shots)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    if shots.is_dir():
        for f in sorted(shots.glob("*.png")):
            try:
                ms = int(f.stem)
            except ValueError:
                continue  # leave already-renamed / unexpected files alone
            f.rename(shots / f"{fs_stamp(base + ms / 1000.0)}.png")
    shot_count = sum(1 for _ in shots.glob("*.png")) if shots.is_dir() else 0
    has_frames = shot_count > 0
    progress("extract-frames", 1.0)

    if not has_audio and not has_frames:
        raise ValueError(f"{src.name} has no audio or video track to import")

    # 3. session.json (config + summary) — written BEFORE ASR so retranscribe_session
    #    can recover the epoch from started_at and align segments with the frames.
    started_at = iso(base)
    config = {
        "command": None,
        "pid": None,
        "app_name": None,
        "bundle_id": None,
        "audio_source": "import",
        "source_file": str(src),
        "capture_screenshots": has_frames,
        "capture_audio": has_audio,
        "screenshot_interval": screenshot_interval,
        "asr_backend": asr_backend,
    }
    summary = {
        "session_id": sid,
        "state": "stopped",
        "dir": str(d),
        "pid": None,
        "window_title": src.name,
        "started_at": started_at,
        "stopped_at": started_at,
        "screenshots": shot_count,
        "screenshot_errors": 0,
        "log_lines": 0,
        "process_running": False,
        "audio_mode": "import" if has_audio else "off",
        "audio_status": "imported" if has_audio else "no-audio",
        "transcript_segments": 0,
        "asr_errors": 0,
        "mic_status": "off",
        "mic_segments": 0,
        **session_capabilities(d),
        "notes": [f"imported from {src.name}"],
    }
    _write_session_json(d, config, summary)

    # 4. ASR over the extracted audio (reuses retranscribe; epoch from started_at). Only
    #    when there's audio; a backend failure leaves a valid session rather than aborting.
    def asr_progress(done: int, total: int, segs: int) -> None:
        progress("transcribe", (done / total) if total else 1.0)

    progress("transcribe", 0.0)
    segments = 0
    if has_audio:
        try:
            segments = retranscribe.retranscribe_session(
                d, asr_backend=asr_backend, chunk_seconds=8.0, on_progress=asr_progress
            )
        except Exception as e:
            log.warning("import ASR failed for %s: %s", src.name, e)
            summary["notes"].append(f"transcription failed: {type(e).__name__}: {e}")
    summary["transcript_segments"] = segments
    summary.update(session_capabilities(d))
    _merge_summary(d, {"transcript_segments": segments, **session_capabilities(d), "notes": summary["notes"]})
    progress("transcribe", 1.0)
    log.info("imported %s -> %s (%d frames, %d segments)", src.name, sid, shot_count, segments)
    return summary


def _write_session_json(d: Path, config: dict, summary: dict) -> None:
    (d / "session.json").write_text(
        json.dumps({"config": config, "summary": summary}, indent=2, ensure_ascii=False)
    )


def _merge_summary(d: Path, updates: dict) -> None:
    p = d / "session.json"
    try:
        meta = json.loads(p.read_text())
        meta.setdefault("summary", {}).update(updates)
        p.write_text(json.dumps(meta, indent=2, ensure_ascii=False))
    except Exception:
        log.exception("failed to update import session.json")
