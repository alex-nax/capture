# Spec: Session

_Status: current as of 2026-06-10. Source of truth = the code; update this spec in the same change as the code._

## Purpose

The session scope defines `CaptureSession`, the orchestrator for a single capture run. It owns and sequences the capture components (`ProcessCapture`, `Screenshotter`, `AudioCapture`), resolves the capture target (launched command, given pid, or app name), manages a strict state lifecycle, creates the session directory, and persists metadata + summary to `session.json`. It is the only layer that knows about all components together; components do not know about each other or about the MCP layer (`docs/architecture.md`, "Dependency rules").

## Files

- `src/capture_mcp/core/session.py` — the `CaptureSession` class (the entire scope).

Dependencies it imports (owned by other scopes, not specced here): `capture_mcp.core.platform` (for `current().window_finder`, see [platform-abstraction.md](platform-abstraction.md)), `capture_mcp.core.audio.AudioCapture`, `capture_mcp.core.proc.ProcessCapture`, `capture_mcp.core.screenshots.Screenshotter` / `parse_resolution`, and `capture_mcp.core.util` (`fs_stamp`, `iso`, `now`), and `capture_mcp.core.events` (`EventBus` / `EventsFileWriter`, see [events.md](events.md)).

## Public contract

### `set_mic_device(device)` (#46)

Switch the microphone on a **running** session: `device` = an input-device id / `"default"` turns
the mic on or switches it; `None`/`""` turns it off. Stops the current `_mic` `AudioCapture` and starts
a new one with `append=True` (when `mic.s16le` already exists) so the mic track stays continuous — each
`AudioCapture`'s epoch is real wall-clock, so iso timestamps line up across the switch. Updates
`self.mic_device` (surfaced in `summary()`), rewrites `session.json`, emits a `mic_device` event. Raises
`RuntimeError` if the session isn't running. Heavy teardown/start runs outside `self._lock`.

### Constructor: `CaptureSession(output_dir, *, ...)` (lines 34–85)

Positional:
- `output_dir: str` — required. Base directory. The session directory is created as `Path(output_dir).expanduser().resolve() / f"capture-{self.id}"`.

Keyword-only (with defaults):
- `command: str | None = None` — shell command to launch (launch mode).
- `pid: int | None = None` — existing process pid to attach to (stored as `self.req_pid`).
- `app_name: str | None = None` — macOS app name to attach to by window owner.
- `bundle_id: str | None = None` — bundle id, forwarded to `AudioCapture`.
- `screenshot_interval: float = 1.0`
- `screenshot_format: str = "png"`
- `screenshot_resolution: str | None = None` — a `"WxH/fmt"`-style spec, parsed by `parse_resolution`.
- `screenshot_jpeg_quality: int | None = None`
- `capture_screenshots: bool = True`
- `capture_audio: bool = True`
- `audio_source: str = "auto"`
- `audio_chunk_seconds: float = 8.0`
- `asr_backend: str = "auto"`
- `cwd: str | None = None` — working directory for launched command.

Constructor side effects / derived attributes:
- `self.id = f"{fs_stamp()}-{secrets.token_hex(3)}"` (line 53–54) — `fs_stamp()` timestamp plus a 6-hex-char random token.
- `self.dir` — resolved session directory path (not yet created).
- `screenshot_resolution` is parsed via `parse_resolution(...)`; if it yields a 3-tuple, `self.screenshot_resolution = (w, h)` else `None`, and the format is overridden: `self.screenshot_format = (parsed[2] if parsed and parsed[2] else screenshot_format).lower()` (lines 63–66). The original spec string is preserved as `self.screenshot_resolution_spec`.
- Initial state: `self.state = "created"`; `self.t0/self.t1/self.pid/self.window_title = None`; `self.notes = []`; component handles `_proc/_shots/_audio = None`; `self._lock = threading.Lock()`.

The constructor does NOT create directories, start anything, or write files.

### Methods

- `start() -> dict` (lines 97–142) — starts the session, returns `summary()`. Raises `RuntimeError` if not in `created` state, or if launch mode fails to start the process. On any component-start failure, rolls back and re-raises. Sets state `"starting"` (and writes `session.json`) before component startup, which runs OUTSIDE the lock.
- `stop() -> dict` (lines 144–164) — stops the session, returns `summary()`. Idempotent for non-`running` states (returns current summary without changing state).
- `summary() -> dict` (lines 208–226) — see "Outputs / artifacts" for exact fields.
- `_stop_components() -> int | None` (lines 136–153) — internal best-effort teardown; returns process exit code or `None`. (Not part of external API but documented in focus notes.)
- `_resolve_target()`, `_start_screenshots()`, `_start_audio()`, `_write_metadata()` — internal helpers.

### States

`self.state` is one of the string literals: `"created"`, `"starting"`, `"running"`, `"stopping"`, `"stopped"`, `"error"`. There is no enum; states are bare strings compared directly in code.

## Behavior

### Construction
1. Generate `id` and resolve `dir`; store all config; parse resolution spec; set `state = "created"`. No I/O.

### `start()` (lines 89–115)
1. Acquire `self._lock`.
2. If `self.state != "created"`, raise `RuntimeError(f"session already {self.state}")` (line 91–92). (A concurrent second `start()` therefore raises `"session already starting"`.)
3. Create the session directory: `self.dir.mkdir(parents=True, exist_ok=True)` (line 93).
4. Set `self.t0 = now()` (line 94).
5. In a `try` block:
   a. `self._resolve_target()` (resolves pid/window and, in launch mode, starts the process).
   b. Launch-mode guard: if `self.command` is set but `self._proc is None`, raise `RuntimeError` with the last note (or `"could not launch command"`) — a launch session whose command never started has captured nothing and must fail loudly rather than report a phantom `running` (lines 98–101).
   c. If `capture_screenshots`, call `_start_screenshots()` (lines 102–103).
   d. If `capture_audio`, call `_start_audio()` (lines 104–105).
6. On ANY exception in the try: call `_stop_components()`, then under the lock set `state = "error"` and write metadata, and re-raise. This is the partial-start rollback.
7. On success: under the lock set `state = "running"` and write metadata; publish the `"running"` state event; log an info line; return `summary()`.

Each state transition also publishes a `state` event on `self.events` (an `EventBus`, public attribute) and the error/stopped paths close the `events.jsonl` writer (final snapshot) — see [events.md](events.md). Components receive `emit=self.events.publish`.

Only the `created`-check / `mkdir` / `t0` / `"starting"` transition runs under `self._lock` (with a `session.json` write recording `"starting"`, the `events.jsonl` writer start, and the `"starting"` state event publish). Component startup — `_resolve_target`, screenshots, audio/ASR load — runs OUTSIDE the lock, mirroring `stop()`'s teardown, so concurrent `stop()`/state reads return immediately observing `"starting"`. The `"error"` and `"running"` transitions re-take the lock.

### `_resolve_target()` (lines 157–176)
1. If `command` is set: create `ProcessCapture(command, dir, cwd=cwd)`, attempt `self.pid = self._proc.start()`. On exception, set `self._proc = None`, append `f"launch failed: {e}"` to notes, log the exception, and return (lines 158–166). (The launch-mode guard in `start()` then converts this into a raised error.)
2. Else if `req_pid is not None`: `self.pid = self.req_pid` (lines 168–169).
3. Else if `app_name` is set: call `platform.current().window_finder.primary(app_name=...)` (the platform abstraction; macOS Quartz or Windows `EnumWindows`). If a `WindowRef` is found, set `self.pid = w.pid` and `self.window_title = w.title or w.app_name`; otherwise append note `f"no on-screen window found for app {app_name!r}"`.
4. If none of the above (no command, no pid, no app_name): nothing is resolved; `self.pid` stays `None`.

### `_start_screenshots()` (lines 178–190)
1. Construct a `Screenshotter` at `self.dir / "screenshots"` with `pid=self.pid`, `app_name=None if self.pid else self.app_name` (so attach-by-app keeps re-resolving the window per tick when no pid is known), and the interval/format/resolution/jpeg-quality config.
2. Call `self._shots.start()`.

### `_start_audio()` (lines 192–204)
1. Construct an `AudioCapture` at `self.dir` with `pid`, `bundle_id`, `source`, `chunk_seconds`, `asr_backend`, and `t0=self.t0`.
2. Call `self._audio.start()`.
3. If `self._audio.status` starts with `"asr-unavailable"` or equals `"no-audio-source"`, append note `f"audio: {status}"` (lines 203–204). These are surfaced as notes but do NOT abort the session.

### `stop()` (lines 117–134)
1. Acquire `self._lock`; if `self.state not in ("running",)`, return `summary()` unchanged (idempotent / no-op for `created`, `stopping`, `stopped`, `error`) (lines 118–120).
2. Set `state = "stopping"` and release the lock (lines 121–122).
3. Call `self._stop_components()` OUTSIDE the lock (lines 123–125) — heavy teardown must not block concurrent status queries.
4. Re-acquire the lock: if `self._proc is not None`, append `f"process exit code: {rc}"`; set `self.t1 = now()`; set `state = "stopped"`; write metadata (lines 127–132).
5. Log info; return `summary()` (lines 133–134).

### `_stop_components()` (lines 136–153)
1. If `_shots`: call `.stop()`, catching and logging any exception (lines 138–142).
2. If `_audio`: call `.stop()`, catching and logging any exception (lines 143–147).
3. If `_proc`: call `.stop()` and return its result; catch/log any exception and fall through (lines 148–152).
4. Return `None` if no `_proc` or if `_proc.stop()` raised (line 153).

Teardown order is screenshots, then audio, then process. Each component is wrapped independently, so a failure in one does not prevent stopping the others (best-effort).

## Invariants & constraints

- **State machine is linear/forward-only.** Valid transitions in code: `created -> starting -> running` (start success), `created -> starting -> error` (start failure), `running -> stopping -> stopped` (stop). `start()` is only valid from `created`; calling it otherwise raises. `stop()` only acts from `running`; from any other state — including `starting` — it is a no-op returning the current summary (a session cannot be stopped until it reaches `running`).
- **Roll back on partial start** (`docs/architecture.md` hard constraint). `start()` calls `_stop_components()` on any failure, transitions to `error`, persists metadata, and re-raises. This guarantees no half-started components are left running.
- **Launch mode must produce a process.** A `command` session with `_proc is None` is treated as a hard failure (lines 100–101).
- **Surface failures, don't swallow them** (`docs/architecture.md`). Audio/ASR start problems are recorded as notes; component-level failure statuses live in `audio_status`/`asr_errors` (owned by `AudioCapture`), not overwritten here.
- **Locking discipline.** `self._lock` guards state transitions and metadata writes only. Both heavy paths run outside the lock: component teardown in `stop()` AND component startup in `start()` (since M0a), so status/stop calls concurrent with a slow start return immediately (observing `"starting"`).
- **Components are isolated.** `CaptureSession` is the only object that holds all three components; it never lets them reference each other (`docs/architecture.md`, Dependency rules).
- **Audio format/timestamp conventions** are delegated: `t0` (from `util.now()`) is passed to `AudioCapture` for offset computation; `util.iso`/`util.fs_stamp` are used for content vs. filename timestamps (`docs/architecture.md`, Naming/conventions).
- **Cross-platform** (macOS + Windows): the session orchestration is OS-neutral; the OS-specific
  capture it drives lives behind `capture_mcp.core.platform` (see [platform-abstraction.md](platform-abstraction.md)).

## Failure modes & handling

- **`start()` called when not `created`** → raises `RuntimeError(f"session already {self.state}")`; no state change.
- **Launch command fails to start** (`_resolve_target` catches the exception, sets `_proc=None`, appends a `launch failed:` note) → `start()` guard raises `RuntimeError` with the last note; rollback path runs (`_stop_components`, `state="error"`, metadata written), then re-raises.
- **Any component `start()` raises** (screenshots or audio) → caught at lines 106–110: `_stop_components()` is called to tear down whatever started, `state="error"`, metadata persisted, exception re-raised to the caller.
- **App attach finds no window** → not fatal: a note `"no on-screen window found for app ..."` is recorded; the session may still start (e.g. audio capture). `self.pid` remains `None`.
- **ASR unavailable / no audio source** → not fatal: recorded as a note (`audio: <status>`); the session continues. The detailed status is preserved in `audio_status`.
- **Component `.stop()` raises during teardown** → caught and logged via `log.exception(...)`; teardown of the remaining components still proceeds; for `_proc`, the return value falls through to `None`.
- **`session.json` write fails** → caught in `_write_metadata` (lines 248–251) and logged via `log.exception`; never raised. A metadata-write failure does not change session state or break `start()`/`stop()`.
- **`stop()` on a non-running session** → returns the current `summary()`; no teardown, no state change.

## Outputs / artifacts

### Session directory layout (module docstring, lines 3–14)

```
<output_dir>/capture-<stamp>-<id>/
    session.json        metadata + final summary
    stdout.log          raw stdout            (launch mode only)
    stderr.log          raw stderr            (launch mode only)
    output.log          merged, timestamped   (launch mode only)
    screenshots/        <iso-stamp>.png, one per interval
    transcript.jsonl    {start,end,offset,text} per recognized segment
    transcript.txt      human-readable, timestamped
    audio.s16le         raw captured audio (16 kHz mono s16le)
```

Of these, `session.py` itself directly creates the directory and writes `session.json`. The `stdout.log`/`stderr.log`/`output.log` files are produced by `ProcessCapture`; `screenshots/` by `Screenshotter`; `transcript.*` and `audio.s16le` by `AudioCapture`. Directory name: `capture-<self.id>` where `self.id = "<fs_stamp>-<6 hex chars>"` (`docs/architecture.md` describes this as `capture-<fs_stamp>-<token>`).

### `summary()` return shape (lines 208–226)

A dict with keys (types reflect the code at runtime):
- `session_id: str` — `self.id`.
- `state: str` — one of created/running/stopping/stopped/error.
- `dir: str` — `str(self.dir)`.
- `pid: int | None`.
- `window_title: str | None`.
- `started_at: str | None` — `iso(self.t0)` if set, else `None`.
- `stopped_at: str | None` — `iso(self.t1)` if set, else `None`.
- `screenshots: int` — `self._shots.count` or `0`.
- `screenshot_errors: int` — `self._shots.errors` or `0`.
- `log_lines: int` — `self._proc.lines` or `0`.
- `process_running: bool | None` — `self._proc.poll() is None` if a process exists, else `None`.
- `audio_mode: str` — `self._audio.mode` or `"off"`.
- `audio_status: str` — `self._audio.status` or `"off"`.
- `transcript_segments: int` — `self._audio.segments` or `0`.
- `asr_errors: int` — `self._audio.asr_errors` or `0`.
- `notes: list[str]` — a copy (`list(self.notes)`) taken as a snapshot, since notes may be appended concurrently.

### `session.json` contents (lines 228–251)

A JSON object (indented 2, `ensure_ascii=False`) with two top-level keys:
- `config` — echoes the constructor inputs: `command`, `pid` (the requested `req_pid`), `app_name`, `bundle_id`, `screenshot_interval`, `screenshot_format` (the resolved/lowercased format), `screenshot_resolution` (the original spec string `screenshot_resolution_spec`), `screenshot_jpeg_quality`, `capture_screenshots`, `capture_audio`, `audio_source`, `audio_chunk_seconds`, `asr_backend`, `cwd`.
- `summary` — the full `summary()` dict at write time.

`session.json` is (re)written on every state transition: when `start()` begins (state `starting` — this is what lets a crashed/killed process be recovered as `interrupted` by the registry, see [session-registry.md](session-registry.md)), at end of successful `start()` (state `running`), on the error path of `start()` (state `error`), and at end of `stop()` (state `stopped`). So the persisted summary reflects the latest transition.

## Configuration

All configuration is via constructor parameters (no env vars are read in `session.py`). Defaults:

| Parameter | Default |
| --- | --- |
| `command` | `None` |
| `pid` | `None` |
| `app_name` | `None` |
| `bundle_id` | `None` |
| `screenshot_interval` | `1.0` |
| `screenshot_format` | `"png"` |
| `screenshot_resolution` | `None` |
| `screenshot_jpeg_quality` | `None` |
| `capture_screenshots` | `True` |
| `capture_audio` | `True` |
| `audio_source` | `"auto"` |
| `audio_chunk_seconds` | `8.0` |
| `asr_backend` | `"auto"` |
| `cwd` | `None` |

Target-selection precedence in `_resolve_target` is: `command` (launch mode) > `pid` > `app_name`. If `command` is set, `pid`/`app_name` are ignored for target resolution. Logging uses the module logger `log = logging.getLogger(__name__)`; per `docs/architecture.md`, all logging must go to stderr (configured at the server entrypoint, not here).

## Known limitations / open items

- **`stop()` during `"starting"` is a silent no-op** returning the current summary; the caller must re-issue the stop once the session is `"running"`. Defined behavior, but a stop-requested-during-start flag (auto-stop on start completion) would be friendlier — open item for the daemon work (M2).
- **No `pause`/`resume` or restart.** A session is single-use: once `stopped`/`error`, it cannot be restarted (`start()` only works from `created`).
- **`error` state is terminal and unrecoverable** from within the object; the caller must create a new session.
- **Notes are appended from capture loops/threads** while `summary()` reads them; `summary()` snapshots via `list(...)` but there is no lock around individual `notes.append` calls in helpers, so the snapshot is best-effort (acceptable for a list of strings under CPython, but not formally synchronized).
- **`_write_metadata` swallows write failures** — if `session.json` cannot be written, the only signal is a logged exception; the run continues without persisted metadata.
- **No validation of `output_dir` writability** at construction time; directory creation/permission errors surface only at `start()` (`mkdir`), where they would propagate as an exception before the try/rollback block (note: the `mkdir` at line 93 and `t0` assignment are OUTSIDE the try/except, so a `mkdir` failure raises directly without setting `state="error"` or writing metadata — the session stays in `created` and no rollback runs). This edge case behavior should be confirmed/decided.

## Tests

- `tests/smoke.py` is the referenced smoke test for the project. As of this writing, verify it exercises at least: a launch-mode `CaptureSession` start/stop round-trip (asserting `state` transitions `created -> running -> stopped` and a non-empty `session.json`), and the rollback path (a session whose `command` cannot start raises and ends in `error` with metadata written). If `tests/smoke.py` does not yet cover these, they are open items.
- Recommended additional checks for this scope:
  - `start()` raises `RuntimeError` when state is not `created`.
  - `stop()` on a non-running session returns the current summary without changing state.
  - Attach-by-app with no window records the `no on-screen window found` note and still starts (when audio is enabled).
  - `summary()` returns the documented keys with correct fallback values (`0`/`None`/`"off"`) when components are absent.
  - `session.json` contains both `config` and `summary`, with `config.screenshot_resolution` equal to the original spec string and `config.pid` equal to the requested pid.
