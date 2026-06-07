# Spec: Platform Abstraction

_Status: **current** as of 2026-06-07. Source of truth = the code; update this spec in the
same change as the code. (Implemented in Session 6; was PLANNED before.)_

## Purpose
Let capture-mcp run on **Windows (with an NVIDIA GPU)** as well as macOS without changing the
MCP tool surface, the session lifecycle, or the on-disk output contract. The three OS-specific
concerns — window discovery, screenshots, and per-app audio — sit behind interfaces; everything
above them (session orchestration, screenshot scheduling, audio chunking, ASR, transcripts) is
shared. `platform.current()` returns the backend for the running OS. Follow-up development runs
on a Windows/NVIDIA box, which also enables the Whisper-vs-Nemotron benchmark (feature #23).

## Files
- `src/capture_mcp/platform/base.py` — interfaces `WindowFinder`, `ScreenGrabber`,
  `AudioSource`; the `WindowRef` dataclass; the `fit_box(sw,sh,bw,bh)` helper; and the
  `Platform` aggregate (holds one of each backend).
- `src/capture_mcp/platform/__init__.py` — `current()` factory + re-exports. Selects a backend
  by `sys.platform` (`darwin`→macos, `win32`→windows), overridable by env `CAPTURE_PLATFORM`,
  and caches one `Platform` per resolved name (`_cache`).
- `src/capture_mcp/platform/macos.py` — `MacWindowFinder` (delegates to `capture_mcp.windows`,
  the Quartz module), `MacScreenGrabber` (`screencapture` + `sips`), `MacAudioSource` (the
  ScreenCaptureKit `audiocap` helper, else `ffmpeg` `avfoundation`), `MacOSPlatform`. Also owns
  `helper_path()` and the screenshot helpers `_sc_format`/`_sips_format`/`_png_size`.
- `src/capture_mcp/platform/windows.py` — `Win32WindowFinder` (`EnumWindows`), `Win32ScreenGrabber`
  (GDI `BitBlt`/`PrintWindow` → GDI+ scale + encode; sets per-monitor **DPI awareness** at import so
  captures aren't cropped on a scaled display), `Win32AudioSource` (WASAPI system loopback),
  `WindowsPlatform`.
- `helper/audiocap_win.py` — Windows audio helper (analogue of `audiocap.swift`): WASAPI **loopback**
  of the default output → 16 kHz mono s16le on stdout, with **auto-reconnect** on a stream error or
  default-output-device change. Launched by `Win32AudioSource` (with `CREATE_NO_WINDOW`).
- `init.ps1` — Windows bootstrap (venv + editable install + smoke), parallel to `init.sh`.
- `scripts/run_interactive.ps1` — run a command in the interactive desktop session (`WinSta0`) via a
  transient Interactive-logon scheduled task; `-NoWait` leaves it running (fire-and-forget).
- `scripts/{capture_youtube_playlist,transcribe_audio,playlist_deliverables}.py` — application tooling
  for the browser-capture workflow (see [`../youtube-capture.md`](../youtube-capture.md)).

## Public contract
### `WindowRef` (dataclass, base.py)
`window_id: int` (CGWindowID on macOS / HWND on Windows), `pid: int`, `app_name: str`,
`title: str`, `width: int`, `height: int`, and `area` (read-only property = `width*height`).

### `WindowFinder`
- `find(pid=None, app_name=None) -> list[WindowRef]` — matching top-level windows, **largest
  area first** (possibly empty). Abstract.
- `primary(pid=None, app_name=None) -> WindowRef | None` — concrete; returns `find(...)[0]` or
  `None`.

### `ScreenGrabber`
- `capture(window_id, out_path, *, fmt, resolution=None, jpeg_quality=None, timeout=None) -> bool`
  — write one screenshot. `window_id is None` ⇒ whole (primary) screen; otherwise that window.
  Encoded as `fmt`; when `resolution=(w,h)` is given, scaled to fit inside it preserving aspect
  ratio and **never upscaled** (`fit_box`). `jpeg_quality` applies only to jpg/jpeg. Returns
  `True` iff `out_path` was written; the grabber logs the reason on `False` (callers count
  `False` as an error). `timeout` bounds any subprocess a backend shells out to (macOS);
  in-process backends (Windows GDI+) ignore it.

### `AudioSource`
- `command(*, pid, bundle_id, source, rate) -> tuple[list[str], str] | None` — `(argv, mode)`
  for a process whose **stdout is 16 kHz mono signed-16-bit-LE PCM**, or `None` if no source can
  satisfy the request. `source` is `"auto"|"app"|"mic"`; `mode` is the kind selected
  (`"app"|"mic"`).

### `Platform` + `current()`
`Platform` holds `.window_finder`, `.screen_grabber`, `.audio_source`. `current()` returns the
cached `Platform` for the running OS (or the `CAPTURE_PLATFORM` override); raises `RuntimeError`
for an unsupported platform. The MCP tools and the session output layout are unchanged.

## Behavior
1. `screenshots.Screenshotter` resolves the target via `platform.current().window_finder` and
   delegates pixel capture to `platform.current().screen_grabber`; it keeps scheduling, the
   `_last_wid` cross-Space cache, and count/error accounting.
2. `audio.AudioCapture._build_command()` delegates to `platform.current().audio_source.command(...)`
   (returns `(None,"none")` when that yields `None`).
3. `session.CaptureSession` resolves an app-name target via
   `platform.current().window_finder.primary(app_name=...)` (reads `.pid`/`.title`/`.app_name`).
4. `proc.ProcessCapture` tokenizes a string command via `util.split_command` (Windows
   `CommandLineToArgvW`, POSIX `shlex.split`).
5. The factory imports the macOS or Windows backend lazily inside `current()`, so importing the
   package on a third OS succeeds until `current()` is called.

## Mapping
| Concern | macOS | Windows |
|---|---|---|
| Screenshots | `screencapture -l` + `sips` (`MacScreenGrabber`) | GDI `BitBlt`/`PrintWindow` → GDI+ scale+encode (`Win32ScreenGrabber`) — png/jpg/jpeg/tiff/gif/bmp + JPEG quality, zero extra deps |
| Window discovery | Quartz `CGWindowList` (`MacWindowFinder`→`windows.py`) | `EnumWindows` + `QueryFullProcessImageNameW` (`Win32WindowFinder`) |
| Per-app / system audio | ScreenCaptureKit `audiocap` helper (per-app) | WASAPI **system loopback** via `helper/audiocap_win.py` (captures the full output mix incl. the target app; auto-reconnects on device change). True per-**process** loopback is a future refinement. |
| Mic fallback | ffmpeg `avfoundation :default` | ffmpeg `dshow` with `CAPTURE_DSHOW_AUDIO` device (only if ffmpeg present) |
| ASR | local Whisper (mlx/faster) | local Whisper (faster-whisper CUDA) **and** NVIDIA Nemotron via Riva |

## Invariants & constraints
- Audio is **16 kHz mono s16le** end to end on every platform (`SAMPLE_RATE`/`BYTES_PER_SAMPLE`).
- Session directory layout, `session.json`, and transcript formats are identical across platforms.
- The MCP tool parameters/returns do not change.
- `stdout` stays clean in `server.py` (the MCP transport) on all platforms.
- `fit_box` never upscales (`scale = min(bw/sw, bh/sh, 1.0)`) and preserves aspect ratio.
- The Windows screenshot/window backend uses only `ctypes` + DLLs that ship with Windows
  (`user32`, `gdi32`, `gdiplus`, `kernel32`, `dwmapi`) and sets **per-monitor DPI awareness** at
  import so captures use physical pixels. The Windows **audio** path needs `pyaudiowpatch` (+ numpy);
  targeting a window id makes screenshots occlusion-proof (`PrintWindow`), so the user can keep
  working with the captured window in the background.
- Backends are stateless/thread-safe and shared; `Win32ScreenGrabber` starts GDI+ once under a
  lock and caches encoder CLSIDs.

## Failure modes & handling
- Unsupported platform → `current()` raises `RuntimeError` naming `sys.platform` and the override.
- Windows screenshot failure (no DC, GDI+ status ≠ Ok, unsupported `fmt`, exception) → `capture`
  returns `False`, logs the reason, and frees all GDI/GDI+ resources in `finally` (no leak); the
  Screenshotter counts one error for the tick.
- Windows window discovery: a failing per-window query is swallowed inside the `EnumWindows`
  callback (logged) so one bad window cannot abort enumeration.
- Windows audio: `source="auto"|"app"` → the WASAPI loopback helper (`mode="loopback"`), which
  reconnects on a read error or default-device change so a long multi-video capture survives;
  `source="mic"` → ffmpeg `dshow` only if `ffmpeg` + `CAPTURE_DSHOW_AUDIO` are present, else `None`.
  If `pyaudiowpatch` / the helper is missing, `app` → `None` (`no-audio-source`). macOS unchanged.

## Outputs / artifacts
Same as the macOS session output on both platforms. The Windows screenshot backend writes the
final image directly (no temp file; the macOS `sips` path still uses a `.tmp.png`).

## Configuration
- `CAPTURE_PLATFORM=auto|macos|windows` — force a backend (default `auto` = by `sys.platform`).
- `CAPTURE_DSHOW_AUDIO` — Windows dshow microphone device name for the ffmpeg mic fallback.
- Existing: `CAPTURE_WHISPER_MODEL`, `CAPTURE_RIVA_*`.
- Packaging: `pyobjc-framework-Quartz` and `mlx-whisper` are gated by
  `sys_platform == "darwin"` in `pyproject.toml`, so the base package installs on Windows.

## Known limitations / open items
- **Windows audio is system loopback, not per-process** (feature #21 audio half): `audiocap_win.py`
  captures the **default output mix** (the target app plus anything else playing), so other audio
  should be muted for a clean transcript. True per-**process** WASAPI loopback (Win 10 2004+) would
  isolate one app and is the remaining refinement.
- **Loopback can lag wall-clock on long runs.** WASAPI loopback only delivers while audio renders, so
  over a long multi-video capture the audio timeline can fall behind real time and the live
  transcript's absolute timestamps drift vs. wall-clock (this skews wall-time-based per-video
  splitting). For clean offsets, re-transcribe the saved `audio.s16le` offline
  (`scripts/transcribe_audio.py`) and split by content.
- **Windows screenshot content needs an interactive desktop.** Discovery/`PrintWindow` capture of
  real app windows requires the process to run in the interactive window station (`WinSta0`); from
  a non-interactive/service station (a Windows service, SSH, or CI) `EnumWindows` sees no user
  windows and the screen DC is the blank service desktop. **Escape hatch:**
  `scripts/run_interactive.ps1` runs a command in the logged-on user's interactive session via a
  transient Interactive-logon scheduled task. This was used to verify real-window capture
  end-to-end (see Tests).
- `PrintWindow` may return black for some GPU/DWM-composited windows; `PW_RENDERFULLCONTENT` is
  used and it falls back to `BitBlt` from the window DC. A capture-by-screen-region alternative is
  not implemented.
- Windows mic capture requires a configured dshow device (no `:default` exists for dshow).
- CI across both OSes is not set up (feature #19).

## Tests
- `tests/smoke.py` is cross-platform and passes **20/20 on Windows** through the abstraction
  (launch-mode logging + GDI+ whole-screen capture at `640x480/jpg` + audio chunking with stub
  ASR + `parse_resolution`), and remains the macOS hermetic suite.
- Live (Session 6, Windows, service window station): factory returns `windows`;
  `CAPTURE_PLATFORM=macos` override returns the macOS backend object; the per-window GDI+ path
  captured the desktop HWND to a correctly sized 1024×768 PNG; whole-screen+scale+JPEG and
  window+scale+JPEG produced valid files; unsupported `fmt` returns `False`.
- Live (Session 6, Windows, **interactive desktop** via `scripts/run_interactive.ps1`): on the real
  `WinSta0` desktop (1536×864), `EnumWindows` found the actual windows (Chrome, Windows Terminal,
  Notepad); `window_finder.primary(app_name="notepad")` resolved the Notepad window;
  `screen_grabber.capture(window_id, ...)` captured **real Notepad content at its true 1152×594**
  (PrintWindow path) plus a scaled JPEG; whole-screen capture produced the full 1536×864 desktop
  (244 KB of real content). This confirms real-window discovery + content capture, not reachable
  from the service station.
- Live (Session 7, Windows, interactive desktop): captured an 8-video YouTube playlist end-to-end
  via `scripts/capture_youtube_playlist.py` (attach to a signed-in Chrome over the remote-debug port,
  window-targeted GDI+ screenshots, WASAPI loopback → faster-whisper large-v3 CUDA). 51.3 min audio,
  0 errors; the 5 narrated videos transcribed correctly (the 3 non-narrated ones verified against
  their source audio). The loopback auto-reconnect carried the run through a default-device change
  that had truncated an earlier attempt at ~18 min.
- Pending: the Whisper-vs-Nemotron benchmark (#23) and true per-process Windows audio (#21).
