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

The app is a single Rust cargo workspace under `crates/` (no Python venv, no Swift helper —
the daemon does ScreenCaptureKit + AVFoundation natively). You need [Rust](https://rustup.rs).

```bash
cd /Users/alex/capture
./init.sh                           # cargo build --workspace + cargo test --workspace
```

That builds every crate: `captured` (daemon), `capture-mcp` (MCP server), `capture-gui`
(GPUI app), plus the `capture-asr*` / `capture-index` / `capture-platform` libraries. The
whisper.cpp ASR engine is built in; model weights download on first use.

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

The ASR layer is pluggable. ASR engines are `dlopen`'d cdylibs behind an
engine-agnostic C ABI, managed by the `capture-asr` crate:

- **Local (default).** `capture-asr-whisper` — a whisper.cpp engine running
  entirely on this Mac (Metal-accelerated). Downloads GGML model weights on first
  use (e.g. `ggml-large-v3-turbo`).
- **Remote (`nemotron`/openai-compatible).** A remote endpoint hosting a model
  that can't run on this Mac directly (e.g. Nemotron-3.5 on an NVIDIA box).
  Configure with env vars: `CAPTURE_RIVA_SERVER`, `CAPTURE_RIVA_API_KEY`,
  `CAPTURE_RIVA_LANG`, `CAPTURE_RIVA_MODEL`.

To add a backend, implement the engine C ABI as a new cdylib crate and register it
in the `capture-asr` runtime manager.

## Register with an MCP client

Example `claude_desktop_config.json` / `.mcp.json` entry (point it at the built binary):

```json
{
  "mcpServers": {
    "capture": {
      "command": "/Users/alex/capture/target/debug/capture-mcp"
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

After `./init.sh` (or `cargo build --workspace`) the binaries live under `target/debug/`:
`capture-mcp` (MCP server), `captured` (daemon), `capture` (CLI). Run them with
`cargo run -p <crate>` or directly from `target/debug/`.

### CLI (simplest)

```bash
# start the local daemon (spawns it in the background; writes ~/.capture/daemon.json)
capture daemon start                 # or: cargo run -p capture-cli -- daemon start

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
across CLI calls; an MCP agent transparently uses the daemon when one is running.

### Daemon (run it in the foreground)

`capture daemon start` spawns it for you; to run it yourself and watch its logs:

```bash
cargo run -p capture-daemon          # or run target/debug/captured directly
# serves http://127.0.0.1:<port>; endpoint + bearer token in ~/.capture/daemon.json (0600)
```

### GUI (native menu-bar app, macOS)

```bash
# build once (needs Rust — https://rustup.rs ; gpui's first compile is heavy)
cargo build -p capture-gui          # add --release for an optimized build
# run it (a dev binary has no bundled daemon, so start one: `capture daemon start`)
cargo run -p capture-gui            # or ./target/debug/capture-gui
```

> The **packaged** `Capture.app` (below) bundles its own daemon binary and auto-spawns it; this
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
bundles the daemon binary, so there's nothing to set up and nothing to start by hand:

```bash
bash packaging/build_macos_dmg.sh        # -> dist/Capture-0.2.0.dmg  (needs Rust + Xcode CLT)
```

The build embeds the native `captured` daemon binary into the app
(`Contents/Resources/captured/`). Open the DMG and drag **Capture.app** to **Applications**.

> ✅ The **official release `.dmg`** (GitHub Releases) is **Developer-ID signed + notarized** —
> just open it and drag to Applications, no Gatekeeper bypass needed. Set
> `CAPTURE_SIGN_IDENTITY` + `CAPTURE_NOTARIZE_PROFILE` to produce a notarized build yourself;
> otherwise the local build is **ad-hoc** signed (dev only).

> ⚠️ A **self-built ad-hoc** dmg is NOT notarized — macOS **Gatekeeper blocks it on first launch**
> ("Apple could not verify 'Capture' is free of malware"). Bypassing means running an app Apple
> hasn't checked — only do it for a build you trust (one you built yourself).

**Bypass Gatekeeper (only for an ad-hoc / self-built dmg, first launch):**

- **Easiest:** **Control-click** (right-click) `Capture.app` → **Open** → **Open** in the dialog.
- **macOS 15 (Sequoia):** double-click → it's blocked → **System Settings ▸ Privacy & Security** →
  scroll to the "'Capture' was blocked" line → **Open Anyway** → confirm with Touch ID/password.
- **Terminal:** strip the download-quarantine flag, then open:
  ```bash
  xattr -dr com.apple.quarantine /Applications/Capture.app
  open /Applications/Capture.app
  ```

**What the app needs / does:**

- Launching **Capture.app** runs a small **menu-bar agent** (`CaptureBar`) — it lives in the menu
  bar (top-right, `● capture`), with **no Dock icon**. It is the persistent part: it spawns the
  bundled daemon, shows capture status, and opens the window. **Closing the window keeps the agent
  (and your captures) running** — re-open from the menu-bar **Open Window**. Use the menu-bar
  **Quit Capture** to fully exit (it stops the daemon when idle, so the app isn't "in use" if you
  want to delete/replace it).
- It is **self-contained** — the agent auto-spawns its **bundled** daemon binary (detached) if one
  isn't already running. No repo, nothing to set up, no `capture daemon start`. (If a daemon is
  already running — e.g. from the repo — it attaches to that one instead.)
- **On-device transcription works out of the box** — the daemon bundles the whisper.cpp ASR engine.
  Whisper **model weights** are *not* bundled (they're large); download them from the app's
  **Whisper models** panel (Download → pick a size; it shows live progress), then **Use** to make
  one active. The default model is `ggml-large-v3-turbo`; `ggml-tiny`/`base` are great for quick
  tests. (You can still point at a remote OpenAI-compatible ASR endpoint instead.)
- Per-app audio still needs **Screen Recording** granted to the app — the daemon does
  ScreenCaptureKit natively and is the TCC-responsible process. A stable signing identity keeps the
  grant persistent across rebuilds.

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

The `captured` daemon uses ScreenCaptureKit natively (via the `screencapturekit`
crate, in `capture-platform`) to capture audio from a single application (matched
by PID or bundle id), converts it to 16 kHz mono PCM, buffers it into windows,
runs ASR, and timestamps each segment. If app audio isn't available (or
`audio_source="mic"`), it falls back to capturing the default microphone via
AVFoundation. It can also capture whole-display ("system") audio.

### About SCStreamError -3805

`-3805` is `SCStreamError.failedApplicationConnectionInterrupted` — the stream's
connection to the capture server was interrupted (commonly right after
`startCapture`, or when focus/Spaces change during background capture). It is
**not** a permission denial (that is `-3801 userDeclined`). In practice it is a
*transient* interruption: `SCShareableContent` enumerates fine (so Screen
Recording is granted), and the very next connection attempt usually succeeds.

The daemon handles this automatically: on `-3805` it **rebuilds the stream and
reconnects** (with backoff), so background capture survives Space/window switches.
Genuine permission errors (`-3801`/`-3803`) are *not* retried — they're reported
instead.

**Make the Screen Recording grant persist (recommended):** an ad-hoc signature
changes every rebuild, so macOS re-prompts. Give the daemon binary a stable
signing identity (a Developer ID, or a self-signed cert reused across rebuilds) so
the grant keys to that identity and survives same-identity updates.

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
