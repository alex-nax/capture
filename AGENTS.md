# Agent Instructions — capture-mcp

## Quick Start
1. Run `pwd` to confirm you are in `/Users/alex/capture`.
2. Read `claude-progress.md` for recent session history.
3. Read `features.json` to find the next task (`"passes": false`, respecting `dependencies`).
4. Run `./init.sh` to set up the venv, install deps, build the Swift helper, and run the smoke test.
5. Verify basics work (`python tests/smoke.py` → `20/20 passed`) before starting new work.
6. Pick ONE feature marked `"passes": false` and implement it.
7. Test end-to-end (not just imports) — see Testing below.
8. Commit with a descriptive message.
9. Append a session entry to the TOP of `claude-progress.md`.
10. In `features.json`, only flip `passes` to `true` after verified end-to-end testing.

## Key Rules
- Work on ONE feature per session; leave the tree working and committable.
- Never delete or reword existing feature descriptions in `features.json`.
- Fix regressions from a previous session BEFORE starting new work.
- Keep `stdout` clean in `server.py` — it is the MCP transport; all logs go to `stderr`.
- Don't block the asyncio event loop in tool handlers — offload blocking work with
  `anyio.to_thread.run_sync` (see `docs/architecture.md`).

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
- The machine is **arm64**, but the system `python3` is **x86_64 miniconda under Rosetta**.
  Always use the project venv (`uv venv --python 3.12 --python-preference only-managed`),
  which is arm64 — required for `mlx-whisper`.
- Per-app audio uses a Swift ScreenCaptureKit helper that needs **Screen Recording**
  permission. A rebuilt ad-hoc binary loses the TCC grant → `startCapture` fails with
  `-3805`. Build with a stable `CODESIGN_IDENTITY` for a persistent grant.

## Testing
- **Hermetic suite (no permissions/GPU):** `python tests/smoke.py` — launch-mode logs +
  screenshots, async MCP tools, audio chunking/offsets (stub ASR), `parse_resolution`.
- **Real ASR:** `say -o /tmp/s.aiff "hello world"; ffmpeg -i /tmp/s.aiff -ac 1 -ar 16000 -f s16le /tmp/s.s16le`
  then run a chunk through `capture_mcp.asr.whisper_local.MlxWhisper(model="mlx-community/whisper-tiny")`.
- **Swift helper:** `bash scripts/build_helper.sh` then `./helper/audiocap --system --rate 16000`
  — expect a `READY ...` line on stderr and PCM bytes on stdout (needs Screen Recording).
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
