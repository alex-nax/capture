# capture-mcp

An MCP server that captures everything a target process produces, **on demand**:

- **Window video** — timestamped PNG screenshots of the process's window every
  *N* seconds (default 1 s, configurable).
- **Logs** — the process's `stdout`/`stderr`, both raw and merged with per-line
  ISO timestamps.
- **Audio → speech** — the process's audio is captured per-app (macOS
  ScreenCaptureKit) and transcribed by a **pluggable ASR backend**, writing each
  recognized segment with the absolute time it was spoken.

An agent calls `capture_start` to begin saving to a chosen location and
`capture_stop` to end it and stop using disk. `capture_status` reports progress.
Prefer to drive it yourself? There's also a `capture` **CLI** and a native
menu-bar **GUI** — no agent needed (see **[Run it manually](#run-it-manually-without-an-agent--daemon-cli-gui)** below).

> Platform: **macOS** (tested on 15.x, Apple Silicon) and **Windows** (10/11, NVIDIA box).
> OS-specific capture (screenshots, window discovery, audio) lives behind a platform
> abstraction — see [`docs/specs/platform-abstraction.md`](docs/specs/platform-abstraction.md).
> On Windows, screenshots use GDI+ and window discovery uses `EnumWindows` (no extra deps);
> **per-app audio is macOS-only today** (Windows WASAPI process loopback is pending).

## Tools

| Tool | Purpose |
|------|---------|
| `capture_start` | Begin a capture; returns a `session_id` and summary. |
| `capture_stop`  | Stop a capture (or the only running one) and flush to disk. |
| `capture_status`| List sessions / show one session's counters. |

### `capture_start` targets

Pick **one**:

- `command` — a command line to **launch**. This is the only mode that captures
  `stdout`/`stderr`. Its window and audio are tracked once it appears.
- `pid` — attach to a running process by PID.
- `app_name` — attach by app-name substring (e.g. `"Safari"`).

Key options: `output_dir` (required), `screenshot_interval` (s, default 1.0),
`capture_screenshots`, `capture_audio`, `audio_source` (`auto`|`app`|`mic`),
`audio_chunk_seconds`, `asr_backend` (`auto`|`local`|`nemotron`), `bundle_id`.

## Session output layout

```
<output_dir>/capture-<id>/
├── session.json        # config + live/final summary
├── stdout.log          # raw stdout              (launch mode)
├── stderr.log          # raw stderr              (launch mode)
├── output.log          # merged, ISO-timestamped (launch mode)
├── screenshots/
│   └── 2026-06-07T09-47-01.250Z.png ...
├── audio.s16le         # raw captured audio (16 kHz mono s16le)
├── transcript.jsonl    # {start,end,start_offset,end_offset,text} per segment
└── transcript.txt      # [ISO timestamp] recognized text
```

## Install

```bash
cd /Users/alex/capture
uv venv && source .venv/bin/activate
uv pip install -e .                 # core server
uv pip install -e '.[mlx]'          # + Apple-Silicon Whisper (recommended ASR)
# or: uv pip install -e '.[whisper]'  # cross-platform faster-whisper
# or: uv pip install -e '.[riva]'     # remote Nemotron-3.5 via NVIDIA Riva

# Build the per-app audio helper (needs Xcode / command line tools):
bash scripts/build_helper.sh
```

### Windows

```powershell
# From the repo root. Creates .venv, installs (pyobjc is platform-gated out),
# and runs the smoke test (20/20). Add ASR extras as needed.
./init.ps1                          # core server + smoke
./init.ps1 -Extras whisper,riva     # + faster-whisper (CUDA) + NVIDIA Riva client
```

No native build is required on Windows (screenshots use GDI+, window discovery uses
`EnumWindows`, both via `ctypes`). Per-app audio is not yet wired (feature #21); the mic
fallback uses `ffmpeg` `dshow` when `ffmpeg` is installed and `CAPTURE_DSHOW_AUDIO` names a
device. Capturing real app-window content requires the **interactive desktop** session
(`WinSta0`) — if you run from a service/SSH/CI context, use `scripts/run_interactive.ps1` to
execute a command in the logged-on user's session.

Grant **Screen Recording** permission to the program that runs the server
(Terminal / your MCP client) under *System Settings ▸ Privacy & Security ▸ Screen
Recording* — required for both screenshots and per-app audio.

## ASR backends

The ASR layer is pluggable (`src/capture_mcp/core/asr/`):

- **Local (default).** `mlx-whisper` (Apple-Silicon-native) or `faster-whisper`.
  Runs entirely on this Mac; downloads model weights on first use.
- **Nemotron-3.5 ASR (`nemotron`/`riva`).** The 600M NeMo model needs an NVIDIA
  GPU, so it can't run on this Mac directly. The adapter talks to a **Riva**
  server hosting it (self-hosted or NVIDIA-hosted). Configure with env vars:
  `CAPTURE_RIVA_SERVER`, `CAPTURE_RIVA_API_KEY`, `CAPTURE_RIVA_LANG`,
  `CAPTURE_RIVA_MODEL`.

To add a backend, implement `ASRBackend.transcribe(pcm, sample_rate)` in a new
module and register it in `asr/__init__.py:create`.

## Register with an MCP client

Example `claude_desktop_config.json` / `.mcp.json` entry:

```json
{
  "mcpServers": {
    "capture": {
      "command": "/Users/alex/capture/.venv/bin/capture-mcp"
    }
  }
}
```

## Run it manually (without an agent) — daemon, CLI, GUI

Beyond the MCP tools, the engine is also driven by a local **daemon** (an HTTP `/v1`
API), a **`capture` CLI**, and a native **menu-bar GUI** — all thin clients of the
same capture engine and the same live session registry, so a capture started by one
shows up in the others. (Design: [`docs/specs/product-architecture.md`](docs/specs/product-architecture.md);
API: [`docs/specs/daemon.md`](docs/specs/daemon.md); GUI: [`docs/specs/gui.md`](docs/specs/gui.md).)

After `uv pip install -e .` you get three console scripts: `capture-mcp` (MCP server),
`captured` (daemon), `capture` (CLI). If your venv predates them, re-run the install —
or use the `python -m capture_mcp.<daemon|cli|server>` forms shown below.

### CLI (simplest)

```bash
# start the local daemon (spawns it in the background; writes ~/.capture/daemon.json)
capture daemon start                 # or: python -m capture_mcp.cli daemon start

capture windows                      # list on-screen windows (app, pid, title)

# start a capture — pick ONE target: --app / --pid / --command
capture start --out ~/.capture/runs --app "Safari"
#   options: --pid N | --command "cmd" | --interval 2 | --no-audio | --no-screenshots
#            --audio-source app|mic | --asr auto|local|openai|nemotron

capture status                       # all sessions + live counters
capture tail <session_id> -n 20      # last N transcript segments
capture watch                        # stream live events (state/screenshots/transcript); Ctrl-C
capture stop                         # stop the running capture (or pass a <session_id>)
capture daemon stop                  # shut the daemon down
```

Output lands in `<out>/capture-<id>/` (layout above). The daemon keeps sessions alive
across CLI calls; an MCP agent transparently uses the daemon when one is running
(`CAPTURE_MCP_EMBEDDED=1` forces the in-process engine instead).

### Daemon (run it in the foreground)

`capture daemon start` spawns it for you; to run it yourself and watch its logs:

```bash
captured                             # or: python -m capture_mcp.daemon
# serves http://127.0.0.1:<port>; endpoint + bearer token in ~/.capture/daemon.json (0600)
```

### GUI (native menu-bar app, macOS)

```bash
# build once (needs Rust — https://rustup.rs ; gpui's first compile is heavy)
cargo build --manifest-path gui/Cargo.toml          # add --release for an optimized build
# run it (a dev binary has no bundled daemon, so start one: `capture daemon start`)
./gui/target/debug/capture-gui
```

> The **packaged** `Capture.app` (below) bundles its own frozen daemon and auto-spawns it; this
> dev binary doesn't, so it needs a daemon already running.

A window with a daemon-health header, a window picker, Start/Stop, a live session list,
and a live **transcript + screenshot preview** (streamed over `/v1/events`). It also adds
a **menu-bar item** (`● capture` idle, `⦿ N` while N capture) with an Open / Stop-all /
Quit menu, a global hotkey **⌃⌘R** to toggle capture from anywhere, and an **Install skill →**
row that drops the `capture` skill into a coding agent's home (see below). To ship it as a
double-clickable app, see **[Installing the macOS app](#installing-the-macos-app-unsigned-test-build)**.

### Screen Recording note

Per-app audio + screenshots need the **Screen Recording** grant for whichever process
launches the capture — your Terminal, the daemon, or your MCP client. Run from a terminal
you've granted (System Settings ▸ Privacy & Security ▸ Screen Recording). If the helper
prints `displays=0` / `no display available`, the launching process simply isn't granted.

## Installing the macOS app (unsigned test build)

Package the GUI as a double-clickable, **self-contained** `Capture.app` inside a `.dmg` — it
bundles a frozen copy of the daemon, so there's no venv to set up and nothing to start by hand:

```bash
bash packaging/build_macos_dmg.sh        # -> dist/Capture-0.1.0.dmg  (needs Rust + Xcode CLT + ./init.sh venv)
```

The build PyInstaller-freezes the daemon into the app (`Contents/Resources/captured/`, with the
signed `audiocap` helper beside it) and ad-hoc signs everything. Open the DMG and drag
**Capture.app** to **Applications**.

> ⚠️ **This build is NOT notarized and NOT Developer-ID signed** — it is *ad-hoc* signed, for
> testing. macOS **Gatekeeper will block it on first launch** ("Apple could not verify 'Capture'
> is free of malware", or "unidentified developer"). **Bypassing that means you are choosing to
> run an app Apple has not checked — only do it for a build you trust** (one you built yourself,
> or got directly from us). Official builds will be Developer-ID signed + notarized (feature #31).

**Bypass Gatekeeper (first launch only):**

- **Easiest:** **Control-click** (right-click) `Capture.app` → **Open** → **Open** in the dialog.
- **macOS 15 (Sequoia):** double-click → it's blocked → **System Settings ▸ Privacy & Security** →
  scroll to the "'Capture' was blocked" line → **Open Anyway** → confirm with Touch ID/password.
- **Terminal:** strip the download-quarantine flag, then open:
  ```bash
  xattr -dr com.apple.quarantine /Applications/Capture.app
  open /Applications/Capture.app
  ```

**What the app needs / does:**

- It is **self-contained** — on launch the GUI auto-spawns its **bundled** frozen daemon (detached,
  so captures outlive the app) if one isn't already running. No repo, no venv, no
  `capture daemon start`. (If you *do* have a daemon running — e.g. from the repo — the app attaches
  to that one instead.)
- The bundled daemon does **capture + raw audio**. **Transcription** needs an ASR backend: the
  freeze excludes the local mlx-whisper models (too big), so configure a remote one
  (`CAPTURE_ASR_BACKEND=openai_compat` + endpoint) or run the repo daemon for on-device ASR.
- Per-app audio still needs **Screen Recording** granted to the app (the daemon it launches is the
  TCC-responsible process); the bundled `audiocap` helper keeps its stable signing identity so that
  grant persists across rebuilds.

**Install the skill into your coding agent.** The app bundles the `capture` skill and can drop it
into a coding agent's home so the agent can drive capture-mcp from any project. Each button shows
its status and re-installs/updates on click (we ship skill updates this way):

- **Claude Code** → `~/.claude/skills/capture/`
- **Codex** → `~/.codex/skills/capture/`

The button label reflects the state: `— install` (not present), `✓` (up to date), or `↑ update`
(installed but the bundled skill is newer). Headless equivalents:

```bash
capture-gui --skill-status                 # report install/up-to-date/update for each agent
capture-gui --install-skill "Claude Code"  # install/update for the named agent
```

## How per-app audio works

`helper/audiocap.swift` uses ScreenCaptureKit to capture audio from a single
application (matched by PID or bundle id), converts it to 16 kHz mono PCM, and
streams it on stdout. The Python side buffers it into windows, runs ASR, and
timestamps each segment. If the helper isn't built (or `audio_source="mic"`),
it falls back to capturing the default microphone via `ffmpeg`. The helper also
supports `--system` for whole-display audio.

### About SCStreamError -3805

`-3805` is `SCStreamError.failedApplicationConnectionInterrupted` — the stream's
connection to the capture server was interrupted (commonly right after
`startCapture`, or when focus/Spaces change during background capture). It is
**not** a permission denial (that is `-3801 userDeclined`). In practice it is a
*transient* interruption: `SCShareableContent` enumerates fine (so Screen
Recording is granted), and the very next connection attempt usually succeeds.

The helper handles this automatically: on `-3805` it **rebuilds the stream and
reconnects** (with backoff), so background capture survives Space/window switches.
You'll see `stream stopped … code=-3805` followed by `READY … (reconnect #1)` and
`audio flowing` in the helper's stderr. Genuine permission errors
(`-3801`/`-3803`) are *not* retried — they're reported instead.

**Make the Screen Recording grant persist (recommended, one-time):** an ad-hoc
signature changes every rebuild, so macOS re-prompts. Give the helper a stable
identity:

```bash
bash scripts/setup_codesign.sh        # creates a self-signed cert + signs the helper
./helper/audiocap --system            # triggers the Screen Recording prompt
# System Settings → Privacy & Security → Screen Recording → enable 'audiocap'
# then rebuild with the same identity:
CODESIGN_IDENTITY="capture-mcp-codesign" bash scripts/build_helper.sh
```

**Workaround** needing only Microphone permission: `audio_source="mic"`.

The session also degrades gracefully: if audio can't start at all, screenshots and
stdout/stderr logging continue and the failure is reported in `audio_status`.

## Caveats

- Attaching by `pid`/`app_name` **cannot** capture pre-existing `stdout`/`stderr`
  (the kernel gives no handle to them); use `command` launch mode for logs.
- Per-app audio requires macOS 13+ and the Screen Recording permission.
- The first transcription downloads model weights (needs network), unless using
  a remote Riva endpoint.
- **ASR runs on fixed offline windows, not true streaming.** Audio is buffered
  into `audio_chunk_seconds` windows and each window is transcribed
  independently, so a word landing on a window boundary can be split across two
  transcript lines. The Nemotron/Riva backend's cache-aware *streaming* mode is
  not yet wired up (the adapter uses `offline_recognize`); switching it to
  `streaming_recognize` is the path to gapless, lower-latency transcription.
- **Speech timestamps are estimates.** The audio timeline is anchored to the
  wall-clock arrival of the first PCM bytes (correcting for capture-startup
  latency), then offsets accrue from sample counts; large silence gaps inserted
  by the source can introduce drift.
