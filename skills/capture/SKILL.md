---
name: capture
description: Capture any process's window as timestamped screenshots, its stdout/stderr, and its per-app audio transcribed to text — from ANY project, via capture-mcp. USE THIS SKILL whenever the user wants to record/capture/screen-record an app, window, or browser; transcribe an app's or browser's audio; "capture the browser video"; launch-and-capture a process or app; set up capture-mcp or the Capture macOS app; add capture to a project's .mcp.json; switch the capture microphone; import an existing audio/video file as a transcribed session; build a visual index/summary of a recording; or FIX a bad transcript (garbled output, repeated "Thank you", or wrong language) — even if they don't say "capture-mcp" by name. It registers capture-mcp (the signed macOS app ships it) in the project's .mcp.json, then runs quick capture + transcription-repair actions.
---

# capture

One-load setup + operation skill for **capture-mcp**: an MCP server that captures a target process's
**window** (timestamped screenshots, configurable format/resolution), **stdout/stderr**, and **per-app
audio → text** via a pluggable ASR backend (local Whisper by default). This skill registers it in the
project's `.mcp.json`, then drives common capture tasks.

Bundled scripts live next to this file in `scripts/`; recipes are in `references/quick-actions.md`.

> Platform: **macOS** today — screenshots, window discovery, logs, and per-app audio → text
> (ScreenCaptureKit). Windows is deferred (#66). See `docs/specs/platform-abstraction.md`.

## Step 1 — Register capture in the project

`capture-mcp` is **daemon-first**: it talks to the running Capture daemon, so the MCP tools share the
app's Screen Recording grant and session registry — captures show up in BOTH the app window and the
agent. From the project directory you want capture available in, run:

```bash
python scripts/discover_mcp.py --configure
```

This locates the `capture-mcp` command (the signed Capture.app ships it), **verifies it answers a real
MCP `initialize` + `tools/list` handshake**, and creates or merges the project's `.mcp.json`
(preserving any other servers), adding the `capture` server.

- **Exit 0** — it printed `CAPTURE_MCP_BIN=…` and registered `capture`. Tell the user to reload MCP in
  their client (Claude Code: restart or re-approve the project's MCP servers), then go to **Step 3**.
- **Exit 3** — capture-mcp isn't available yet. Go to **Step 2** to install the app, then re-run this.

(Run `discover_mcp.py` without `--configure` to just print the verified path; register it yourself with
`configure_mcp.py --bin "<path>"`.) The tools this exposes: `capture_start` / `capture_stop` /
`capture_status`, `list_windows`, `list_audio_devices`, `capture_set_mic` (live mic switch),
`capture_prune`, `capture_retranscribe`, `transcription_settings` (language + chunk — fixes
hallucinated/wrong-language transcripts), `capture_import` (audio/video → a session), `capture_index`
(multimodal index of the screenshots), `index_models`. See `references/quick-actions.md` §7.

## Step 2 — Install Capture.app (only if Step 1 found nothing)

Capture.app is the signed, notarized macOS app: it runs the daemon, owns the Screen Recording grant,
provides the GUI, and **ships the `capture-mcp` the skill registers**.

- https://github.com/alex-nax/capture/releases/latest — download `Capture-<ver>.dmg`, open it, drag
  **Capture.app** to Applications, and launch it. Its menu-bar agent runs the daemon and opens the
  window; first launch prompts for **Screen Recording** (grant it once — the signed app makes it
  stick). The app **auto-updates** (it checks Releases and installs newer versions after you confirm).

Then **re-run Step 1** — `discover_mcp.py` finds the freshly-installed `capture-mcp` and registers it.
(The app's Settings → Skills can also copy the MCP command path to your clipboard.)

## Step 3 — Verify, then offer quick actions

Confirm `.mcp.json` contains the `capture` server. The **Screen Recording** permission (macOS, needed
for screenshots + per-app audio) is owned by **Capture.app** — it prompts on first launch, the signed
app makes the grant persist, and the daemon-first MCP shares it. So for app users there's nothing extra
to do; capture still runs (screenshots + logs) without the grant, and audio auto-reconnects through
transient interruptions.

Once the `capture` MCP tools are available, perform what the user asked using
`references/quick-actions.md` (which has the full tool reference in §7). Common jobs:
- **Capture a browser video** — navigate the browser to a URL, then capture its window + audio.
- **Launch & capture a process/app** — start a command (captures its stdout/stderr too) or attach to a
  running app by pid/name.
- **Fix a wrong/hallucinated transcript** (repeated "Thank you.", wrong language) — §8:
  `transcription_settings(language=…, chunk_seconds=30)` then `capture_retranscribe(session_id, language=…)`.
- **Import an existing audio/video file** as a transcribed session — `capture_import(path)`.
- **Switch the microphone mid-capture** — `capture_set_mic(session_id, device)`.
- **Index a recording** (visual summary via a remote vision LLM) — `capture_index(session_id)`.
- **Change the default ASR model** — `python scripts/set_model.py --model <repo> --prefetch` (it sets
  the running daemon's model over `/v1`; e.g. `ggml-large-v3-turbo`), or `transcription_settings` /
  per-capture `asr_backend`.

## When something goes wrong → file a bug (so it gets tracked)

If a capture fails or behaves wrong and you can't quickly fix it, offer to report it upstream so the
maintainers can track it:
1. **Preview** the issue (collects safe diagnostics — version, OS/arch, the session's
   `audio_status`/errors/notes; secrets/env values are omitted):
   ```bash
   python scripts/report_issue.py --summary "<what went wrong>" --session-dir "<output_dir>"
   ```
2. **Show the user** the previewed title/body and get explicit OK — posting publishes to a public repo
   (`github.com/alex-nax/capture`). Let them redact anything they consider sensitive.
3. **File it** — re-run with `--create` (uses `gh` if installed+authenticated), or have the user open
   the prefilled URL the script prints:
   ```bash
   python scripts/report_issue.py --summary "<...>" --session-dir "<dir>" --create
   ```
Never post without the user's confirmation.

## Notes that save time
- ASR model names are GGML (whisper.cpp): `ggml-tiny` (fast) or the default `ggml-large-v3-turbo`.
- The first transcription downloads model weights (needs network) unless prefetched.
- Keep the server's stdout clean — it's the MCP transport; this is handled by capture-mcp itself.
