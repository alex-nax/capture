# Spec: Session registry

_Status: current as of 2026-06-10. Source of truth = the code; update this spec in the same change as the code._

## Purpose

`SessionRegistry` tracks capture sessions for any frontend. It was extracted from
`server.py` in the M0a split (feature #25) so the MCP server today — and the daemon /
CLI / GUI planned in [product-architecture.md](product-architecture.md) — share one
implementation. It does two jobs: thread-safe bounded tracking of **live**
`CaptureSession` objects owned by this process, and rebuilding **history** records of
finished sessions from disk at startup, so restarting a server no longer loses
session history.

## Files

- `src/capture_mcp/core/registry.py` — the entire scope (`SessionRegistry`,
  `MAX_SESSIONS`, `default_index_path`).
- `src/capture_mcp/core/__init__.py` — re-exports `SessionRegistry` / `MAX_SESSIONS` /
  `CaptureSession`.

## Public contract

- `default_index_path() -> Path` — `$CAPTURE_SESSION_INDEX` if set, else
  `~/.capture/sessions.jsonl`.
- `SessionRegistry(index_path=None, max_sessions=MAX_SESSIONS)` — resolves the index
  path (arg beats env beats default) and loads history immediately (constructor does
  file I/O; tolerant of a missing/corrupt index).
- `add(session)` — register a live session; appends one JSON line
  `{"id", "dir", "created_at"}` to the index (best-effort).
- `get(session_id) -> CaptureSession | None` — live sessions only.
- `running() -> list[CaptureSession]` — live sessions with `state == "running"`
  (a `"starting"` session is not yet stoppable, so it is excluded on purpose).
- `summary(session_id) -> dict | None` — live `session.summary()`, else a **copy** of
  the history record, else `None`.
- `summaries() -> list[dict]` — all known sessions (live overlays history on id
  collision), oldest first (ids are timestamp-prefixed, so lexical == chronological).
- `history_record(session_id) -> dict | None` — recovered records only (copies).
- `MAX_SESSIONS = 100` — default bound (moved verbatim from `server.py`).

## Behavior

- **Index** is an append-only JSONL log of every session ever started through `add()`.
  Re-reading it, later lines win on duplicate ids; blank/corrupt lines are skipped
  (torn writes in an append-only log are expected, not an error).
- **History load** (constructor): newest `max_sessions` index entries are recovered
  via `_recover(id, dir)`, which reads `<dir>/session.json` and takes its `summary`:
  - recorded state in `("starting", "running", "stopping")` → the recording process
    died mid-capture: state is rewritten to `"interrupted"` and a note appended;
  - `session.json` missing/unreadable (dir deleted, crash before first write) →
    minimal record with state `"unknown"`;
  - otherwise the record is used as-is (`stopped` / `error`).
- **Pruning** (`_prune_locked`, called from `add()`): when live + history exceeds
  `max_sessions`, evict oldest entries whose state is **not** live
  (`starting`/`running`/`stopping` are never evicted) — same bounded-finished-history
  tradeoff as the pre-extraction server registry. Evicted sessions remain in the
  index file and reappear via history after a restart.

## Invariants & constraints

- **All `_live`/`_history` access is under the registry's own lock**; callers never
  take it. `summaries()`/`summary()` call `session.summary()` under the lock —
  `summary()` must stay cheap and lock-free (it is; it snapshots attributes).
- **History records are read-only copies**: accessors return `dict(rec)` so callers
  cannot mutate registry state.
- **Index writes never break a capture**: `_append_index` catches everything and
  logs to stderr (mirrors `session.json` write behavior).
- **One registry per index file is assumed** — there is no cross-process file lock;
  two live servers sharing an index will interleave appends (safe: append-only,
  later-line-wins) but will NOT see each other's live sessions. Cross-process
  session visibility is the daemon's job (M2, product-architecture.md).

## Failure modes & handling

- Missing index file → empty history (first run; not an error).
- Unreadable index / unwritable index dir → logged, registry works memory-only.
- Corrupt index lines → skipped silently (by design).
- `session.json` unreadable for an indexed id → `"unknown"` record (kept, so the user
  can still see the id and dir).
- Same id indexed twice → later line wins.

## Outputs / artifacts

- The index file (`~/.capture/sessions.jsonl` or `$CAPTURE_SESSION_INDEX`): one JSON
  object per line, keys `id`, `dir`, `created_at` (ISO). Append-only; never rewritten
  or compacted (growth is unbounded — see Known limitations).

## Configuration

- `CAPTURE_SESSION_INDEX` — overrides the index path. Read when the registry is
  constructed (the MCP server constructs it at import time — set the env var before
  importing `capture_mcp.server`; `tests/smoke.py` does).
- `max_sessions` constructor arg (default `MAX_SESSIONS = 100`).

## Known limitations / open items

- The index file grows without bound (one small line per session; compaction is a
  non-urgent open item — fold into the M2 daemon work).
- History is loaded once at construction; sessions finished by *another* process
  while this one runs are not visible until restart (daemon fixes this properly).
- `created_at` in the index is the registration time, not the session's `t0`.
- No machine-wide capture root exists yet (`output_dir` is per-call), so history is
  only as complete as the index — session dirs moved/deleted degrade to `"unknown"`.
  The default-capture-root design is tracked in product-architecture.md.

## Tests

`tests/smoke.py::test_registry_history` (hermetic): a fresh registry recovers the
suite's stopped sessions from the index; a crafted index entry whose `session.json`
says `"running"` recovers as `"interrupted"`; a missing-dir entry recovers as
`"unknown"`; corrupt index lines are tolerated; `summaries()` is oldest-first.
`tests/smoke.py::test_status_during_start` covers the registry's role in pre-start
visibility (`"starting"` listed in `capture_status`). Cross-process restart was
verified manually (two sequential processes sharing one index; see Session 10 in
`claude-progress.md`).
