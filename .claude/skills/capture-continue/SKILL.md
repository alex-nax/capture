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
   - `./init.sh` — builds the cargo workspace and runs the tests (`cargo build --workspace` + `cargo test --workspace`). Just needs `cargo`; no Python venv, no Swift helper (the daemon does ScreenCaptureKit + AVFoundation natively).
   - Windows (deferred, #66): packaging still references Python and is not wired for v3 yet — see `docs/specs/platform-abstraction.md`.
4. **Verify baseline BEFORE new work:** `cargo test --workspace` → expect all green. Fix any regression from a previous session first.
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
- **Single Rust cargo workspace** under `crates/` (package names keep the `capture-` prefix):
  `core`, `platform`, `asr`, `asr-whisper`, `index`, `engine`, `daemon`, `mcp`, `gui`. `./init.sh`
  just needs `cargo`. The dev/eval utilities under `tools/` are pure-stdlib `python3` /v1 clients.
- **The `capture-mcp` stdout is the MCP transport** — never print to it; logs go to `stderr`.
- **SCStreamError -3805** is a *transient* connection interruption (NOT a permission denial,
  which is -3801). The daemon auto-reconnects through it. A stable signing identity for the daemon
  is only for making the Screen Recording grant persist across rebuilds.
- **ASR engine:** the built-in whisper.cpp engine (`capture-asr-whisper`) uses GGML weights
  (e.g. the default `ggml-large-v3-turbo`, or `ggml-tiny` for quick tests).

## Cross-platform / next work
- Follow-up runs on a **Windows PC with an NVIDIA card** (Windows packaging is deferred, #66).
  Priorities there:
  - Build out the **platform abstraction** so screenshots/windows/audio have Windows backends
    (WASAPI loopback, `EnumWindows`, a Windows screenshot path) — design in
    `docs/specs/platform-abstraction.md`.
  - **Benchmark the local whisper.cpp engine vs a remote NVIDIA ASR backend** using the
    openai-compatible/Riva remote backend (`CAPTURE_RIVA_*` env). Remote ASR backends are deferred (#80).

## Testing
- Workspace suite: `cargo test --workspace` (no permissions/GPU). Single crate: `cargo test -p <crate>`.
- Real ASR: transcribe a `say`→16k s16le clip through the whisper.cpp engine (`capture-asr-whisper`)
  with a small GGML model (e.g. `ggml-tiny`).
- End-to-end: attach to a real app by `pid`; confirm `screenshots/` fills and (with permission)
  `transcript.jsonl` fills.
