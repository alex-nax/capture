---
name: capture-continue
description: Continue development on the capture-mcp project (this repo) across sessions and machines. Use whenever the user says "capture continue", "harness continue", "continue capture-mcp", "pick up where we left off", "next feature", or wants to make incremental progress here. Orients from claude-progress.md / features.json / docs/specs, bootstraps the env, verifies, implements ONE feature WITH its spec, tests, and commits. Works on macOS now and the planned Windows/NVIDIA box.
---

# capture-continue

Incremental, single-feature development loop for **capture-mcp** (MCP server that captures a
process's window screenshots, stdout/stderr, and per-app audio → ASR). This skill is
repo-local so it travels to other machines with the code.

## Steps

1. **Confirm location:** `pwd` → repo root (has `AGENTS.md`, `features.json`, `docs/specs/`).
2. **Orient (read, in order):**
   - `claude-progress.md` (top entry = most recent session, known issues, next task)
   - `features.json` (pick the next `"passes": false` honoring `dependencies`, priority high→low)
   - `docs/specs/<scope>.md` for the scope you'll touch (this is the CONTRACT — load it first)
   - `docs/architecture.md` (hard constraints)
3. **Bootstrap the env:**
   - macOS: `./init.sh` (creates the uv-managed **arm64** venv, installs `.[mlx]`, builds the Swift helper, runs smoke).
   - Windows (follow-up): there is no `init.ps1` yet — see `docs/specs/platform-abstraction.md` (feature #22). On Windows, expect the macOS-only paths (screencapture/Quartz/ScreenCaptureKit/sips) to be unavailable until the platform backends land.
4. **Verify baseline BEFORE new work:** `python tests/smoke.py` → expect `20/20 passed`. Fix any regression from a previous session first.
5. **Pick ONE feature** marked `"passes": false`. One feature per session.
6. **Implement it**, honoring the hard constraints in `docs/architecture.md`.
7. **📋 Update the matching `docs/specs/<scope>.md` in the SAME change** (MANDATORY — see AGENTS.md).
   Keep *Public contract / Behavior / Invariants / Failure modes / Open items* in sync. New
   scope ⇒ new spec + link it in `docs/specs/README.md`. A change where code moved but the spec
   didn't is incomplete.
8. **Test end-to-end** (not just imports). See Testing below.
9. **Commit** (branch first if on the default branch). End commit messages with the project's
   `Co-Authored-By` trailer. Commit code + spec + progress together.
10. **Update `claude-progress.md`** (new entry at TOP) and flip `features.json` `passes` to `true`
    only after verified end-to-end testing. Never delete/reword existing feature descriptions.

## Project facts (load these — they bite otherwise)
- **arm64 venv required** on this Mac: the system `python3` is x86_64 miniconda under Rosetta;
  mlx-whisper/modern wheels need the uv-managed arm64 venv that `init.sh` creates.
- **`server.py` stdout is the MCP transport** — never print to it; logs go to `stderr`.
- **Async tool handlers** offload blocking work via `anyio.to_thread.run_sync` (don't block the loop).
- **SCStreamError -3805** is a *transient* connection interruption (NOT a permission denial,
  which is -3801). The helper auto-reconnects through it. Stable signing
  (`scripts/setup_codesign.sh`) is only for making the Screen Recording grant persist across rebuilds.
- **ASR model gotcha:** `mlx-community/whisper-base` does NOT exist (404). Use `whisper-tiny`
  or the default `whisper-large-v3-turbo` via `CAPTURE_WHISPER_MODEL`.

## Cross-platform / next work
- Follow-up runs on a **Windows PC with an NVIDIA card**. Priorities there:
  - Build the **platform abstraction** (feature #20/#21) so screenshots/windows/audio have
    Windows backends (WASAPI loopback, `EnumWindows`, a Windows screenshot path) — design in
    `docs/specs/platform-abstraction.md`.
  - **Benchmark local Whisper vs NVIDIA Nemotron-3.5 ASR** (feature #23) using the existing
    Riva/Nemotron adapter (`src/capture_mcp/asr/nemotron.py`, `CAPTURE_RIVA_*` env).

## Testing
- Hermetic: `python tests/smoke.py` (no permissions/GPU).
- Real ASR: transcribe a `say`→16k s16le clip via `MlxWhisper(model="mlx-community/whisper-tiny")`.
- Helper: `bash scripts/build_helper.sh` then `./helper/audiocap --system` → expect `READY` + PCM.
- End-to-end: attach to a real app by `pid`; confirm `screenshots/` fills and (with permission)
  `transcript.jsonl` fills.
