---
name: capture
description: Capture any process's window as timestamped screenshots, its stdout/stderr, and its per-app audio transcribed to text — from ANY project, via capture-mcp. USE THIS SKILL whenever the user wants to record/capture/screen-record an app, window, or browser; transcribe an app's or browser's audio; "capture the browser video"; launch-and-capture a process or app; set up capture-mcp; add capture to a project's .mcp.json; or change the capture ASR model or config — even if they don't say "capture-mcp" by name. It installs capture-mcp and its dependencies if missing, creates or merges .mcp.json, then runs quick capture actions.
---

# capture

One-load setup + operation skill for **capture-mcp**: an MCP server that captures a target
process's **window** (timestamped screenshots, configurable format/resolution), **stdout/stderr**,
and **per-app audio → text** via a pluggable ASR backend (local Whisper by default). This skill
gets it installed, registers it in the project's `.mcp.json`, and drives common capture tasks.

Bundled scripts live next to this file in `scripts/`; recipes are in `references/quick-actions.md`.

> Platform: **macOS** and **Windows** are supported (see `docs/specs/platform-abstraction.md`).
> - **macOS** — screenshots, window discovery, logs, and **per-app audio → text** (ScreenCaptureKit).
> - **Windows** — screenshots (GDI+) + window discovery (EnumWindows) + logs work with **no extra deps
>   and no permission prompt**; per-app audio (WASAPI process loopback) is not wired yet, so audio
>   falls back to the microphone there. Use `install.ps1` to set it up.

## Step 1 — Install capture-mcp if not present
1. Decide the install location (default `~/.capture-mcp` / `%USERPROFILE%\.capture-mcp`, override
   with `CAPTURE_HOME`).
2. Run the installer for the platform (clones the repo, makes a venv, installs the package + an ASR
   backend):
   - **macOS / Linux** — also builds **and stably code-signs** the ScreenCaptureKit audio helper so
     the Screen Recording grant is approved **once** and persists:
     ```bash
     bash scripts/install.sh
     ```
   - **Windows** — screenshots/window-discovery/logs need no helper; per-app audio loopback isn't
     wired yet (audio falls back to mic):
     ```powershell
     powershell -ExecutionPolicy Bypass -File scripts/install.ps1
     ```
3. The installer prints `CAPTURE_MCP_BIN=<path>` (the `capture-mcp` entry point) and
   `CAPTURE_MCP_PY=<path>` (the venv python). Capture both — Step 2 needs the bin path.
   If it's already installed, the script fast-pulls and re-prints the paths.

Requires `git` and Python 3.10+. macOS/Linux: uses `uv` if available (recommended); on macOS
`swiftc` (Xcode Command Line Tools) is needed for per-app audio — the script notes if it's missing.
Windows: needs Python 3.12 (`winget install Python.Python.3.12`); `install.ps1` finds the `py`
launcher or a python.org install (ignores the Microsoft Store stub).

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
helper binary built. On **macOS**, per-app audio needs **Screen Recording** permission for the app
that launches the server. `install.sh` already stably signs the helper, so this is a **one-time**
grant that persists across rebuilds — trigger the prompt with `"<CAPTURE_HOME>/helper/audiocap"
--system`, enable `audiocap` **and the launching terminal/app** under System Settings > Privacy &
Security > Screen Recording, then reopen it. (If the helper was only ad-hoc signed, re-run
`bash <CAPTURE_HOME>/scripts/setup_codesign.sh` to make the grant stick.) Capture still runs
(screenshots + logs) without the grant, and audio auto-reconnects through transient interruptions.
On **Windows** no permission prompt is involved; per-app audio loopback isn't wired yet, so audio
uses the microphone there.

Once the `capture` MCP tools are available, perform what the user asked using
`references/quick-actions.md`. Quick actions include:
- **Capture a browser video** — navigate the browser to a URL, then capture its window + audio.
- **Launch & capture a process/app** — start a command (captures its stdout/stderr too) or attach
  to a running app by pid/name.
- **Change the default ASR model (and download it)** —
  `python scripts/set_model.py --model <repo> --prefetch --python "<CAPTURE_MCP_PY>"`.
- **Change other per-project config** — edit the `capture` entry's `env` / pass per-capture params
  (interval, format/resolution, audio_source, asr_backend); see the recipes file.

## When something goes wrong → file a bug (so it gets tracked)
If a capture fails or behaves wrong and you can't quickly fix it, offer to report it upstream so
the maintainers can track it:
1. **Preview** the issue (collects safe diagnostics — version, OS/arch, the session's
   `audio_status`/errors/notes; secrets/env values are omitted):
   ```bash
   python scripts/report_issue.py --summary "<what went wrong>" --session-dir "<output_dir>"
   ```
2. **Show the user** the previewed title/body and get explicit OK — posting publishes to a public
   repo (`github.com/alex-nax/capture`). Let them redact anything they consider sensitive.
3. **File it** — re-run with `--create` (uses `gh` if installed+authenticated), or have the user
   open the prefilled URL the script prints:
   ```bash
   python scripts/report_issue.py --summary "<...>" --session-dir "<dir>" --create
   ```
Never post without the user's confirmation.

## Notes that save time
- ASR model names: `mlx-community/whisper-tiny` (fast) or the default
  `mlx-community/whisper-large-v3-turbo`. **`mlx-community/whisper-base` does not exist (404).**
- The first transcription downloads model weights (needs network) unless prefetched.
- Keep the server's stdout clean — it's the MCP transport; this is handled by capture-mcp itself.
