# Agent Instructions — capture-mcp

## Quick Start
1. Run `pwd` to confirm you are in `/Users/alex/capture`.
2. Read `claude-progress.md` for recent session history.
3. Read `features.json` to find the next task (`"passes": false`, respecting `dependencies`).
4. Run `./init.sh` to build the cargo workspace and run the tests (`cargo build --workspace` + `cargo test --workspace`).
5. Verify basics work (`cargo test --workspace` → all green) before starting new work.
6. Pick ONE feature marked `"passes": false` and implement it.
7. Test end-to-end (not just imports) — see Testing below.
8. Commit with a descriptive message.
9. Append a session entry to the TOP of `claude-progress.md`.
10. In `features.json`, only flip `passes` to `true` after verified end-to-end testing.

## Key Rules
- Work on ONE feature per session; leave the tree working and committable.
- Never delete or reword existing feature descriptions in `features.json`.
- Fix regressions from a previous session BEFORE starting new work.
- Keep `stdout` clean in the `capture-mcp` crate — it is the MCP transport; all logs go to `stderr`.
- The app is a single Rust cargo workspace under `crates/` (package names keep the `capture-`
  prefix): `core`, `platform`, `asr`, `asr-whisper`, `index`, `engine`, `daemon`, `mcp`, `gui`.

## 📋 SPECS ARE MANDATORY (documentation is a first-class step)
Every scope has a spec in **`docs/specs/`** — these are the source of intent for the
harness. **Whenever you implement or change behavior, update the matching spec in the
SAME change** (or add a new spec for a new scope). Treat "code changed, spec didn't" as
an incomplete change.

- Before coding: read the relevant `docs/specs/<scope>.md` to load the contract.
- While coding: keep the spec's *Public contract*, *Behavior*, *Invariants*, and
  *Failure modes* in sync with what you actually build.
- After coding: verify the spec matches the code (the code is the source of truth; the
  spec must not lie), then commit code + spec + `claude-progress.md` together.
- New scope/module → add `docs/specs/<scope>.md` (use the section template that the
  existing specs follow) and link it from `docs/specs/README.md`.

This is what keeps the harness stable across sessions: a new agent reads the spec to know
intent, then the code to know reality, and the two must agree.

## Environment gotchas (this machine)
- The whole app is a Rust cargo workspace — `./init.sh` just needs `cargo` (no Python venv,
  no Swift helper). The dev/eval utilities under `tools/` and the eval skills are pure-stdlib
  `python3` /v1 clients (any `python3`, no venv) that proxy the running daemon.
- Per-app audio uses the daemon's native ScreenCaptureKit path (the `screencapturekit` crate in
  `capture-platform`) and needs **Screen Recording** permission. A rebuilt ad-hoc binary loses the
  TCC grant → capture fails with `-3805`. Sign the daemon with a stable identity for a persistent grant.

## Testing
- **Workspace suite (no permissions/GPU):** `cargo test --workspace` — the hermetic unit/
  integration tests across all crates. Run a single crate's tests with `cargo test -p <crate>`
  (e.g. `cargo test -p capture-index`).
- **Real ASR:** `say -o /tmp/s.aiff "hello world"; ffmpeg -i /tmp/s.aiff -ac 1 -ar 16000 -f s16le /tmp/s.s16le`
  then run a chunk through the whisper.cpp engine (`capture-asr-whisper`) with a small GGML model.
- **End-to-end window+audio:** attach to a real app by `pid` and confirm screenshots land in
  `<out>/capture-*/screenshots/` and (if permission granted) `transcript.jsonl` fills.

## Bundled skills (travel with this repo)
This repo ships harness skills in `.claude/skills/` so they're available on any machine that
checks out the code:
- **capture-continue** — the per-session development loop (orient → bootstrap → verify →
  implement ONE feature WITH its spec → test → commit). Use this to "pick up where we left off".
- **capture-audit** — health/consistency audit (spec↔code drift, features/progress accuracy, smoke).
- **capture-spec** — create/update a `docs/specs/<scope>.md` (enforces the mandatory-spec policy).

## Architecture
See `docs/architecture.md` for module boundaries and dependency rules, and `docs/specs/` for
per-scope specs. Cross-platform/Windows plan: `docs/specs/platform-abstraction.md` (PLANNED).
