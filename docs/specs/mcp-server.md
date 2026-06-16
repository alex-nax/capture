# Spec: MCP Server

_Status: current as of 2026-06-10. Source of truth = the code; update this spec in the same change as the code._

## Purpose

The MCP server is the entrypoint and orchestration layer for capture-mcp. It exposes
on-demand process capture to an MCP client over stdio as five tools
(`capture_start`, `capture_stop`, `capture_status`, `list_windows`, `list_audio_devices`). It validates arguments,
constructs `CaptureSession` objects and tracks them via the shared
`core.registry.SessionRegistry` (bounded live tracking + disk-backed history — see
[session-registry.md](session-registry.md)), and offloads all blocking work to worker
threads so the MCP event loop (and the stdio transport) stays responsive. Per `docs/architecture.md`, this layer **only
orchestrates** — it must not contain capture logic itself (that lives in `session.py`
and the components).

## Files

- `src/capture_mcp/server.py` — the entire scope (the `FastMCP` instance, the three
  tool handlers, the module-level `registry = SessionRegistry()`, and `main`).

Imported but out of scope here:
- `src/capture_mcp/core/session.py` — `CaptureSession` (called via `.start()`, `.stop()`).
- `src/capture_mcp/core/registry.py` — `SessionRegistry` (the registry itself moved to
  the engine in the M0a split; specced in [session-registry.md](session-registry.md)).

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

### Tool: `capture_start` (`server.py:45-135`)

Async handler returning a `dict` (the session summary produced by
`session.start()`). Parameters (name : type = default):

- `output_dir: str` (required) — base directory for the session folder.
- `command: str | None = None` — shell command to launch and capture (the only mode
  that captures logs).
- `pid: int | None = None` — PID of a running process to attach to.
- `window_id: int | None = None` — pin screenshots to this exact window (a `window_id`
  from `list_windows`); refines a `pid`/`app_name` target when one process owns several
  windows (audio stays per-process).
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
- `mic_device: str | None = None` — also record this input device as a SEPARATE mic track
  (`mic.s16le`/`mic_transcript.jsonl`) in addition to the app audio; id from
  `list_audio_devices`, or `"default"`. The helper applies acoustic echo cancellation.
- `audio_chunk_seconds: float = 8.0`.
- `asr_backend: str = "auto"` — `"auto"`, `"local"`/`"whisper"`, or `"nemotron"`/`"riva"`.
- `cwd: str | None = None` — working directory for a launched command.

**Target selection rule:** exactly one of `command`, `pid`, `app_name` must be
provided (see Behavior / Failure modes for the precise notion of "provided").

Return: the `dict` returned by `CaptureSession.start()` (a session summary that
includes `session_id`). The exact shape is defined by `session.py` and is not fixed
by this scope.

### Tool: `capture_stop` (`server.py:138-168`)

Async handler. Parameter:

- `session_id: str | None = None` — the session to stop. If omitted: stop the unique
  running session if there is exactly one.

Returns a `dict`:
- When `session_id is None` and **no** sessions are running:
  `{"stopped": [], "note": "no running captures"}` (`server.py:152`).
- For a session this server owns: the `dict` returned by its `.stop()` (final summary).
- For a finished session recovered from the on-disk index (a previous server's
  session): its recovered record, as-is — mirroring `stop()`'s idempotent
  return-summary behavior on already-stopped sessions (`server.py:163-167`).

### Tool: `capture_status` (`server.py:171-185`)

Async handler. Parameter:

- `session_id: str | None = None`.

Returns a `dict`:
- If `session_id` is given: `registry.summary(session_id)` — the live summary for a
  session this server owns, else the recovered history record.
- If omitted: `{"sessions": registry.summaries()}` — every session this server has
  created **plus** finished sessions recovered from the on-disk index at startup,
  oldest first, up to the registry bound.

### Tool: `list_windows` (`server.py:188-202`)

Async handler (added in feature #29). Parameters:

- `app_name: str | None = None` — case-insensitive substring filter.
- `pid: int | None = None` — process id filter.

Returns `{"windows": [...], "count": <len>}` where each entry is a JSON-ready
dict with keys `window_id`, `pid`, `app_name`, `title`, `width`, `height`,
ordered largest-area first (the first match is what `capture_start` would
target). Backed by `core.list_windows()` -> `platform.current().window_finder
.find()` (the same picker the daemon's `/v1/windows` and the GUI will use),
offloaded via `anyio.to_thread.run_sync`. May be empty (no error). Note: on
macOS without the Screen Recording grant, window titles can be empty
strings — fields are stable, contents permission-dependent.

### Tool: `list_audio_devices` (`server.py`)

Async handler, no parameters. Returns `{"devices": [{id, name, default}]}` — the
microphone/input devices for `capture_start`'s `mic_device`. Daemon-first
(`GET /v1/audio/mics`); embedded fallback calls
`platform.current().audio_source.list_input_devices()` (macOS: the bundled
`audiocap --list-mics`; other platforms: empty list).

### Errors

Validation/lookup failures are raised as `ValueError`. FastMCP converts a raised
exception into an MCP tool error result; the messages below are the exact strings the
code raises (see Failure modes).

## Behavior

### Daemon-first dispatch (all four tools)

`_daemon()` returns a live `DaemonClient` when a `captured` daemon is discoverable
(`~/.capture/daemon.json`) and answers `/v1/health`, **unless** `CAPTURE_MCP_EMBEDDED`
is set (forces embedded). The check is per-call and cheap (~2 s probe), so a daemon
started/stopped mid-session is picked up. When a daemon is present, each tool proxies
to it (the blocking client call is offloaded via `anyio.to_thread.run_sync`, and
`DaemonError` is remapped to `ValueError` so the surfaced message matches the embedded
path); otherwise the tool runs the embedded engine exactly as described below. Argument
validation (exactly-one-target) happens in the tool **before** dispatch, so validation
errors are identical regardless of backend. `capture_stop`'s "stop the unique running
one" resolution is replicated against the daemon's `/v1/sessions` for the daemon path.
This is what lets an MCP agent share one live registry — and, with the packaged signed
daemon (#31/#30), one Screen Recording grant — with the CLI and GUI.

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
   (`server.py:114-130`). Construction is synchronous and happens on the event loop
   (it is expected to be cheap; the heavy work is in `.start()`).
6. Calls `registry.add(session)` **before** starting (`server.py:131-134`), so
   `capture_status` already lists the session in state `"starting"` while a slow
   start (ASR model load) is in flight.
7. Returns `await anyio.to_thread.run_sync(session.start)` — the blocking start runs
   on a worker thread (`server.py:135`).

Note: if `session.start()` raises, the exception propagates and the session
**remains registered** in state `"error"` (visible in `capture_status`, and recorded
on disk via `session.json`) instead of vanishing as it did pre-M0a. Rollback of
partially started components is `CaptureSession.start()`'s responsibility (per
`docs/architecture.md`).

### `capture_stop`

1. If `session_id is None`: snapshot `running = registry.running()` (state
   `"running"` only — a `"starting"` session is not yet stoppable):
   - If empty: return `{"stopped": [], "note": "no running captures"}`.
   - If more than one: raise `ValueError("multiple captures running; pass
     session_id. Running: " + <ids>)`.
   - Otherwise: return `await anyio.to_thread.run_sync(running[0].stop)`.
2. If `session_id` is given: `registry.get(session_id)` → live session → offloaded
   `.stop()`. Else `registry.history_record(session_id)` → return the recovered
   record (already finished). Else raise
   `ValueError(f"unknown session_id {session_id!r}")` (`server.py:160-168`).

### `capture_status`

1. If `session_id` given: return `registry.summary(session_id)` (live summary or
   recovered record), or raise `ValueError(f"unknown session_id {session_id!r}")` if
   the registry knows nothing about it (`server.py:180-184`).
2. Otherwise return `{"sessions": registry.summaries()}` (`server.py:185`).

Locking, pruning, and history recovery are the registry's concern now — see
[session-registry.md](session-registry.md). `summary()` is still assumed cheap (it
is called under the registry lock).

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
- **Registry access is thread-safe** inside `SessionRegistry` (its own lock); the
  server holds no registry lock itself (see [session-registry.md](session-registry.md)).
- **Bounded retained history.** The registry evicts oldest **finished** sessions
  (live ones never), same tradeoff as pre-M0a; the bound now covers recovered
  history records too.
- **Registry insertion is PRE-start** (changed in M0a). A session is registered
  before `session.start()` runs, so `capture_status` sees `"starting"` sessions and
  failed starts remain visible as `"error"` records.

## Failure modes & handling

- **No target / multiple targets** (`capture_start`): raises `ValueError` with one of
  the two messages in Behavior steps 3-4. Session is not constructed.
- **`pid=0`**: treated as *present* by `_present`, so it passes the exactly-one-target
  check, but is rejected downstream as invalid by `CaptureSession` (per the comment at
  `server.py:113-115`). The server itself does not validate the pid value.
- **`session.start()` raises** (e.g. command fails to launch, ASR load fails): the
  exception propagates out of `capture_start`; the session stays registered in state
  `"error"`. Cleanup of partially started components is owned by
  `CaptureSession.start()`.
- **Unknown `session_id`** (`capture_stop`, `capture_status`): raises
  `ValueError(f"unknown session_id {session_id!r}")` — "unknown" now means unknown
  to live tracking AND to recovered history.
- **`capture_stop` on a recovered (already finished) session**: returns the recovered
  record; not an error.
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

- **Module constants:** logging level `INFO`, stream `sys.stderr` (`server.py:26-30`).
  The registry bound (`MAX_SESSIONS = 100`) moved to `core/registry.py`.
- **Environment:** `CAPTURE_MCP_EMBEDDED` (any non-empty value) forces the embedded
  engine and disables daemon-first dispatch (use in headless/CI). `CAPTURE_DAEMON_JSON`
  (read via the daemon client) locates the daemon discovery file. Constructing
  `SessionRegistry()` at import time resolves the session-index path from
  `CAPTURE_SESSION_INDEX` (default `~/.capture/sessions.jsonl`) — set it before
  importing the module (tests do). ASR backends / the Swift helper consult their own
  env, out of scope here.
- **Per-call configuration** is entirely via tool parameters (see Public contract for
  names, types, and defaults).

## Known limitations / open items

- `MAX_SESSIONS` and the logging level are hard-coded; there is no env/config
  override beyond `CAPTURE_SESSION_INDEX`.
- The exact shape of the summary `dict` (keys/types returned by `start`, `stop`,
  `summary`) is defined by `session.py` and not pinned here.
- A `"starting"` session cannot be stopped; callers must wait for `"running"` (or the
  error). Acceptable for MCP polling; revisit for the daemon API (M2,
  [product-architecture.md](product-architecture.md)).
- macOS + Windows via the platform abstraction (inherited via the components).

## Tests

- `tests/smoke.py` (hermetic, 28 checks as of M0a) covers this scope's happy paths:
  launch-mode start/status/stop round-trip, exactly-one-target validation (0 and 2
  targets), **status responsiveness during a slow start** (session visible as
  `"starting"`, status returns while start is in flight), and the disk-backed
  history rebuild (see [session-registry.md](session-registry.md) Tests). The suite
  sets `CAPTURE_SESSION_INDEX` to a temp path before importing `server`.
- `tests/contract/run_contracts.py` pins the tools/list contract (tool names +
  input schemas, descriptions excluded) against `tests/contract/golden/`; it
  fails on drift and regenerates with `--regen` after an intentional change
  (done for the `list_windows` addition, feature #29).
- Still uncovered here: `_present` blank-string semantics, multiple-running stop
  dispatch, prune-at-bound behavior, and the async/offload contract (no stdout
  writes) — open items.
