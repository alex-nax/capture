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
- `src/capture_mcp/daemon/models.py` — the **`/v1` contract**: pydantic models
  (`StartSessionRequest` + response models) and `v1_schema()`. Pydantic is already a
  transitive dep (via `mcp`), so this adds nothing to the install.
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
| POST | `/v1/sessions` | `capture_start` args + `output_dir` | session summary (201); exactly-one-target enforced. Optional `window_id` pins screenshots to that exact window (audio stays per-pid); optional `mic_device` ALSO records that input device as a separate mic track (`mic.s16le`/`mic_transcript.jsonl`); `capture_screenshots:false` ⇒ an audio-only session (no `screenshots/`). Optional `preset` (#54: meeting/coding/lecture/auto/general/custom) records the capture intent + the `index_preset` a later index defaults to — both surfaced on the summary as `capture_preset`/`index_preset` |
| POST | `/v1/sessions/import` | `{path, output_dir?, asr_backend?, screenshot_interval?}` | import an audio/video file as a finished session (background, 202; progress over `/v1/events`: `import`→`import_done`/`_error`). The bundled helper extracts audio (→`audio.s16le`) and, for video, frames (→`screenshots/`, named on the audio timeline); ASR runs over the audio. A silent video imports as frames-only. 400 if the path is missing/not a file |
| GET  | `/v1/sessions` | — | `{sessions:[summary,...]}` (live + recovered, oldest first) |
| GET  | `/v1/sessions/{id}` | — | session summary (404 if unknown). Every summary carries **capability flags** `has_screenshots`/`has_audio`/`has_mic`/`can_retranscribe`, recomputed from on-disk artifacts on each read (so pruning is reflected for live + recovered sessions) |
| POST | `/v1/sessions/{id}/stop` | — | final summary; a recovered (finished) id returns its record |
| POST | `/v1/sessions/{id}/mic` | `{device}` | switch the microphone on a LIVE capture: a device id / `"default"` = on/switch, `null`/`""` = off. Appends to `mic.s16le`/`mic_transcript.*` (continuous). Returns the updated summary. 404 unknown/finished, 400 if not running |
| POST | `/v1/sessions/{id}/delete` | — | `{deleted, session_id}`; removes the session dir (guarded: must contain `session.json`) + its registry/index record. 404 unknown, **400 if still live** (stop first) |
| POST | `/v1/sessions/{id}/prune` | `{parts}` | free disk on a finished session — `parts` ⊆ `screenshots` (delete all), `screenshots_halve` (drop every other frame), `audio` (remove `audio.s16le`/`mic.s16le`). Returns `{pruned, freed_bytes, screenshots, <capability flags>}`. 400 if live/bad parts |
| POST | `/v1/sessions/{id}/retranscribe` | `{asr_backend?, model?}` | re-run ASR over `audio.s16le` with the active/chosen model, replacing the transcript (background, 202; progress over `/v1/events`: `retranscribe` → `retranscribe_done`/`_error`). 400 if live or audio pruned |
| POST | `/v1/sessions/{id}/index` | `{provider?, host?, port?, endpoint?, model?, sample_rate?, max_leaves?, fuse_transcript?, prompt_preset?, leaf_prompt?, leaf_schema?, classify_prompt?, max_px?}` | build the multimodal index — caption the screenshots with a remote vision LLM + summarize the timeline as a tree (background, 202; SSE `index`→`index_done`/`_error`). Endpoint comes from a full `endpoint` URL or composed from `provider`+`host`+`port` (#52). `prompt_preset` omitted ⇒ defaults to the session's recorded `index_preset` (#54), then "auto". 400 if live / no screenshots (`can_index`), **503 if the endpoint is unset or unreachable** |
| GET  | `/v1/sessions/{id}/index` | — | the built index tree (`index.json`); 404 if not indexed yet |
| GET  | `/v1/index/status` | `?url=&model=` | `{available, configured, url, model}` — is indexing usable (a configured + reachable vision endpoint). Drives the GUI gate |
| GET  | `/v1/index/providers` | — | `{providers:[{id,label,…}], default}` — the index vision-LLM providers (lmstudio/ollama/openai/custom) for the GUI selector (#52) |
| GET  | `/v1/index/models` | `?provider=&host=&port=&key=&url=` | `{models:[...], provider, reachable}` — a provider's available models (GETs its `/v1/models`); populates the GUI model dropdown (#53) |
| GET  | `/v1/sessions/{id}/transcript` | `?tail=N` | `{session_id, segments:[...], count}` from `transcript.jsonl` |
| GET  | `/v1/asr/runtimes` | — | `{active, gpu:{nvidia}, runtimes:[{id,label,kind,engine,device,requires,installed,active}]}` — selectable ASR runtimes (no engine bundled by default; user picks one by hardware). See [asr-runtimes.md](asr-runtimes.md) |
| POST | `/v1/asr/runtimes/install` | `{id, source?}` | `{id, started}` (202); downloads + extracts the runtime pack in the background (progress over `/v1/events`: `asr_runtime_install`→`_done`/`_error`) then sets it active. `source` overrides the pack URL with a local zip/dir or URL. 400 for an unknown id |
| POST | `/v1/asr/runtime` | `{id}` | `{active}` — set the active runtime (persisted; loaded into the running daemon). 400 if unknown / not installed |
| GET  | `/v1/asr/backend` | — | `{runtime, engine, device, available, error}` — the active runtime + whether an engine is importable + the last load error (so the GUI shows why ASR is off; **no silent fallback**) |
| GET  | `/v1/asr/models` | — | `{backend_available, active, models:[{repo,name,size_label,downloaded,active,downloading}]}` — Whisper model catalog (runtime-aware: faster-whisper repos vs mlx) |
| POST | `/v1/asr/models/download` | `{repo}` | `{repo, started}` (202); downloads in background, progress over `/v1/events`; dup is `started:false` |
| POST | `/v1/asr/models/delete` | `{repo}` | `{repo, deleted, freed_bytes}`; removes the model's weights from the HF cache. 400 if not in catalog, **409 if it's currently downloading** |
| POST | `/v1/asr/model` | `{repo}` | `{active}` — set the active model (persisted to `~/.capture/config.json`); 400 if not in catalog |
| GET  | `/v1/audio/mics` | — | `{devices:[{id,name,default}]}` — input devices for the mic selector (macOS: `audiocap --list-mics`; other platforms: `[]`) |
| GET  | `/v1/permissions` | — | `{platform, screen_recording, microphone}` — macOS TCC status (`granted`/`denied`/`undetermined`/`not_applicable`) |
| POST | `/v1/permissions/request` | `{kind}` | trigger the `screen_recording` or `microphone` prompt (the daemon is the grantee); 400 for an unknown kind |
| GET  | `/v1/events` | — | **SSE** (`text/event-stream`): each `data:` line is one event `{t, type, session_id, …}`; `: ping` heartbeats |
| GET  | `/v1/schema` | — | the `/v1` JSON Schema (`{api_version, models:{…}}`) from the pydantic models — for client/Rust-type generation |
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
detached (POSIX `start_new_session`; Windows `CREATE_NEW_PROCESS_GROUP|CREATE_NO_WINDOW`)
and waits for `/v1/health`.

## Behavior

- **Engine reuse:** `POST /v1/sessions` validates the body with the
  `StartSessionRequest` pydantic model (unknown fields, types, exactly-one-target,
  `output_dir` all enforced → 400 on failure), builds a `CaptureSession`, calls
  `registry.add(session)` + `attach_stream(session)` **before** `session.start()`
  (so a slow/failed start is visible as `starting`/`error` and its events stream —
  same contract as the MCP server), then returns the start summary. Blocking work
  runs in the handler thread; `SessionRegistry` is thread-safe, so concurrent
  clients are safe.
- **The `/v1` contract (models.py):** the request model validates at runtime; the
  **response** models are NOT enforced at runtime (the daemon serves engine dicts,
  resilient to benign additions) but ARE pinned by the contract test, which
  round-trips live responses through them and golden-compares `v1_schema()`. Models
  use `extra="forbid"`, so an unexpected field is a contract breach caught in CI.
  Because of this, the registry returns **uniformly full-shaped** session records
  (see [session-registry.md](session-registry.md)) so every `/v1/sessions` entry
  satisfies `SessionSummary`.
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
- **ASR model manager (`/v1/asr/*`):** lists the curated `mlx-community/whisper-*`
  catalog (`core.asr.manager`) with per-model `downloaded` (HF-cache check — config.json
  + **either** `weights.npz` *or* `weights.safetensors`, since `whisper-large-v3-turbo`
  ships safetensors while the rest ship npz) and `active` flags. `POST .../download`
  validates the repo against the catalog, then
  fetches it into the shared HF cache on a background thread (a dup while in-flight
  is a no-op). Progress is fanned out over `/v1/events` as `asr_download`
  (`{repo, fraction, downloaded, total}`), then `asr_download_done` /
  `asr_download_error` — these events carry **no `session_id`** (daemon-wide).
  Progress is measured by polling the repo's on-disk cache size vs the Hub's
  reported total. To make that poll *meaningful* the download **forces the plain
  HTTP backend** (`constants.HF_HUB_DISABLE_XET = True`, read live by hf_hub): xet
  streams content-addressed chunks into a separate cache and only materializes the
  final blob at the very end, so the size poll would read ~0 % then jump to 100 % —
  the plain backend instead grows a `<blob>.incomplete` file the poll can track.
  `POST .../delete` (`manager.delete`) `rmtree`s the repo's HF-cache dir (catalog-
  validated; 409 while it's downloading); deleting the *active* model just reverts
  it to "active · needs download". `POST /v1/asr/model` persists the active model to
  `~/.capture/config.json` (`core.config`), which the Whisper backend reads (arg →
  `CAPTURE_WHISPER_MODEL` env → config → default) so a GUI choice applies to new
  captures started anywhere. Weights are **downloaded on demand, never bundled**.
- **Permissions (`/v1/permissions`, macOS):** the daemon **only checks** status
  (`core.permissions`): Screen Recording via `Quartz.CGPreflightScreenCaptureAccess` (safe)
  and Microphone via `AVCaptureDevice.authorizationStatusForMediaType` (safe). It does **NOT**
  trigger the Screen Recording prompt — `CGRequestScreenCaptureAccess` requires a window-server
  connection and **aborts the headless daemon** (SIGABRT); so `request("screen_recording")`
  returns status without prompting, and the **GUI** shows that prompt itself (CoreGraphics FFI,
  it's a real app). `request("microphone")` ALSO returns status without prompting —
  `requestAccessForMediaType` likewise aborts a background-only process when it must show the
  dialog. The mic prompt comes from elsewhere: the GUI links to Settings, and macOS prompts
  automatically the first time the ffmpeg mic-fallback opens the device. A new Screen Recording
  grant needs the daemon to
  **restart** (GUI "Restart daemon" → `/v1/admin/shutdown` → the menu-bar agent respawns it);
  a Microphone grant applies immediately. Attribution/persistence for the ad-hoc daemon is the
  #31 TCC caveat. Non-macOS → `not_applicable`.
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
- **Contract: DONE.** pydantic models (`models.py`) validate requests and pin the
  responses; `v1_schema()` + `GET /v1/schema` emit the JSON Schema; the contract
  test golden-checks it (`tests/contract/golden/v1_schema.json`). **[planned]** Rust
  type generation (typify) from the schema for the GUI; per-route schema refs.
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
- `test_daemon` also round-trips live `health`/`windows`/`sessions`/summary responses
  through the pydantic models, asserts a two-target request is rejected 400, and that
  `GET /v1/schema` is served. `tests/contract/run_contracts.py` pins `v1_schema`
  against a golden (`--regen` after an intentional model change).
