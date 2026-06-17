---
name: capture
description: Capture any process's window as timestamped screenshots, its stdout/stderr, and its per-app audio transcribed to text — from ANY project, via capture-mcp. USE THIS SKILL whenever the user wants to record/capture/screen-record an app, window, or browser; transcribe an app's or browser's audio; "capture the browser video"; launch-and-capture a process or app; install or set up capture-mcp or the Capture macOS app; add capture to a project's .mcp.json; switch the capture microphone; import an existing audio/video file as a transcribed session; build a visual index/summary of a recording; or FIX a bad transcript (garbled output, repeated "Thank you", or wrong language) — even if they don't say "capture-mcp" by name. It installs capture-mcp (or points to the signed macOS app), creates or merges .mcp.json, then runs quick capture + transcription-repair actions.
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

capture-mcp ships two ways. They cooperate: the MCP server is **daemon-first**, so when the macOS
app is running, the MCP tools talk to the app's daemon — one Screen Recording grant, one session
registry, and captures show up in BOTH the app's window and the agent. Pick based on the user's setup:

**A. The macOS app (recommended for any Mac user — it owns the daemon + permission + GUI).**
Capture.app is a signed, notarized app distributed via **GitHub Releases**. If `/Applications/Capture.app`
is absent and the user is on a Mac, point them to the latest DMG:
- https://github.com/alex-nax/capture/releases/latest — download `Capture-<ver>.dmg`, open it, drag
  **Capture.app** to Applications, launch it. Its menu-bar agent runs the bundled daemon and opens the
  window; first launch prompts for **Screen Recording** (grant it once — the signed app makes it stick).
  The app **auto-updates** (it checks Releases and installs newer versions after you confirm), so it
  stays current on its own.

The app gives you the daemon + grant + GUI. To then drive capture from an MCP client (Claude Code etc.),
you still register the `capture` server (Step 2) — its `capture-mcp` command comes from **B**, and being
daemon-first it attaches to the app's daemon.

**B. From source (the agent/dev path, and the only path on Windows/Linux).** Clones the repo, makes a
venv, installs the package + an ASR backend, and on macOS **builds + stably code-signs** the audio helper
so the Screen Recording grant persists. This is what provides the `capture-mcp` command for `.mcp.json`,
and it works standalone (headless, no app) too.
- **macOS / Linux:** `bash scripts/install.sh`
- **Windows:** `powershell -ExecutionPolicy Bypass -File scripts/install.ps1`
  (screenshots/windows/logs need no helper; per-app audio loopback isn't wired yet — audio falls back to mic)

The installer prints `CAPTURE_MCP_BIN=<path>` (the `capture-mcp` entry point) and `CAPTURE_MCP_PY=<path>`
(the venv python) — capture both; Step 2 needs the bin. Re-running fast-pulls and re-prints them.

Requires `git` + Python 3.10+ (Windows: 3.12 via `winget install Python.Python.3.12`). macOS/Linux use
`uv` if present; macOS per-app audio needs `swiftc` (Xcode CLT) — the script notes if it's missing. (The app
in **A** already bundles a prebuilt signed helper, so app users never need Xcode.)

## Step 2 — Register capture in the project's .mcp.json
From the project directory the user wants capture available in:
```bash
python scripts/configure_mcp.py --bin "<CAPTURE_MCP_BIN>" [--model mlx-community/whisper-large-v3-turbo]
```
This **creates `.mcp.json` or merges into an existing one** (preserving other MCP servers),
adding a `capture` server. Tell the user to reload MCP servers in their client (in Claude Code:
restart or re-approve the project's MCP servers) so the capture tools appear: `capture_start` /
`capture_stop` / `capture_status` plus `list_windows`, `list_audio_devices`, `capture_set_mic`
(live mic switch), `capture_prune`, `capture_retranscribe`, `transcription_settings` (language +
chunk length — fixes hallucinated/wrong-language transcripts), `capture_import` (audio/video → a
session), and `capture_index` (multimodal index of the screenshots). See `references/quick-actions.md` §7.

## Step 3 — Verify prerequisites, then offer quick actions
Confirm `.mcp.json` contains the `capture` server and `CAPTURE_MCP_BIN` exists. The **Screen Recording**
permission (macOS, needed for screenshots + per-app audio) depends on which install owns the daemon:
- **App users (A):** Capture.app already prompted for the grant on first launch — nothing to do. The
  daemon-first MCP shares it.
- **From-source (B), no app:** the grant attaches to whatever launches the helper. `install.sh` stably
  signs it, so it's a **one-time** grant — trigger the prompt with `"<CAPTURE_HOME>/helper/audiocap"
  --system`, enable `audiocap` **and the launching terminal/app** under System Settings > Privacy &
  Security > Screen Recording, then reopen. (Ad-hoc signed? `bash <CAPTURE_HOME>/scripts/setup_codesign.sh`
  makes it stick.)

Capture still runs (screenshots + logs) without the grant, and audio auto-reconnects through transient
interruptions. On **Windows** there's no permission prompt; per-app audio loopback isn't wired yet, so
audio uses the microphone there.

Once the `capture` MCP tools are available, perform what the user asked using
`references/quick-actions.md` (which has the full tool reference in §7). Common jobs:
- **Capture a browser video** — navigate the browser to a URL, then capture its window + audio.
- **Launch & capture a process/app** — start a command (captures its stdout/stderr too) or attach
  to a running app by pid/name.
- **Fix a wrong/hallucinated transcript** (repeated "Thank you.", wrong language) — §8:
  `transcription_settings(language=…, chunk_seconds=30)` then `capture_retranscribe(session_id, language=…)`.
- **Import an existing audio/video file** as a transcribed session — `capture_import(path)`.
- **Switch the microphone mid-capture** — `capture_set_mic(session_id, device)`.
- **Index a recording** (visual summary via a remote vision LLM) — `capture_index(session_id)`.
- **Change the default ASR model** — `python scripts/set_model.py --model <repo> --prefetch --python
  "<CAPTURE_MCP_PY>"`, or `transcription_settings`/per-capture `asr_backend`.

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
