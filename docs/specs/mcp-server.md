# Spec: MCP Server

_Status: current as of 2026-06-07. Source of truth = the code; update this spec in the same change as the code._

## Purpose

The MCP server is the entrypoint and orchestration layer for capture-mcp. It exposes
on-demand process capture to an MCP client over stdio as three tools
(`capture_start`, `capture_stop`, `capture_status`). It validates arguments,
constructs and tracks `CaptureSession` objects in a bounded in-memory registry, and
offloads all blocking work to worker threads so the MCP event loop (and the stdio
transport) stays responsive. Per `docs/architecture.md`, this layer **only
orchestrates** — it must not contain capture logic itself (that lives in `session.py`
and the components).

## Files

- `src/capture_mcp/server.py` — the entire scope (the `FastMCP` instance, the three
  tool handlers, the session registry, `_prune_locked`, and `main`).

Imported but out of scope here:
- `src/capture_mcp/session.py` — `CaptureSession` (called via `.start()`, `.stop()`,
  `.summary()`, and the `.id` / `.state` attributes).

## Public contract

### Process / transport

- Entry point `main()` (`server.py:200`) calls `mcp.run()` on a `FastMCP` instance
  named `"capture-mcp"` (`server.py:33`). The module is runnable as `python -m
  capture_mcp.server` (via the `__main__` guard, `server.py:204`) or as the
  `capture-mcp` console script (per the module docstring).
- Transport is **stdio** (the FastMCP default). `mcp.run()` is invoked with no
  arguments, so the default transport is used.
- **stdout** is reserved exclusively for the MCP transport. **stderr** carries logs:
  `logging.basicConfig(level=logging.INFO, stream=sys.stderr, ...)` (`server.py:26-30`),
  with logger name `"capture_mcp"`. There are no `print()` calls in the server path.

### Tool: `capture_start` (`server.py:59-149`)

Async handler returning a `dict` (the session summary produced by
`session.start()`). Parameters (name : type = default):

- `output_dir: str` (required) — base directory for the session folder.
- `command: str | None = None` — shell command to launch and capture (the only mode
  that captures logs).
- `pid: int | None = None` — PID of a running process to attach to.
- `app_name: str | None = None` — case-insensitive substring of an app name to attach to.
- `bundle_id: str | None = None` — bundle id for per-app audio (e.g. `"com.apple.Safari"`).
- `screenshot_interval: float = 1.0` — seconds between screenshots.
- `screenshot_format: str = "png"` — image format (png, jpg/jpeg, tiff, gif, bmp per docstring).
- `screenshot_resolution: str | None = None` — bounding box `"WxH"`, optionally with a
  format suffix (e.g. `"1280x720/jpg"`); scale-to-fit, never upscale.
- `screenshot_jpeg_quality: int | None = None` — JPEG quality 0-100 (jpg only).
- `capture_screenshots: bool = True`.
- `capture_audio: bool = True`.
- `audio_source: str = "auto"` — `"auto"`, `"app"`, or `"mic"`.
- `audio_chunk_seconds: float = 8.0`.
- `asr_backend: str = "auto"` — `"auto"`, `"local"`/`"whisper"`, or `"nemotron"`/`"riva"`.
- `cwd: str | None = None` — working directory for a launched command.

**Target selection rule:** exactly one of `command`, `pid`, `app_name` must be
provided (see Behavior / Failure modes for the precise notion of "provided").

Return: the `dict` returned by `CaptureSession.start()` (a session summary that
includes `session_id`). The exact shape is defined by `session.py` and is not fixed
by this scope.

### Tool: `capture_stop` (`server.py:152-180`)

Async handler. Parameter:

- `session_id: str | None = None` — the session to stop. If omitted: stop the unique
  running session if there is exactly one.

Returns a `dict`:
- When `session_id is None` and **no** sessions are running:
  `{"stopped": [], "note": "no running captures"}` (`server.py:168`).
- Otherwise: the `dict` returned by the target session's `.stop()` (final summary).

### Tool: `capture_status` (`server.py:183-197`)

Async handler. Parameter:

- `session_id: str | None = None`.

Returns a `dict`:
- If `session_id` is given: `session.summary()` for that session.
- If omitted: `{"sessions": [s.summary() for s in _sessions.values()]}` — a list of
  summaries for **every** session this server has created (running and finished, up
  to the registry bound).

### Errors

Validation/lookup failures are raised as `ValueError`. FastMCP converts a raised
exception into an MCP tool error result; the messages below are the exact strings the
code raises (see Failure modes).

## Behavior

### `capture_start`

1. Defines a local `_present(v)` predicate (`server.py:113-120`): `None` is absent; a
   `str` is present only if it is non-blank after `.strip()`; any other non-`None`
   value (notably `pid=0`) is present.
2. Builds `provided` = the list of target names among `("command", command)`,
   `("pid", pid)`, `("app_name", app_name)` for which `_present` is true
   (`server.py:122`).
3. If `len(provided) == 0`: raise `ValueError("specify exactly one target: command,
   pid, or app_name")` (`server.py:123-124`).
4. If `len(provided) > 1`: raise `ValueError(f"specify exactly one target, but got:
   {...}")` listing the provided names (`server.py:125-126`).
5. Constructs a `CaptureSession` with all parameters forwarded verbatim
   (`server.py:128-144`). Construction is synchronous and happens on the event loop
   (it is expected to be cheap; the heavy work is in `.start()`).
6. Calls `summary = await anyio.to_thread.run_sync(session.start)` — the blocking
   start (subprocess launch, ASR model load, etc.) runs on a worker thread
   (`server.py:145`).
7. Under `_lock`: inserts `_sessions[session.id] = session`, then calls
   `_prune_locked()` (`server.py:146-148`).
8. Returns `summary` (`server.py:149`).

Note: if `session.start()` raises, the exception propagates and the session is
**never** added to `_sessions` (registry insertion happens only after a successful
start). Rollback of partially started components is `CaptureSession.start()`'s
responsibility (per `docs/architecture.md`).

### `capture_stop`

1. Under `_lock`, snapshots `running = [s for s in _sessions.values() if s.state ==
   "running"]` (`server.py:163-164`).
2. If `session_id is None`:
   - If `running` is empty: return `{"stopped": [], "note": "no running captures"}`.
   - If more than one is running: raise `ValueError("multiple captures running; pass
     session_id. Running: " + <ids>)`.
   - Otherwise: return `await anyio.to_thread.run_sync(running[0].stop)`.
3. If `session_id` is given: under `_lock`, look up `_sessions.get(session_id)`. If
   missing, raise `ValueError(f"unknown session_id {session_id!r}")`. Otherwise
   return `await anyio.to_thread.run_sync(session.stop)` (`server.py:176-180`).

### `capture_status`

1. Acquire `_lock` (`server.py:191`).
2. If `session_id` given: return `session.summary()`, or raise `ValueError(f"unknown
   session_id {session_id!r}")` if not found (`server.py:192-196`).
3. Otherwise return `{"sessions": [...summaries...]}` (`server.py:197`).

Note: `capture_status` calls `session.summary()` **while holding `_lock`**, unlike
`capture_stop` which releases the lock before the offloaded `.stop()` call.
`.summary()` is therefore assumed to be cheap and non-blocking. This is a potential
concern if `summary()` ever does I/O (see Known limitations).

### `_prune_locked` (`server.py:44-56`)

1. Caller must already hold `_lock` (only `capture_start` calls it).
2. If `len(_sessions) <= MAX_SESSIONS` (100), return immediately.
3. Compute `finished = sorted(sid for sid, s in _sessions.items() if s.state !=
   "running")` — finished session ids sorted lexically. Session ids are
   timestamp-prefixed, so lexical order equals chronological order.
4. Evict the oldest finished ids: `_sessions.pop(sid, None)` for the first
   `len(_sessions) - MAX_SESSIONS` of them.

## Invariants & constraints

These map directly onto the "Hard constraints" in `docs/architecture.md`:

- **stdout is sacred.** stdout is the MCP transport; all logging is routed to stderr
  (`logging.basicConfig(stream=sys.stderr)`, `server.py:28`). Never `print()` to
  stdout in the server path.
- **Never block the event loop.** FastMCP runs sync tools on the loop, so all three
  handlers are `async def` and push blocking work (`session.start` / `session.stop`)
  through `anyio.to_thread.run_sync` (`server.py:145, 174, 180`). The comment at
  `server.py:35-37` documents this rationale.
- **server.py only orchestrates.** It validates args, constructs/tracks sessions, and
  offloads — no capture logic lives here (`docs/architecture.md` dependency rules).
- **Exactly-one-target.** `capture_start` requires precisely one of `command`, `pid`,
  `app_name`, using the `_present` semantics (blank strings don't count; `pid=0`
  counts and is later rejected by the session as invalid).
- **Registry access is guarded.** All reads/writes of `_sessions` happen under
  `_lock` (a `threading.Lock`), because session objects are mutated from worker
  threads while the registry is read/written from the event-loop thread.
- **Bounded retained history.** `_prune_locked` keeps the registry from growing
  without bound by evicting oldest **finished** sessions. Running sessions are never
  evicted, so the bound is on retained finished history, not absolute size (assumes
  few concurrent live captures — documented at `server.py:45-50`).
- **Registry insertion is post-start.** A session enters `_sessions` only after
  `session.start()` succeeds, so the registry never contains sessions that failed to
  start.

## Failure modes & handling

- **No target / multiple targets** (`capture_start`): raises `ValueError` with one of
  the two messages in Behavior steps 3-4. Session is not constructed.
- **`pid=0`**: treated as *present* by `_present`, so it passes the exactly-one-target
  check, but is rejected downstream as invalid by `CaptureSession` (per the comment at
  `server.py:113-115`). The server itself does not validate the pid value.
- **`session.start()` raises** (e.g. command fails to launch, ASR load fails): the
  exception propagates out of `capture_start`; the session is not registered. Cleanup
  of partially started components is owned by `CaptureSession.start()`.
- **Unknown `session_id`** (`capture_stop`, `capture_status`): raises
  `ValueError(f"unknown session_id {session_id!r}")`.
- **`capture_stop` with no `session_id` and nothing running**: returns
  `{"stopped": [], "note": "no running captures"}` (not an error).
- **`capture_stop` with no `session_id` and multiple running**: raises `ValueError`
  listing the running ids; the caller must retry with an explicit id.
- **Exceptions in general**: any `ValueError` (or other exception) raised by a handler
  is surfaced by FastMCP as an MCP tool error to the client. Errors are not logged
  explicitly by the handlers themselves; FastMCP handles propagation.

## Outputs / artifacts

This scope writes **no files of its own**. All on-disk artifacts (screenshots,
`stdout.log`/`stderr.log`/`output.log`, `audio.s16le`, `transcript.jsonl`/`.txt`,
`session.json`) are produced by `CaptureSession` and its components under
`<output_dir>/capture-<id>/` (per the `capture_start` docstring and
`docs/architecture.md`). The server's only outputs are:

- Tool return values (the `dict` summaries described in Public contract).
- Log lines on **stderr** (INFO level, format `"%(asctime)s %(levelname)s %(name)s:
  %(message)s"`).

## Configuration

- **Module constants:**
  - `MAX_SESSIONS = 100` (`server.py:41`) — registry bound used by `_prune_locked`.
  - Logging level `INFO`, stream `sys.stderr` (`server.py:26-30`).
- **No environment variables** are read by `server.py` itself. (ASR backends and the
  Swift helper may consult their own env/config, but that is out of this scope.)
- **Per-call configuration** is entirely via tool parameters (see Public contract for
  names, types, and defaults).

## Known limitations / open items

- `_prune_locked` bounds **finished** sessions only; a pathological scenario with many
  simultaneous *running* sessions could exceed `MAX_SESSIONS` (acknowledged as
  acceptable in the code comment, `server.py:45-50`).
- `MAX_SESSIONS` and the logging level are hard-coded; there is no env/config override.
- `capture_status` calls `session.summary()` while holding `_lock`; if `summary()`
  ever performs blocking I/O this would briefly serialize against other registry
  operations. Currently assumed cheap — not independently verified in this scope.
- The exact shape of the summary `dict` (keys/types returned by `start`, `stop`,
  `summary`) is defined by `session.py` and not pinned here.
- Sessions are kept only in memory; restarting the server loses all session history.
- macOS-only, per `docs/architecture.md` (inherited via the components).

## Tests

- `tests/smoke.py` is the referenced verification entry point for the project. (Its
  current coverage of this scope was not inspected while writing this spec — verify
  before relying on it.)
- Suggested coverage specific to this scope, all exercisable without real capture by
  stubbing `CaptureSession`:
  - **Exactly-one-target validation**: zero targets and >1 target each raise the
    expected `ValueError`; each single target (incl. `pid=0` reaching the session
    layer) is accepted by the validation step.
  - **`_present` semantics**: blank/whitespace `command`/`app_name` are treated as
    absent; `pid=0` is treated as present.
  - **Registry + `_prune_locked`**: with `MAX_SESSIONS` reachable, inserting beyond
    the bound evicts oldest *finished* sessions and never evicts *running* ones;
    lexical/chronological ordering holds for timestamp-prefixed ids.
  - **`capture_stop` dispatch**: no-id + none running → `{"stopped": [], "note": ...}`;
    no-id + exactly one running → stops it; no-id + multiple running → `ValueError`
    listing ids; explicit unknown id → `ValueError`.
  - **`capture_status` dispatch**: known id → `summary()`; unknown id → `ValueError`;
    no id → `{"sessions": [...]}`.
  - **Async/offload contract**: handlers are `async def` and route blocking work
    through `anyio.to_thread.run_sync` (assert `start`/`stop` are invoked off the loop
    thread; assert no `print`/stdout writes occur in the server path).
