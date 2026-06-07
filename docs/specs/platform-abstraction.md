# Spec: Platform Abstraction (PLANNED — design, not yet implemented)

_Status: **PLANNED / design** as of 2026-06-07. This describes intended future structure for
running capture-mcp on Windows (NVIDIA) in addition to macOS. It is NOT current behavior — the
code today is macOS-only. Implement against this spec, then flip its Status to "current" and
keep it in sync with the code (specs are mandatory; see AGENTS.md)._

## Purpose
Let capture-mcp run on **Windows (with an NVIDIA GPU)** as well as macOS, without changing the
MCP tool surface or the session/output contract. Follow-up development happens on a Windows PC
so we can also **benchmark local Whisper against NVIDIA Nemotron-3.5 ASR**.

## Files (planned)
- `src/capture_mcp/platform/__init__.py` — `current()` factory selecting a backend by `platform.system()`.
- `src/capture_mcp/platform/base.py` — interfaces: `ScreenGrabber`, `WindowFinder`, `AudioSource`.
- `src/capture_mcp/platform/macos.py` — wraps today's code (screencapture/sips, Quartz, the
  ScreenCaptureKit helper / ffmpeg avfoundation).
- `src/capture_mcp/platform/windows.py` — Windows backends (see below).
- `init.ps1` — Windows bootstrap equivalent of `init.sh`.

## Public contract (planned interfaces)
- `WindowFinder.find(pid=None, app_name=None) -> list[WindowRef]` and `primary(...) -> WindowRef|None`.
- `ScreenGrabber.capture(window_ref|None, out_path, fmt, resolution, quality) -> bool`.
- `AudioSource.command(pid|bundle|system, rate) -> (argv, mode) | None` — yields a process whose
  stdout is **16 kHz mono signed-16-bit-LE PCM** (the existing cross-platform audio contract).
The MCP tools (`capture_start/stop/status`) and the session output layout are unchanged.

## Behavior (planned)
1. `session.py` / `screenshots.py` / `audio.py` / `windows.py` call `platform.current()` instead
   of importing macOS APIs directly.
2. The factory returns the macOS or Windows backend; everything above the interface is shared.

## Mapping
| Concern | macOS (today) | Windows (planned) |
|---|---|---|
| Screenshots | `screencapture -l` + `sips` | Windows.Graphics.Capture, or `PrintWindow`/BitBlt (pywin32), or a small native helper |
| Window discovery | Quartz `CGWindowList` | `EnumWindows`/`GetWindowThreadProcessId` (pywin32) |
| Per-app audio | ScreenCaptureKit helper (`audiocap.swift`) | WASAPI **process loopback** (Win 10 2004+) — lib TBD (e.g. pyaudiowpatch / a small WASAPI helper) |
| Mic fallback | ffmpeg `avfoundation` | ffmpeg `dshow`/`wasapi` |
| ASR | local Whisper (mlx/faster) | local Whisper (faster-whisper CUDA) **and** NVIDIA Nemotron-3.5 via Riva |

## Invariants & constraints
- Audio is **16 kHz mono s16le** end to end on every platform (`SAMPLE_RATE`/`BYTES_PER_SAMPLE`).
- Session directory layout, `session.json`, and transcript formats are identical across platforms.
- The MCP tool parameters/returns do not change.
- Keep `stdout` clean in `server.py` (MCP transport) on all platforms.

## ASR benchmark (planned — Windows/NVIDIA)
Compare on the same captured `audio.s16le` clips:
- **Local Whisper**: faster-whisper CUDA (and mlx on Mac for reference).
- **NVIDIA Nemotron-3.5 ASR**: via the existing Riva adapter (`src/capture_mcp/asr/nemotron.py`,
  `CAPTURE_RIVA_SERVER`/`CAPTURE_RIVA_API_KEY`/`CAPTURE_RIVA_FUNCTION_ID`/`CAPTURE_RIVA_LANG`).
Measure: WER vs a reference, real-time factor / latency, GPU memory. Report in a results doc.

## Outputs / artifacts
Same as the macOS session output. Benchmark adds a results table (location TBD).

## Configuration
Existing env: `CAPTURE_WHISPER_MODEL`, `CAPTURE_RIVA_*`. Planned: a backend override env (e.g.
`CAPTURE_PLATFORM=auto|macos|windows`) for testing.

## Known limitations / open items
- Choose the Windows screenshot API (Graphics Capture vs PrintWindow) and audio loopback lib.
- `init.sh` is bash → needs `init.ps1`; smoke test must run on Windows (avoid macOS-only assumptions).
- Per-app audio on Windows requires Win 10 2004+ process loopback; pre-2004 falls back to system loopback.
- Quartz/pyobjc and mlx are macOS/arm64-only → make them optional deps gated by platform.
- CI across both OSes.

## Tests (planned)
- `tests/smoke.py` must pass on Windows (gate or provide platform backends for the paths it touches).
- A benchmark harness producing the Whisper-vs-Nemotron comparison on identical audio.
