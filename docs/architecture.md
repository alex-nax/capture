# Architecture

> **Specs are mandatory.** Each scope has a detailed spec in [`docs/specs/`](specs/).
> They are the source of *intent*; the code is the source of *reality*; the two must
> agree. **Update the matching spec in the same change as any behavior change** — see the
> "SPECS ARE MANDATORY" section in [`AGENTS.md`](../AGENTS.md). This document covers the
> cross-cutting structure; per-scope detail lives in the specs.

## Module map

```
server.py            MCP entrypoint. Async tools: capture_start/stop/status.
  └─ session.py      CaptureSession: orchestrates one capture; owns components; writes session.json
       ├─ proc.py            ProcessCapture: launch + tee stdout/stderr (launch mode only)
       ├─ screenshots.py     Screenshotter: periodic screencapture (+ sips resize/convert)
       ├─ windows.py         Quartz CGWindowList helpers (pid/app -> CGWindowID)
       └─ audio.py           AudioCapture: drive source -> chunk -> ASR -> transcript
            └─ asr/          ASR backends behind ASRBackend (base.py)
                 ├─ whisper_local.py   mlx-whisper / faster-whisper (local default)
                 ├─ nemotron.py        NVIDIA Riva / Nemotron-3.5 (remote)
                 └─ __init__.py        create(name) factory ("auto" -> local, fallback riva)
helper/audiocap.swift   Standalone ScreenCaptureKit binary: per-app audio -> s16le PCM on stdout
util.py                 timestamps (iso/fs_stamp), shared helpers
```

## Dependency rules
- `server.py` only **orchestrates**: validate args, create/track `CaptureSession`, offload to
  threads. It must not contain capture logic.
- `session.py` owns component lifecycles. Components (`proc`, `screenshots`, `audio`) do **not**
  know about each other or about the MCP layer.
- All ASR access goes through the `ASRBackend` interface. Adding a backend = new module +
  one branch in `asr/__init__.py:create`. Nothing else imports a concrete backend directly.
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
- macOS-only today (screencapture, Quartz, ScreenCaptureKit, sips). Cross-platform would mean
  new `screenshots`/`audio`/`windows` backends behind the same interfaces. arm64 venv required
  for mlx-whisper.
