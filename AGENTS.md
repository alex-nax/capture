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

## Architecture
See `docs/architecture.md` for module boundaries and dependency rules.
