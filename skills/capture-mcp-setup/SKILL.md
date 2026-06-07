---
name: capture-mcp-setup
description: Install, wire up, and operate capture-mcp — capture any process's window as timestamped screenshots, its stdout/stderr, and its per-app audio transcribed to text — from ANY project. USE THIS SKILL whenever the user wants to record/capture/screen-record an app, window, or browser; transcribe an app's or browser's audio; "capture the browser video"; launch-and-capture a process or app; set up capture-mcp; add capture to a project's .mcp.json; or change the capture ASR model or config — even if they don't say "capture-mcp" by name. It installs capture-mcp and its dependencies if missing, creates or merges .mcp.json, then runs quick capture actions.
---

# capture-mcp-setup

One-load setup + operation skill for **capture-mcp**: an MCP server that captures a target
process's **window** (timestamped screenshots, configurable format/resolution), **stdout/stderr**,
and **per-app audio → text** via a pluggable ASR backend (local Whisper by default). This skill
gets it installed, registers it in the project's `.mcp.json`, and drives common capture tasks.

Bundled scripts live next to this file in `scripts/`; recipes are in `references/quick-actions.md`.

> Platform: fully supported on **macOS** today (screenshots + per-app audio). Linux/Windows
> support is in progress (see the project's `docs/specs/platform-abstraction.md`). On non-macOS,
> the Python/MCP install still works; per-app audio/screenshots may be limited.

## Step 1 — Install capture-mcp if not present
1. Decide the install location (default `~/.capture-mcp`, override with `CAPTURE_HOME`).
2. Run the installer (clones the repo, makes a venv, installs the package + an ASR backend, and
   on macOS builds the ScreenCaptureKit audio helper):
   ```bash
   bash scripts/install.sh
   ```
3. The installer prints `CAPTURE_MCP_BIN=<path>` (the `capture-mcp` entry point) and
   `CAPTURE_MCP_PY=<path>` (the venv python). Capture both — Step 2 needs the bin path.
   If it's already installed, the script fast-pulls and re-prints the paths.

Requires `git` and Python 3.10+; uses `uv` if available (recommended). On macOS, `swiftc`
(Xcode Command Line Tools) is needed for per-app audio — the script notes if it's missing.

## Step 2 — Register capture in the project's .mcp.json
From the project directory the user wants capture available in:
```bash
python scripts/configure_mcp.py --bin "<CAPTURE_MCP_BIN>" [--model mlx-community/whisper-large-v3-turbo]
```
This **creates `.mcp.json` or merges into an existing one** (preserving other MCP servers),
adding a `capture` server. Tell the user to reload MCP servers in their client (in Claude Code:
restart or re-approve the project's MCP servers) so the `capture_start` / `capture_stop` /
`capture_status` tools appear.

## Step 3 — Verify prerequisites, then offer quick actions
Confirm: `CAPTURE_MCP_BIN` exists, `.mcp.json` contains the `capture` server, and (macOS) the
helper binary built. On macOS, per-app audio needs **Screen Recording** permission for the app
that launches the server (and `bash <CAPTURE_HOME>/scripts/setup_codesign.sh` makes that grant
persist) — but capture still runs (screenshots + logs) without it, and audio auto-reconnects
through transient interruptions.

Once the `capture` MCP tools are available, perform what the user asked using
`references/quick-actions.md`. Quick actions include:
- **Capture a browser video** — navigate the browser to a URL, then capture its window + audio.
- **Launch & capture a process/app** — start a command (captures its stdout/stderr too) or attach
  to a running app by pid/name.
- **Change the default ASR model (and download it)** —
  `python scripts/set_model.py --model <repo> --prefetch --python "<CAPTURE_MCP_PY>"`.
- **Change other per-project config** — edit the `capture` entry's `env` / pass per-capture params
  (interval, format/resolution, audio_source, asr_backend); see the recipes file.

## Notes that save time
- ASR model names: `mlx-community/whisper-tiny` (fast) or the default
  `mlx-community/whisper-large-v3-turbo`. **`mlx-community/whisper-base` does not exist (404).**
- The first transcription downloads model weights (needs network) unless prefetched.
- Keep the server's stdout clean — it's the MCP transport; this is handled by capture-mcp itself.
