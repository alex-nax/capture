# Spec: Daemon (`captured`) + CLI (`capture`)

_Status: current as of 2026-06-15. Source of truth = the code; update this spec in the same change as the code._

## Purpose

`captured` is the V2 daemon-peers nucleus (see
[product-architecture.md](product-architecture.md)): a local HTTP `/v1` API over
the capture engine, so the MCP server, the `capture` CLI, and (later) the GPUI
app are thin peer clients of **one** shared `SessionRegistry` — an
agent-started capture is visible to the CLI, sessions outlive any single client,
and (once packaged + signed) the daemon is the macOS Screen Recording
TCC-responsible process so one grant covers every client (spike #30 PASSED).

**This slice** is stdlib-only (no new deps): HTTP/1.1 on `127.0.0.1:<ephemeral>`
with a bearer token. The unix-domain-socket transport, the WebSocket event
stream, daemon lifecycle install (launchd/systemd/Task Scheduler), and the MCP
server's daemon-first mode are **[planned]** (see Known limitations).

## Files

- `src/capture_mcp/daemon/server.py` — `CaptureDaemon` (ThreadingHTTPServer +
  shared `SessionRegistry` + token), the `_Handler` routes, `daemon_json_path`,
  `write_daemon_json`, `run_daemon`, `main`.
- `src/capture_mcp/daemon/client.py` — `DaemonClient` (stdlib urllib),
  `from_discovery`, `available`, and one method per route. Reused by the CLI and
  intended for the MCP daemon-first mode.
- `src/capture_mcp/daemon/__main__.py` — `python -m capture_mcp.daemon`.
- `src/capture_mcp/cli/__init__.py` (+ `__main__.py`) — the `capture` CLI.
- Console scripts (`pyproject.toml`): `captured` → `daemon.server:main`,
  `capture` → `cli:main`.

## Public contract

### Transport & discovery
- Bind `127.0.0.1` on an ephemeral port (`port=0`). Endpoint is
  `http://127.0.0.1:<port>`.
- `~/.capture/daemon.json` (override `CAPTURE_DAEMON_JSON`), mode **0600**:
  `{endpoint, token, pid, api_version, version}`. Written on start, removed on
  clean stop.
- **Auth:** every route except `GET /v1/health` requires
  `Authorization: Bearer <token>` (constant-time compared). Missing/wrong → 401.
- `API_VERSION = "1.0"`.

### Routes (all JSON)
| Method | Path | Body / query | Returns |
|--------|------|--------------|---------|
| GET  | `/v1/health` | — (no auth) | `{ok, version, api_version, pid, platform, sessions:{live,history}}` |
| GET  | `/v1/windows` | `?app_name=&pid=` | `{windows:[...], count}` (via `core.list_windows`) |
| POST | `/v1/sessions` | `capture_start` args + `output_dir` | session summary (201); exactly-one-target enforced |
| GET  | `/v1/sessions` | — | `{sessions:[summary,...]}` (live + recovered, oldest first) |
| GET  | `/v1/sessions/{id}` | — | session summary (404 if unknown) |
| POST | `/v1/sessions/{id}/stop` | — | final summary; a recovered (finished) id returns its record |
| GET  | `/v1/sessions/{id}/transcript` | `?tail=N` | `{session_id, segments:[...], count}` from `transcript.jsonl` |
| GET  | `/v1/events` | — | **SSE** (`text/event-stream`): each `data:` line is one event `{t, type, session_id, …}`; `: ping` heartbeats |
| POST | `/v1/admin/shutdown` | — | `{shutdown:true}` then the server stops |

Errors are `{"error": <message>}` with the documented status (400 bad
request/validation, 401 auth, 404 unknown, 500 unexpected — never a stack trace).

### CLI (`capture`)
`daemon start|stop|status` · `status [SESSION_ID]` · `windows [--app N] [--pid P]`
· `start --out DIR (--command C | --pid P | --app N) [--interval --no-screenshots
--no-audio --audio-source --asr]` · `stop [SESSION_ID]` (the unique running one if
omitted) · `tail SESSION_ID [-n N]` · `watch [SESSION_ID]` (stream `/v1/events`,
optionally filtered to one session; Ctrl-C to stop). Prints JSON; non-zero exit +
`{"error":…}` on failure. `daemon start` spawns `python -m capture_mcp.daemon`
detached (`start_new_session`) and waits for `/v1/health`.

## Behavior

- **Engine reuse:** `POST /v1/sessions` builds a `CaptureSession`, calls
  `registry.add(session)` **before** `session.start()` (so a slow/failed start is
  visible as `starting`/`error` — same contract as the MCP server), then returns
  the start summary. Blocking work runs in the handler thread; `SessionRegistry`
  is thread-safe, so concurrent clients are safe.
- **Single instance:** `run_daemon` refuses to start if `daemon.json` exists and
  the referenced endpoint answers `/v1/health` (`SystemExit(3)`).
- **Exactly-one-target** and the `output_dir`-required rule mirror the MCP server;
  unknown body fields are rejected (400).
- **Event stream (`/v1/events`, SSE):** each `CaptureSession`'s `EventBus`
  (events.md) is forwarded into a daemon-wide fan-out by a per-session forwarder
  thread (`attach_stream`, subscribed **before** `start()` so `starting`/`running`
  are carried, ending after the terminal `stopped`/`error` event). Each event is
  tagged with `session_id`. Connected SSE handlers each hold a bounded queue;
  fan-out is **live-only** (no replay — late joiners read `events.jsonl`); a slow
  client drops events rather than blocking a capture. Heartbeat (`: ping`) every
  `CAPTURE_SSE_HEARTBEAT_SECONDS` (default 15) keeps the connection alive and lets
  the writer notice a dead client. **SSE, not WebSocket:** the event channel is
  one-way (daemon→client), which SSE serves from the stdlib server with no dep;
  clients send commands via the REST routes. WebSocket stays **[planned]** only if
  a bidirectional channel is ever needed.
- **stdout is NOT special** here (unlike `server.py`): the daemon is its own
  process; logs go to stderr.

## Invariants & constraints

- **No new runtime dependency** (stdlib `http.server` + `urllib`); the daemon is
  not part of the `minimal` install and adds nothing to the agent path.
- **Token is a secret:** `daemon.json` is created 0600; the token is never logged.
- **The daemon does not change capture behavior** — it wraps the same engine, so
  the session-dir layout, `session.json`, transcripts, and events are identical to
  the MCP/embedded path.
- **No capture logic lives here** — `server.py` (MCP) and `daemon/` are both thin
  frontends over `core/`; `daemon/` imports `core`, never the MCP server.

## Failure modes & handling

- Wrong/missing token → 401. Invalid JSON body → 400. Unknown session → 404.
  Bad target (zero/multiple) or missing `output_dir` → 400. Capture fails to
  start → 400 `capture failed to start: …` (the session stays registered as
  `error`, recoverable via `GET /v1/sessions`).
- A second `captured` while one is live → exit 3.
- CLI with no daemon running → exit 1, `no daemon running; start it with
  capture daemon start` (no embedded fallback in the CLI — that is the MCP
  server's job).
- Unexpected handler exception → 500 `{"error": "<Type>: <msg>"}`, logged with a
  stack trace server-side only.

## Outputs / artifacts

- `~/.capture/daemon.json` (0600), removed on clean stop. No other files — all
  capture artifacts are produced by the engine under each session dir.

## Configuration

- `CAPTURE_DAEMON_JSON` — discovery file path (default `~/.capture/daemon.json`).
- `CAPTURE_SSE_HEARTBEAT_SECONDS` — `/v1/events` keep-alive interval (default 15).
- Inherits `CAPTURE_SESSION_INDEX` (registry history) and all engine env
  (`CAPTURE_WHISPER_MODEL`, `CAPTURE_OPENAI_ASR_URL`, …).

## Known limitations / open items

- **Transport:** 127.0.0.1 + token only. **[planned]** unix domain socket
  (macOS/Linux, peer-uid check) per product-architecture.md; 127.0.0.1 stays the
  Windows transport.
- **Event stream: DONE via SSE** (`GET /v1/events`, see Behavior). **[planned]**
  WebSocket only if bidirectional is ever needed; a small per-session ring buffer
  for late-joiner replay (today: live-only, history via `events.jsonl`).
- **MCP daemon-first mode: DONE.** The MCP server (`server.py`) proxies every tool
  to a running daemon and falls back to the embedded engine otherwise
  (`CAPTURE_MCP_EMBEDDED=1` forces embedded). See mcp-server.md. The *grant*-sharing
  benefit ("one grant covers every terminal's agent") additionally needs the
  packaged signed daemon (#31) to be the stable TCC-responsible process; the
  routing mechanism itself is in place now.
- **No daemon lifecycle install** (launchd agent / systemd user unit / Windows
  logon task) — `capture daemon start` is a foreground-detached spawn for now.
- **No pydantic models / JSON-Schema contract** yet (the routes return engine
  dicts directly). **[planned]** schema emission + Rust type generation for the
  GUI (product-architecture.md "contract firewall").
- `transcript` reads the whole `transcript.jsonl` then tails in memory (fine for
  meeting-scale files; a seek-based tail is a later refinement).

## Tests

`tests/smoke.py` (hermetic; `CAPTURE_DAEMON_JSON` pointed at a temp file):
- `test_daemon` (in-process `CaptureDaemon` + `DaemonClient`): `/v1/health`,
  **401 without a token**, a launch-mode capture round-trip through the API
  (start → list → stop, `log_lines == 6`), `/v1/windows`, transcript-tail shape,
  and a 404 for an unknown id.
- `test_cli_daemon`: the `capture` CLI **spawns a real daemon subprocess** and
  drives it — `daemon start` → `daemon status` (running) → `windows` → `status`
  → `daemon stop`.
- `test_sse_events`: an SSE client connects to `/v1/events` **before** a capture
  starts and receives the full state lifecycle (`starting`→`running`→`stopping`→
  `stopped`) plus live `log_line`/`screenshot_taken`, all tagged with `session_id`.
  (`CAPTURE_SSE_HEARTBEAT_SECONDS` is lowered in the suite.)
