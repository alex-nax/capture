# Architecture

> **Specs are mandatory.** Each scope has a detailed spec in [`docs/specs/`](specs/).
> They are the source of *intent*; the code is the source of *reality*; the two must
> agree. **Update the matching spec in the same change as any behavior change** — see the
> "SPECS ARE MANDATORY" section in [`AGENTS.md`](../AGENTS.md). This document covers the
> cross-cutting structure; per-scope detail lives in the specs.

## Module map

```
server.py            MCP entrypoint (thin frontend). Async tools: capture_start/stop/status.
core/                THE ENGINE — frontend-independent (MCP today; daemon/CLI/GUI per specs/product-architecture.md)
  ├─ registry.py     SessionRegistry: bounded live tracking + disk-backed history (sessions.jsonl index)
  └─ session.py      CaptureSession: orchestrates one capture; owns components; writes session.json
       ├─ proc.py            ProcessCapture: launch + tee stdout/stderr (launch mode only)
       ├─ screenshots.py     Screenshotter: schedule + delegate pixel capture to platform
       └─ audio.py           AudioCapture: drive source -> chunk -> ASR -> transcript
            └─ asr/          ASR backends behind ASRBackend (base.py)
                 ├─ whisper_local.py   mlx-whisper / faster-whisper (local default)
                 ├─ nemotron.py        NVIDIA Riva / Nemotron-3.5 (remote)
                 └─ __init__.py        create(name) factory ("auto" -> local, fallback riva)
core/platform/       OS abstraction: WindowFinder / ScreenGrabber / AudioSource + current() factory
  ├─ base.py            interfaces, WindowRef, fit_box, Platform aggregate
  ├─ macos.py           screencapture/sips, Quartz (via windows.py), audiocap/ffmpeg
  └─ windows.py         GDI+ screenshots, EnumWindows discovery, ffmpeg dshow audio
core/windows.py      macOS Quartz CGWindowList helpers (pid/app -> CGWindowID); used by platform/macos
helper/audiocap.swift   Standalone ScreenCaptureKit binary: per-app audio -> s16le PCM on stdout
core/util.py            timestamps (iso/fs_stamp), split_command, shared helpers
```

## Dependency rules
- `server.py` only **orchestrates**: validate args, create sessions, track them via
  `core.registry.SessionRegistry`, offload to threads. It must not contain capture logic.
- **`core/` must not import any frontend** (no `mcp`, no server module): the engine is
  consumed by the MCP server today and by the daemon/CLI/GUI later (product-architecture.md).
- `session.py` owns component lifecycles. Components (`proc`, `screenshots`, `audio`) do **not**
  know about each other or about the MCP layer.
- All ASR access goes through the `ASRBackend` interface. Adding a backend = new module +
  one branch in `asr/__init__.py:create`. Nothing else imports a concrete backend directly.
- All OS-specific capture goes through the `platform` abstraction (`WindowFinder`/`ScreenGrabber`/
  `AudioSource`). `session`/`screenshots`/`audio` call `platform.current()`; they must not import
  `screencapture`/Quartz/GDI/Win32 APIs directly. Adding an OS = new backend module + one branch
  in `platform/__init__.py:current`.
- The Swift helper is a **process boundary**, not a library: it communicates only via raw PCM
  on stdout and human-readable status on stderr. Keep that contract stable
  (`READY rate=<n> channels=1 fmt=s16le ...` then bytes).

## Hard constraints (encode these in review)
- **stdout is sacred in server.py** — it is the MCP transport. All logging goes to `stderr`
  (`logging.basicConfig(stream=sys.stderr)`). Never `print()` to stdout in the server path.
- **Never block the event loop.** FastMCP runs sync tools on the loop, so tool handlers are
  `async def` and push blocking work (subprocess, ASR load, thread joins) through
  `anyio.to_thread.run_sync`.
- **Capture loops never die.** Screenshot/reader/pump loops catch their own exceptions and
  count errors; one bad tick must not stop the session.
- **Roll back on partial start.** Any component whose `start()` fails must close its files and
  tear down its child process; `CaptureSession.start()` stops already-started components and
  re-raises.
- **Reader-before-files on shutdown.** In `audio.py`, kill the source and join the reader
  thread BEFORE flushing/closing transcript files, to avoid close+write races.
- **Surface failures, don't swallow them.** Audio/ASR failures must remain visible in
  `audio_status` / `asr_errors` (do not overwrite a failure status with "stopped"/"running").

## Naming / conventions
- Timestamps: `util.iso()` for content, `util.fs_stamp()` for filenames (`:` -> `-`).
- Audio is always 16 kHz mono signed-16-bit-LE end to end (`SAMPLE_RATE`, `BYTES_PER_SAMPLE`).
- Session output: `<output_dir>/capture-<fs_stamp>-<token>/` (see README for the layout).

## Platform
- **macOS and Windows**, behind a platform abstraction (`platform/`, see
  [`specs/platform-abstraction.md`](specs/platform-abstraction.md)). `session`/`screenshots`/`audio`
  call `platform.current()` instead of importing OS APIs directly; the macOS backend wraps
  screencapture/Quartz/ScreenCaptureKit/sips, the Windows backend uses GDI+/`EnumWindows`/ctypes
  (no extra deps). The factory selects by `sys.platform` (override `CAPTURE_PLATFORM`).
- OS-only deps are gated in `pyproject.toml` by `sys_platform == "darwin"` (pyobjc/mlx) so the
  base package installs on Windows. macOS arm64 venv is required for mlx-whisper.
- Per-app audio on Windows (WASAPI process loopback) is not yet implemented (feature #21).
