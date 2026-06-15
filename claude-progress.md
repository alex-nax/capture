# Progress Log

## Session 27 — 2026-06-15
**Agent**: builder (macOS box 15.7.3, branch **v2**)
**Summary**: Implemented the **audiocap macOS-26 enumeration-retry** (#30 follow-up) — AND in
verifying it, **broke this box's Screen Recording grant by rebuilding the helper** (a real,
valuable finding; needs Alex to re-grant).
- **`helper/audiocap.swift`**: `SCShareableContent` enumeration now uses `enumerateShareableContent()`
  — a bounded retry (5 attempts, 0.5s backoff) instead of `exit(5)` on the first failure, so the
  helper rides through macOS 26's intermittent enumeration flakiness rather than leaning on the
  daemon's respawn. Compile-verified (`swiftc` rc=0).
- **INCIDENT — grant broken on macOS 15**: I then rebuilt + re-signed the production
  `helper/audiocap` with the stable identity (`capture-mcp-codesign`) to deploy/verify on this box.
  On **macOS 15.7.3 the same-identity rebuild LOST the Screen Recording grant** (→ `displays=0` /
  `app-audio-failed (rc=4): no display available`), **contradicting the macOS-26 spike** where the
  same-identity rebuild kept it. So: with a self-signed (no-Team-ID) cert, **macOS 15 effectively
  keys the grant to the cdhash** (every rebuild needs re-approval; maybe compounded by Sequoia's
  periodic re-approval). I cannot restore a TCC grant programmatically — **Alex must re-approve**
  (run `./helper/audiocap --system` from an interactive Terminal → approve in System Settings →
  Screen Recording → quit & reopen Terminal). LESSON: do NOT rebuild the signed helper on a working
  box to "verify"; commit the source and rebuild on the target (macOS 26) where the change is
  testable. The earlier captures THIS session used the pre-rebuild binary (grant was fine then).
- **Refined #30** in product-architecture.md (the identity-keying conclusion is **macOS-version-
  dependent for self-signed certs**; #31 must re-verify Developer-ID grant persistence on macOS 15,
  not assume the macOS-26 result generalizes) + screencapturekit-helper.md (retry + the grant-
  fragility note).
**Verification**: source compiles; Python smoke 68/68 (unaffected). The enumeration-retry itself
could NOT be functionally verified here (the macOS-26 flakiness isn't reproducible on 15, and the
rebuilt helper can't capture until the grant is restored).
**CORRECTION (same session)**: FALSE ALARM — I did NOT break the grant. Alex ran
`./helper/audiocap --system` from his own Terminal → `displays=2`, READY, **audio flowing**. The
rebuilt same-identity helper works fine from a Screen-Recording-granted Terminal on macOS 15.7.3.
The `displays=0` I saw was the **Claude Code shell's execution context** (not a granted GUI app) —
an artifact of where I run commands, NOT a TCC regression. Reverted the overstated
"macOS-15-keys-self-signed-to-cdhash" claim in product-architecture.md ([confirmed #30] stands,
no contradicting evidence) and screencapturekit-helper.md (displays=0 = launching process lacks
the grant). LESSON #2: don't escalate a result from a non-granted execution context into a TCC
finding — verify from the context that actually holds the grant. The enumeration-retry source
change stands (good); it's only functionally testable on the macOS-26 box.

---

## Session 26 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**; v2 was squashed to one commit + pushed to GitHub
this session per Alex's request — origin/v2 = 162222a, current-dated; local tag v2-presquash keeps
the granular history; new commits continue normally on top)
**Summary**: Built **#33 slice 4 — global hotkey ⌃⌘R** (the spec's `global-hotkey` crate),
completing the menu-bar-app trio (tray-icon + muda + global-hotkey).
- **`gui/src/hotkey.rs` (new)**: registers ⌃⌘R via `global-hotkey` 0.8 (Carbon
  RegisterEventHotKey — **no accessibility permission** needed); returns the manager (kept alive in
  the view = stays registered) + the hotkey id.
- **`gui/src/app.rs`**: `GlobalHotKeyEvent::receiver()` drained in the existing 250ms tray loop;
  on key-down → `toggle_capture` (running → stop_all; else start on the selected window). A UI hint
  "⌃⌘R toggles capture from anywhere" renders when registration succeeds.
- **Verification**: `cargo build` clean; the GUI ran and **showed the hotkey hint** (= manager +
  register both succeeded → hotkey is registered with the system) with a live capture listed. The
  actual key-press→toggle path could NOT be auto-verified: a synthetic keystroke (osascript) timed
  out (Terminal lacks Accessibility) and synthetic CGEvents don't reliably trigger Carbon hotkeys —
  needs a real hardware ⌃⌘R (Alex can confirm). Honest status recorded in features.json #33.
- Specs: gui.md (hotkey files/behavior; "start" needs a selected window — frontmost-default would
  need engine z-order; menu-bar icon + LSUIElement still pending).
**#33 status**: slices 1–4 DONE (window + daemon client + picker + start/stop + live session list +
SSE live transcript/preview + menu-bar tray + global hotkey). **Remaining**: a real menu-bar icon +
LSUIElement, onboarding + Settings, RenderImage eviction for the preview, `.app`/DMG
packaging+signing (#31, needs Alex's Developer ID), gpui 0.2.2 → zed git rev for Linux/a11y.
**Next**: the audiocap macOS-26 enumeration-retry (#30 follow-up, Python-side) or GUI onboarding/
Settings. #31 packaging needs Alex's Developer ID.

---

## Session 25 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: Built **#33 slice 3 — menu-bar (tray) presence** for the GPUI app, via the spec's
exact combo **tray-icon 0.24 + muda 0.19** (compiled clean on macOS in ~18s; GTK deps are
Linux-gated).
- **`gui/src/tray.rs` (new)**: a status-item with a title that reflects the running-capture count
  (`● capture` idle, `⦿ N` running) + an Open/Stop-all/Quit menu (string ids).
- **`gui/src/app.rs`**: tray built on the main thread in `CaptureApp::new`; a `cx.spawn`+250ms
  `Timer` loop drains `muda::MenuEvent::receiver()` and keeps the title synced to the running count
  — all tray UI mutation on the GPUI main thread. Menu handlers: Stop-all (off-thread
  `/v1/.../stop` of every running session), Open (`cx.activate`), Quit (`process::exit`).
- **Verified visually** (3 menu-bar screenshots): the title went **`● capture` → `⦿ 1` →
  `● capture`** across a CLI start/stop — live bidirectional sync, and the tray operates the daemon
  independent of the main window.
- Specs: gui.md (tray files/contract/behavior; global-hotkey + real icon + LSUIElement remain);
  features.json #33.
**Verification**: `cargo build` clean (no warnings); manual end-to-end on macOS (screenshots).
Python untouched (68/68 + 4/4 stand).
**#33 status**: slices 1–3 DONE (window + daemon client + picker + start/stop + live session list +
SSE live transcript/preview + **menu-bar presence**). **Remaining**: global hotkey, onboarding +
Settings, RenderImage eviction for the preview, `.app`/DMG packaging+signing (#31), gpui 0.2.2 →
zed git rev for Linux/a11y.
**Next**: global hotkey (global-hotkey crate) for quick start/stop, or the audiocap macOS-26
enumeration-retry (#30 follow-up). #31 packaging needs Alex's Developer ID.

---

## Session 24 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: Built **#33 slice 2 — the GUI live session-detail pane** (transcript streaming +
screenshot preview over `/v1/events` SSE), turning the polled list into a real live view.
- **`gui/src/daemon.rs`**: added `transcript(id, tail)` (REST backfill) and `open_events()` — the
  `/v1/events` SSE line reader (a **no-timeout** ureq agent; the 30s agent would kill the stream).
- **`gui/src/app.rs`**: a background **std::thread** reads SSE forever (reconnect loop) and, for the
  tracked session, appends `transcript_segment` text + the latest `screenshot_taken` path into a
  shared `Arc<Mutex<LiveState>>`. Clicking a session (or auto-selecting the newest running one)
  backfills its transcript via REST then tracks it live. The detail pane renders the latest
  screenshot via `img(PathBuf)` + the last ~12 transcript lines; the 1s poll loop repaints it.
  Two-column lists (windows | sessions) to make room.
- **Verified visually** (two screenshots ~10s apart on the live YouTube capture): the session
  counts grew (15s/6seg → 36s/12seg), the **transcript grew live** (new lines streamed in via SSE),
  and the **screenshot preview rendered the actual video frame**. Exactly the ask.
- Specs: gui.md (SSE behavior, contract, files; moved SSE out of Known-limitations to done; the
  `img()` cache-leak-on-long-runs → RenderImage is the remaining preview note); features.json #33.
**Verification**: `cargo build` clean; manual end-to-end on macOS (screenshots). Python untouched
(68/68 + 4/4 stand).
**#33 status**: slices 1–2 DONE (window + daemon client + picker + start/stop + live session list +
**live transcript/preview via SSE**). **Remaining**: tray/menu-bar + global hotkey, onboarding +
Settings/ASR-model manager, RenderImage-with-eviction for the preview, `.app`/DMG packaging+signing
(#31), gpui 0.2.2 → zed git rev for Linux/a11y.
**Next**: tray/menu-bar presence (tray-icon+muda) or the audiocap macOS-26 enumeration-retry. #31
packaging needs Alex's Developer ID. (Per [[feedback-keep-momentum]]: I'll keep going on the
clear next step rather than asking.)

---

## Session 23 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: Built **#33 slice 1 — the native GPUI GUI app** (`capture-gui`). Alex chose
**crates.io gpui 0.2.2, macOS-first** (fastest to a running window; migrate to a pinned zed git
rev when Linux/a11y is tackled). The backend was ready (#32 daemon /v1 + SSE + contract), so the
GUI is a pure thin client.
- **`gui/` (new Cargo project, gitignored target)**: `daemon.rs` (ureq client mirroring
  client.py — discover ~/.capture/daemon.json, health/sessions/windows/start/stop, surfaces the
  daemon's {"error"} body), `app.rs` (`CaptureApp` GPUI `Render`: health header, /v1/windows
  picker (clickable, capped 7), Start/Stop buttons, live session list polled every 1.5s via
  cx.spawn+Timer with blocking HTTP on the background executor + WeakEntity::update/notify),
  `main.rs` (Application::run, one window). Deps: gpui 0.2.2, ureq, serde, dirs — gpui's first
  compile is heavy but builds clean.
- **Ran + verified visually** (screencapture of the GPUI window): connected to the daemon
  (health shown), window picker populated with real targets, and the **session list showed a LIVE
  running YouTube capture (54 shots / 15 segs, polled) PLUS earlier sessions recovered from the
  disk index** — the daemon-peers shared-registry working through the GUI. Start/Stop fired
  end-to-end (GUI→daemon→engine→per-app audio+ASR).
- Specs: new docs/specs/gui.md + index row; features.json #33 slice-1 annotated.
**Verification**: `cargo build` clean (no warnings); manual end-to-end on macOS (screenshots).
Python smoke/contracts untouched this session (no Python changed) — still 68/68 + 4/4 from
Session 22.
**Observed (note, not blocking)**: on GUI launch a capture auto-started/-stopped once — almost
certainly a stray macOS click-through delivered to the freshly-focused window (cursor over a
button as it opened), not an on_click-on-render bug; worth confirming when wiring real input.
**#33 status**: slice 1 (window + daemon client + picker + start/stop + live session list) DONE.
**Remaining**: SSE /v1/events live preview+transcript (RenderImage), tray/menu-bar + hotkey,
onboarding + Settings, .app/DMG packaging+signing (#31), gpui 0.2.2 -> zed git rev for Linux/a11y.
**Next**: wire /v1/events (SSE) into the GUI for a live transcript/preview pane (credit-free), or
the audiocap macOS-26 enumeration-retry. #31 packaging needs Alex's Developer ID.

---

## Session 22 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: Built the **`/v1` pydantic + JSON-Schema contract** (the GUI "contract firewall") —
the next V2 task. **No new dependency**: pydantic 2.13 is already transitive via `mcp`.
- **`daemon/models.py`**: pydantic models = the `/v1` contract. `StartSessionRequest` (validates
  the POST body — unknown fields/types/exactly-one-target/output_dir, all `extra="forbid"`) +
  response models (`SessionSummary`, `SessionsResponse`, `WindowsResponse`, `TranscriptResponse`,
  `HealthResponse`, `WindowInfo`, `TranscriptSegment`, `ErrorResponse`). `v1_schema()` emits the
  combined JSON Schema.
- **`daemon/server.py`**: `_start_session` now validates via `StartSessionRequest` (replacing the
  hand-rolled field checks; dead `_SESSION_ARGS`/`_present` removed); new `GET /v1/schema` route.
- **Contract test**: `tests/contract/run_contracts.py` gained a `v1_schema` golden
  (`golden/v1_schema.json`, 4/4 contracts). Runtime serves engine dicts (resilient); the *test*
  enforces the models — round-trips live `health`/`windows`/`sessions`/summary responses through
  them, asserts a 2-target request → 400, and `/v1/schema` is served.
- **Registry fix (required by the contract)**: `_recover` now merges recovered records onto a
  full-shaped `_template`, so EVERY `/v1/sessions` entry (live, stopped, interrupted, unknown) has
  one uniform shape and satisfies `SessionSummary` — even from a partial/old session.json.
  session-registry.md updated.
- Specs: daemon.md (models/route/validation/tests + uniform-record note), product-architecture.md
  (contract firewall [current, #32]), session-registry.md.
**Verification**: smoke **68/68** (+3: live responses match the contract, bad request 400,
/v1/schema served); contracts **4/4** (new v1_schema golden). The contract caught the real
recovered-record shape divergence before it could reach the GUI.
**#32 status**: daemon + CLI + MCP daemon-first + SSE events + **/v1 pydantic+JSON-Schema contract**
all DONE. **Remaining for passes:true**: UDS transport, daemon-lifecycle install, Rust typify from
the schema, and cross-terminal AUDIO (needs #31). **Next**: `audiocap` macOS-26 enumeration-retry
(#30 follow-up), UDS transport, or daemon-lifecycle install. #31 still needs Alex's Developer ID.

---

## Session 21 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: Built **#32 — live event stream `GET /v1/events`** (the daemon's EventBus fan-out),
the next V2 task. Zero new deps; reuses the M0b `EventBus` (#26).
- **Transport decision: SSE, not WebSocket.** The event channel is one-way (daemon→client), which
  Server-Sent Events serve straight from the stdlib `ThreadingHTTPServer` with no dependency;
  clients send commands via the REST routes. WS stays [planned] only if bidirectional is ever
  needed. Documented in daemon.md + product-architecture.md.
- **`daemon/server.py`**: `CaptureDaemon` gained an SSE fan-out (`sse_register/unregister/
  broadcast`, bounded per-client queues, slow clients drop rather than block) and `attach_stream`
  — a per-session forwarder thread that subscribes to `session.events` **before** `start()` (so
  `starting`/`running` are carried), tags each event with `session_id`, and ends after the
  terminal state. `_serve_sse` streams `text/event-stream` with `: ping` heartbeats
  (`CAPTURE_SSE_HEARTBEAT_SECONDS`, default 15). `_start_session` now attaches the stream.
- **Client + CLI**: `DaemonClient.events()` generator; `capture watch [SESSION_ID]` streams events
  (optionally filtered), Ctrl-C to stop.
- **Demo earlier this session**: ran the full daemon+CLI stack on the original UE5 motion-matching
  YouTube video (`8iqK-mCcE0Y`) — 79s per-app audio, 11 transcript segments, 41 screenshots, 0
  errors, all via `capture start/status/tail/stop` over `/v1`; matches the 2026-06-07 capture.
- Specs: daemon.md (events route/behavior/heartbeat/tests), product-architecture.md /v1 [current]
  + SSE note; features.json #32 annotated.
**Verification**: smoke **65/65** (+3 `test_sse_events`: SSE client connected pre-start receives
starting→running→stopping→stopped + log_line/screenshot_taken, all session-tagged); contracts
**3/3** (MCP/contract surface unchanged); real `capture watch` on a live daemon captured
{state:4, screenshot_taken:5, log_line:6} for a 6-line launch run.
**#32 status**: daemon + CLI + MCP daemon-first + SSE events all DONE. **Remaining for passes:true**:
pydantic models + JSON-Schema contract, UDS transport, daemon-lifecycle install, cross-terminal
AUDIO (needs #31). **Next**: pydantic + JSON-Schema `/v1` contract (the GUI "contract firewall"),
or the `audiocap` macOS-26 enumeration-retry (#30 follow-up). #31 still needs Alex's Developer ID.

---

## Session 20 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: Built **#32 MCP daemon-first mode** — the credential-free half that finishes the
agent-sharing story. The MCP server now proxies its tools to a running `captured` daemon and
falls back to the embedded engine otherwise.
- **`server.py`**: `_daemon()` returns a live `DaemonClient` when `~/.capture/daemon.json` is
  discoverable + `/v1/health` answers, unless `CAPTURE_MCP_EMBEDDED` is set (forces embedded;
  for headless/CI). Per-call, cheap (~2s probe), so a daemon started/stopped mid-session is
  picked up. All four tools (`capture_start/stop/status`, `list_windows`) gained a daemon branch
  (blocking client call offloaded via `anyio.to_thread`; `DaemonError`→`ValueError` so messages
  match the embedded path). Exactly-one-target validation stays in the tool *before* dispatch, so
  validation errors are backend-independent; `capture_stop`'s "stop the unique running one"
  resolution is replicated against `/v1/sessions` for the daemon path.
- **Net effect**: two terminals' MCP agents both proxy to the one daemon → they share its live
  registry (and, with the signed launchd daemon #31/#30, its TCC grant). An agent-started capture
  is visible to `capture status` and vice-versa.
- **Specs (mandatory)**: daemon.md (daemon-first now DONE), mcp-server.md (new "Daemon-first
  dispatch" behavior + `CAPTURE_MCP_EMBEDDED`/`CAPTURE_DAEMON_JSON` config), product-architecture.md
  (embedded-fallback + server.py marked [current, #32]).
**Verification**: smoke **62/62** (+3: `test_mcp_daemon_first` — MCP `capture_status`/`list_windows`
route to a running daemon and see a daemon-only session; `CAPTURE_MCP_EMBEDDED=1` makes that
session absent again, proving the fallback). Contracts **3/3** (MCP tool schemas unchanged — the
proxying is internal). Sanity: with no daemon, `capture_status()` returns embedded `{sessions:[]}`.
**#32 status**: daemon API + CLI + MCP daemon-first are all DONE. **Remaining for passes:true**:
pydantic models + JSON-Schema contract, UDS + WebSocket `/v1/events`, daemon-lifecycle install,
and the cross-terminal-AUDIO benefit (needs #31's signed daemon). Kept `passes:false`, annotated.
**Next**: pydantic/JSON-Schema contract for `/v1` (sets up the GUI "contract firewall"), the
WebSocket event stream, or the `audiocap` enumeration-retry (#30 follow-up). #31 packaging still
needs Alex's Developer ID cert.

---

## Session 19 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: Built **#32 slice 1 — the `captured` daemon + `capture` CLI**, the credential-free
core of V2 (the daemon-peers architecture validated by spike #30).
- **`capture_mcp/daemon/`** (stdlib-only, no new deps): `CaptureDaemon` = `ThreadingHTTPServer` +
  a shared `SessionRegistry`, bound to `127.0.0.1:<ephemeral>` with a **bearer token**.
  `/v1` routes: `health` (no auth), `windows`, `sessions` (POST start / GET list / GET one /
  POST stop), `sessions/{id}/transcript?tail=N`, `admin/shutdown`. Discovery via
  `~/.capture/daemon.json` (0600, `CAPTURE_DAEMON_JSON` override); single-instance guard.
  `client.py` = stdlib `DaemonClient` (urllib) reused by the CLI and (later) MCP daemon-first.
- **`capture_mcp/cli/`**: `capture` CLI — `daemon start|stop|status`, `status [id]`, `windows`,
  `start`, `stop [id]`, `tail`. `daemon start` spawns `python -m capture_mcp.daemon` detached.
  Console scripts added: `captured`, `capture`.
- Same engine contract as MCP: register-before-start (failed start visible as `error`),
  exactly-one-target, identical session-dir output. No capture logic in the frontends.
- **Specs (mandatory)**: new `docs/specs/daemon.md`; index row; architecture.md module map
  (daemon/ + cli/ as peer frontends); product-architecture.md `/v1` block + layout marked
  `[current, #32 slice 1]`.
**Verification**: smoke **59/59** (+14: in-process API round-trip incl. 401-without-token, a
launch capture through the API with `log_lines==6`, windows/transcript/404; and the CLI spawning
+ driving a real daemon subprocess start→status→windows→status→stop); contracts **3/3** (MCP tool
surface + session layout unchanged).
**#32 status**: slice 1 (daemon API + CLI) done; **remaining for passes:true** — the MCP server's
daemon-first mode + embedded fallback (`CAPTURE_MCP_EMBEDDED=1`), the cross-terminal-audio
criterion (needs #31's packaged signed daemon), pydantic models + JSON-Schema contract, and the
UDS/WebSocket transport. Kept `passes:false` with criteria annotated.
**Next**: MCP daemon-first mode (finishes #32's agent-sharing story, credential-free) and/or the
`audiocap` enumeration-retry (#30 follow-up). #31 packaging still needs Alex's Developer ID cert.

---

## Session 18 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: **Feature #30 (TCC attribution spike) PASSED** — the load-bearing gate for the
daemon-peers architecture is now validated, unblocking #31. Alex ran the `tcc-spike` distro on a
spare Mac (**macOS 26.5.1, arm64**) and returned the results tarball; analyzed and recorded:
- **Attribution works**: a launchd user-agent → signed `CaptureSpike.app` PyInstaller daemon →
  `audiocap` chain streamed audio (`audio_flowing: true`, "READY … audio flowing"), with the
  **daemon (not any terminal)** holding the Screen Recording grant. `launchctl print` confirms it
  ran as `gui/501/com.capturemcp.spike` from the bundle.
- **Grant persists across a same-identity update**: rebuild (new cdhash) + re-sign with the SAME
  identity/bundle-id + restart → `daemon_version 1.0.1`, audio flowed immediately, **respawns=0**,
  no re-prompt.
- **Negative control**: re-signing with a DIFFERENT identity LOST the grant ("the user declined
  TCCs… capture") → the grant **keys to the code-signing identity** → a **stable Developer ID
  (Team ID + bundle id) across updates is mandatory** for the product.
- **macOS 26 caveat**: `SCShareableContent` enumeration is intermittently flaky (audiocap `exit 5`
  interleaved with healthy audio; respawn loop rode through it). **Follow-up logged**: add a bounded
  enumeration retry to `audiocap.swift` so the real helper doesn't lean on a supervisor restart.
- Recorded: `spike/tcc-attribution/results/` (FINDINGS.md + status_*.json + sysinfo + launchctl
  dump); product-architecture.md gate → PASSED + the TCC invariant marked [confirmed #30] + the
  macOS-26 follow-up; features.json #30 → passes:true.
**Also this session (earlier)**: closed the helper-path regression (Session 17 — `test_helper_path`
+ spec), and shipped the spike as a clone-and-run **`tcc-spike` GitHub branch** (prebuilt universal
audiocap + agent-oriented RUNBOOK.md; `03_check.sh` made non-blocking under
`CAPTURE_SPIKE_NONINTERACTIVE=1`).
**Verification**: docs/spec/features only (no engine code touched); smoke 45/45, contracts 3/3 still
hold from Session 17.
**Next**: #31 (M1 packaged signed engine) is now unblocked but needs Alex's **Developer ID cert**
for real notarization. The credential-free, now-validated path is **#32 (daemon /v1 API + CLI)** —
recommended as the next build. The audiocap enumeration-retry is a small standalone fix worth doing
alongside.

---

## Session 17 — 2026-06-15
**Agent**: builder (macOS box, branch **v2**)
**Summary**: Closed the loop on the **helper-path regression** found during a real meeting capture.
Context: while capturing a live Google Meet on v2, per-app audio silently produced `no-audio-source`
(screenshots worked, transcript was empty). Root cause: the M0a split (#25) moved
`platform/macos.py` into `core/platform/`, one level deeper, but `_HELPER` kept `parents[3]` — which
now resolves to `src/helper/audiocap` (nonexistent) instead of `<repo>/helper/audiocap`. The code
fix (`parents[3]→[4]`) was committed mid-meeting (`e4f16e1`); this session adds the **owed test +
spec** so it can't recur:
- **`tests/smoke.py::test_helper_path`** (darwin-only, skips elsewhere): pins `macos._HELPER ==
  <repo>/helper/audiocap`, and when the helper is built asserts `helper_path()` returns it (not
  `None`). **Proven to fail** on the `parents[3]` off-by-one (verified by temporarily reverting:
  43/45 with the bug, 45/45 fixed).
- **`docs/specs/platform-abstraction.md`**: new Invariant documenting the `parents[4]` resolution +
  why (the silent-audio failure mode), and a Tests note for the guard.
- Why smoke missed it originally: the audio test stubs ASR and uses the **mic** source, so the
  macOS per-app helper path was never exercised hermetically. Now it is (path-level).
**Verification**: smoke **45/45** (2 new helper-path checks); contracts **3/3**.
**Branch note**: meeting captures in the interim ran on `main` (where the path + the external
`~/.capture/bin/transcribe_meeting.py` import were already correct); that external helper was made
branch-resilient (try `core.session` except `session`).
**Next (V2 roadmap):** the critical path #31 (packaged signed engine) → #32 (daemon) is gated on
**#30 (TCC attribution spike)**, whose kit is ready (`spike/tcc-attribution/`) and awaits a run on
Alex's spare Mac. The daemon **/v1 API + CLI** code itself does NOT depend on packaging/the spike
and could start in parallel — decision pending.

---

## Session 16 — 2026-06-10
**Agent**: builder (macOS box, ultracode, branch **v2**)
**Summary**: Prepared the **#30 TCC-attribution spike kit** for Alex's spare Mac (the feature
itself stays open until the spike RUNS there — its criteria need the actual report/screenshots).
- **`spike/tcc-attribution/`**: dev-side `make_kit.sh` builds a **universal (arm64+x86_64,
  min macOS 13) audiocap** and tars a self-contained kit (68 KB) → `dist/capture-tcc-spike.tar.gz`.
  Target Mac needs NO Xcode, NO Apple Developer account, no admin: `01_setup.sh` (uv → py3.12 →
  PyInstaller → builds **CaptureSpike.app** via `--windowed --osx-bundle-identifier` — PyInstaller's
  own .app layout is codesign-clean), `02_install.sh` (self-signed stable identity, deep-sign,
  launchd agent), `03_check.sh` (THE test: grant → kickstart → audio_flowing verdict),
  `04_update_sim.sh` (same-identity update; `--rotate-identity` negative control),
  `05_collect.sh` (evidence tarball), `uninstall.sh`. Daemon stub `captured_spike.py` respawns
  audiocap, scans the frozen helper contract (READY / -3801/-3803 / -3805), writes
  `~/CaptureSpike/status.json` with a human-readable `verdict` every 2s.
- **Dry-run on this box caught two real kit bugs** before they hit the spare Mac:
  (1) codesign rejects a symlinked CFBundleExecutable → switched to PyInstaller-built .app;
  (2) a stray `version.txt` in Contents/MacOS breaks bundle sealing ("code object is not signed")
  → version now ships via `--add-data`/`_MEIPASS`. Final kit verified here end-to-end short of
  launchd persistence (auto-mode policy correctly blocked installing an agent on the dev box):
  bundle deep-signs + verifies strict; foreground daemon run → READY scanned, **307 KB PCM in
  10 s, verdict "AUDIO FLOWING"** (this box has a grant; the spare Mac is the real test).
- product-architecture.md #30 item now points at the kit.
**Verification**: all six kit scripts `bash -n` clean; full 01→build→sign→run chain exercised
with the final artifacts; smoke/contracts untouched (35-43/43 + 3/3 from Session 15 still stand).
**Next**: Alex runs the kit on the spare Mac (runbook: spike/tcc-attribution/README.md), brings
back `tcc-spike-results-*.tar.gz`; then #30 gets its verdict written into product-architecture.md
and #31 (packaged signed engine) is unblocked — or redirected if the result is negative.

---

## Session 15 — 2026-06-10
**Agent**: builder (macOS box, ultracode, branch **v2**)
**Summary**: Built **feature #29 (`list_windows` MCP tool)** — agents now have the same window
picker the daemon (`/v1/windows`) and GPUI GUI will use.
- **`core.list_windows(pid=None, app_name=None)` (new)**: JSON-ready dicts (window_id, pid,
  app_name, title, width, height) from `platform.current().window_finder.find()`, largest-area
  first; lives in core so MCP/daemon/CLI/GUI all wrap the identical function.
- **`list_windows` MCP tool**: optional app_name (case-insensitive substring) / pid filters,
  offloaded via anyio.to_thread; returns `{"windows": [...], "count": n}`; empty result is not
  an error. Note: without the Screen Recording grant, macOS window titles may be empty strings
  (fields stable, contents permission-dependent).
- **Contract workflow exercised for real**: the tools/list golden correctly FAILED on the new
  tool (2/3), spec updated first (mcp-server.md: four tools + new section), then `--regen` →
  3/3. This is the intended sequence for every future tool-surface change.
**Verification**: smoke **43/43** (4 new: shape+count, entry fields, largest-first ordering on 7
real windows, app_name filter — 'Google Chrome' → 2); contracts 3/3 after regen.
**Known issues / next**: Windows-side verification of the tool pends the Windows box (same
WindowFinder seam, expected to just work). **Next**: #30 (TCC attribution spike — NEEDS A CLEAN
macOS 14/15 VM from Alex; gates #31 packaging and the daemon milestones), or jump to #32 daemon
groundwork that doesn't depend on the spike.

---

## Session 14 — 2026-06-10
**Agent**: builder (macOS box, ultracode, branch **v2**)
**Summary**: Built **feature #28 (openai-compat remote ASR backend + `minimal` extra)**.
- **`core/asr/openai_compat.py` (new, stdlib-only — urllib + wave, zero new deps)**: POSTs each
  float32 chunk as an in-memory 16-bit WAV (multipart/form-data, `response_format=verbose_json`,
  optional model/language fields, optional Bearer auth) to any OpenAI-compatible
  `/v1/audio/transcriptions` endpoint. Maps `segments` → per-segment `Segment`s (blank text
  skipped, start/end clamped to the chunk); plain `text` → one full-chunk segment; HTTP errors
  raise with the body's first 500 bytes (AudioCapture counts them as asr_errors and continues).
  Env: `CAPTURE_OPENAI_ASR_URL` (required) / `_MODEL` / `_KEY` / `_LANGUAGE` / `_TIMEOUT`.
  The Nemotron WSL2/Docker lab, whisper.cpp server, faster-whisper-server, or api.openai.com are
  now just configured endpoints.
- **Factory**: names `openai`/`openai-compat`/`openai_compat`; `auto` chain is now local →
  openai-compat (only if URL env set) → Riva/Nemotron. Local stays preferred; force remote with
  an explicit name.
- **`minimal` extra (pyproject)**: named empty extra documenting/enabling the screenshots+logs-only
  install; remote transcription still works from it because the new backend is stdlib-only.
- Specs: asr.md (backend contract, env, auto chain, names), specs README ASR row,
  product-architecture #28 → done.
**Verification**: smoke **39/39** (4 new: direct backend WAV/model/Bearer verified server-side
against a hermetic stub HTTP server, blank-segment skipping; full AudioCapture pipeline with
`asr_backend="openai"` → 6 timestamped segments at offsets 0.5/2.0/8.5/10.0/16.5/18.0);
contracts 3/3; **fresh-venv minimal install verified** (uv venv → `.[minimal]` → no
mlx/faster-whisper/riva present → real capture: 3 screenshots, logs, events.jsonl). Note: first
`screencapture` from a brand-new venv binary can take >1s (cold TCC consult) — harmless, but
worth remembering when writing time-sensitive tests.
**Next**: #29 (list_windows MCP tool — last cheap pre-daemon win), then #30 (TCC spike, needs a
clean macOS VM from Alex) before #31 packaging.

---

## Session 13 — 2026-06-10
**Agent**: builder (macOS box, ultracode, branch **v2**)
**Summary**: Built **feature #27 (M0c — contract fixtures + frozen helper contract)**. The
frozen interfaces now have a regression gate before any daemon/GUI layering starts.
- **`tests/contract/run_contracts.py` + `golden/`** (standalone, no pytest): pins
  (1) **tools/list** — tool names + input schemas, descriptions stripped so doc edits aren't
  contract breaks; (2) **session-dir layout** — file set, session.json key structure,
  events.jsonl event keys + state sequence + final-line type (OS-neutral: key names only, no
  timestamps/paths/counts); (3) **PCM chunk math** — 20s @ 8s windows → exactly 3 segments,
  start_offsets [0.5, 8.5, 16.5], end_offsets [7.5, 15.5, 19.5], 640000 raw bytes. Drift →
  exit 1 with a mini-diff and a pointer to `--regen` (which requires the matching spec update).
- **`docs/specs/helper-contract.md` (new, FROZEN)**: the process-boundary protocol all audio
  helpers speak — argv; PCM-only stdout (16kHz mono s16le, unbuffered); stderr `READY ` line
  **scanned, not line 1** (diagnostics precede it); exit codes 0/1/2 (+3/4/5 macOS startup);
  -3801/-3803 fatal vs -3805 reconnect-with-backoff taxonomy; Windows reopen-on-error analogue.
  The planned native Windows per-process helper (#34) must be a drop-in behind this file.
- **Drift fixed while freezing**: `audiocap.swift` header comment claimed "first line is READY"
  (false — content/target diagnostics come first) → rewritten; **`audiocap_win.py` shutdown
  referenced a nonexistent `state` dict → NameError on SIGTERM/SIGINT** → fixed to close the
  actual stream; its docstring claimed a stall watchdog that doesn't exist → docstring now says
  `--stall-timeout` is reserved/unused (open item).
- Docs wired: specs README index row; screencapturekit-helper.md points at the frozen contract;
  mcp-server.md Tests + AGENTS.md + capture-continue skill mention the contract runner;
  product-architecture.md M0c → done.
**Verification**: smoke **35/35**; contracts **3/3 hold**; injected golden drift → exit 1 (then
restored); `audiocap_win.py` py_compile clean; `audiocap.swift` compiles to a temp path (the
stably-signed `helper/audiocap` binary was NOT touched — TCC grant intact).
**Known issues / next**: helper protocol verification is still manual (folds into #31 `capture
doctor`); per-OS golden variance unproven until the Windows box runs the suite. **Next**: #28
(openai_compat ASR + minimal extra), #29 (list_windows tool), or #30 (TCC spike, needs clean VM).

---

## Session 12 — 2026-06-10
**Agent**: builder (macOS box, ultracode, branch **v2**)
**Summary**: Built **feature #26 (M0b — EventBus + per-session events.jsonl)**, completing M0
of the product-architecture roadmap.
- **`core/events.py` (new)**: `EventBus` — in-process fan-out, `publish()` never raises/never
  blocks, bounded per-subscriber queues (1000; overflow drops are counted on
  `Subscription.dropped`, the capture loop is never stalled by an observer).
  `EventsFileWriter` — tails the bus into `<session>/events.jsonl`: every `state` event +
  periodic counter snapshots (`CAPTURE_EVENTS_SNAPSHOT_SECONDS`, default 5.0) + one final
  snapshot always last; high-volume types (log_line/screenshot_taken/transcript_segment) stay
  on the bus only — never duplicated on disk (output.log/screenshots//transcript.jsonl have them).
- **Wiring**: components got an optional `emit=None` hook (frontend-ignorant, zero-overhead when
  unset): Screenshotter → `screenshot_taken`/`screenshot_error`; ProcessCapture → `log_line`
  per merged line; AudioCapture → `transcript_segment` + `audio_status` (start/no-data/stop).
  `CaptureSession.events` is public; state events published at every transition; writer started
  before the `"starting"` event so the file records the full lifecycle, drained+finalized on
  stop AND on the start-error path.
- Specs in the same change: **new events.md**; session.md/screenshots.md/process-logs.md/
  audio.md event-hook sections; architecture.md module map; product-architecture.md M0b →
  [current]; specs README index row.
**Verification**: smoke **35/35** (7 new: events.jsonl state order starting→running→stopping→
stopped, periodic+final snapshots with final counters matching the final summary; live bus
subscriber gets state + exactly 6 log_line with both stream tags + screenshot_taken, 0 drops).
**Known issues / next**: no replay for late bus subscribers (daemon M2 needs a small ring
buffer); `audio_status` emitted at 3 fixed points, not every mutation; `dropped` not surfaced in
summary(). **Next**: #27 (contract fixtures + helper-contract.md), #28/#29 (cheap wins), or #30
(TCC spike — gates packaging; needs a clean macOS VM).

---

## Session 11 — 2026-06-10
**Agent**: builder (macOS box, ultracode, branch **v2**)
**Summary**: Built **feature #25 (M0a — engine/MCP package split + SessionRegistry + start()
lock fix)**, the first roadmap milestone from docs/specs/product-architecture.md.
- **Package split**: engine modules moved verbatim (`git mv`) to `src/capture_mcp/core/`
  (session/screenshots/audio/proc/util/windows + platform/ + asr/); `server.py` stays put
  (console script + `.mcp.json` entries unchanged) and is now a thin frontend. All intra-engine
  imports were already relative, so the move was clean. New rule in docs/architecture.md:
  **core/ imports no frontend code**.
- **`core/registry.py` (new)**: `SessionRegistry` — bounded live tracking (same never-evict-live
  prune semantics, extracted from server.py) + **disk-backed history**: append-only
  `~/.capture/sessions.jsonl` index (override `CAPTURE_SESSION_INDEX`; smoke points it at a
  temp file), rebuilt at construction by re-reading each `session.json`. Recovered states:
  finished kept as-is; live-at-crash → `interrupted`; missing session.json → `unknown`;
  corrupt index lines tolerated. `capture_status` now lists recovered sessions;
  `capture_stop` on a recovered id returns its record (idempotent-stop semantics).
- **start() lock fix**: new `"starting"` state; component startup (subprocess, ASR load) runs
  OUTSIDE `session._lock`, mirroring stop(); session.json now also written at `starting` (what
  makes crash→`interrupted` recovery work). Server registers sessions **pre-start**, so status
  shows `starting` and failed starts stay visible as `error` instead of vanishing.
- Scripts/skill imports updated (`capture_youtube_playlist`, `transcribe_audio`,
  `run_interactive.ps1`, skill `set_model.py`); specs updated in the same change:
  **new session-registry.md**, mcp-server.md + session.md rewritten where behavior changed,
  all docs' module paths → `core/`, architecture.md module map, product-architecture.md M0a
  flipped to [current].
**Verification**: smoke **28/28** (20 baseline + 8 new: status-visible-during-slow-start,
status-not-blocked, registry rebuild/interrupted/unknown/ordering); cross-process restart
verified (proc1 captures+stops → proc2 fresh server sees it, status+stop work);
`mcp.list_tools()` → same 3 tools, `output_dir` still the only required param.
**Known issues / next**: stop() during `"starting"` is a documented no-op (auto-stop flag is an
open item for M2); index file grows unbounded (compaction folded into M2). **Next**: #26 (M0b
EventBus + events.jsonl), #27 (contract fixtures + helper-contract.md), or #30 (TCC spike —
gates all packaging).

---

## Session 10 — 2026-06-10
**Agent**: designer (macOS box, ultracode)
**Summary**: Decided the product direction for taking capture-mcp beyond agent-only use (easy
install, GUI, multi-OS) via a 12-agent design study (4 codebase readers + GPUI web research +
3 independent proposals + 3-lens judge panel + completeness critic). Owner constraints fixed
during the session: **native GUI only — no web UI/webview ever; the GUI is GPUI (Zed's Rust
framework); MCP stays first-class**.
- **Decision — daemon-peers architecture**: extract the engine into a signed `captured` daemon
  with a versioned local `/v1` HTTP+WS API (UDS+token; 127.0.0.1 on Windows); GPUI app, MCP
  server, and a new CLI are thin peer clients. Won 2-of-3 judge lenses. Key wins: sessions
  survive client restarts (GUI quit ≠ dead meeting capture), and the daemon becomes the
  TCC-responsible process so **one** Screen Recording grant covers every terminal's agent +
  GUI + cron — dissolving the worst documented pain in permissions-and-signing.md. Rejected:
  GUI-owned Python sidecar (kills live captures with the GUI; defers the TCC fix), full Rust
  engine port (~2× premium, parity risk on PrintWindow/DPI-ladder/WASAPI-reconnect; kept as a
  contract-preserving later option behind /v1).
- **Spec**: new `docs/specs/product-architecture.md` (decision record + plan, [current] vs
  [planned] marked) + index row. Captures the critic's load-bearing findings: TCC csreq pins
  Team ID + bundle id (cert renewal safe, bundle-id churn not); macOS 15 periodic re-approval
  breaks "grant once forever"; Azure Trusted Signing unavailable to individuals (v1 Windows
  ships with SmartScreen warnings); Wayland portal can't target windows by name (app_name
  degrades); no machine-wide session index exists today (GUI history needs a capture root);
  Windows per-process loopback must be a native helper with PROCESS_TREE mode, not Python
  ctypes async-COM.
- **features.json**: seeded #25–#35 — M0 split (registry/EventBus/events.jsonl/lock fix #25–26),
  contract fixtures + frozen helper-contract.md (#27), asr/openai_compat.py + minimal extra
  (#28), list_windows MCP tool (#29), **clean-VM TCC attribution spike that gates the daemon
  bet (#30)**, M1 packaged signed engine via brew (#31), M2 daemon+CLI (#32), M3 GPUI macOS
  app (#33), M4 Windows + native per-process-loopback helper (#34), M5 Linux (#35).
**Verification**: design-only session — no engine code touched; smoke not rerun. Full study
artifacts (3 proposals, 3 verdicts, 33-finding critique) in /tmp/wf_design/ (ephemeral; the
spec + features.json carry everything durable).
**Next suggested task**: #25 (M0a package split + SessionRegistry — pure refactor, agents see
zero change), then #30 (TCC spike) before any packaging work; #28/#29 are cheap independent wins.

---

## Session 9 — 2026-06-08
**Agent**: builder (macOS box)
**Summary**: Used capture live to transcribe a Google Meet standup (per-app audio via
ScreenCaptureKit → mlx-whisper), then hardened the **distributable skill** and fixed the
**code-signing path** that was silently broken on macOS + OpenSSL 3.
- **`scripts/setup_codesign.sh` (feature #15)** — was failing with `SecKeychainItemImport: MAC
  verification failed`. Two bugs fixed: (1) OpenSSL 3.x exports a PKCS#12 with a SHA-256/AES MAC
  that `security import` can't read → now uses **`-legacy`** (3DES/RC2 + SHA-1) **plus a non-empty
  throwaway passphrase** (empty-password p12 also fails MAC verification); (2) `have_identity()`
  used `find-identity -v` (valid/trusted only), but a self-signed cert is untrusted
  (`CSSMERR_TP_NOT_TRUSTED`) so it never lists under `-v` — the post-import check always reported
  failure. Now greps `find-identity -p codesigning` (no `-v`). Re-signed `helper/audiocap` with the
  stable identity (`Authority=capture-mcp-codesign`, no longer adhoc); `audiocap --system` → READY.
- **Skill (`skills/capture/`, feature #24)** — `install.sh` now runs `setup_codesign.sh` (stable
  sign) instead of an ad-hoc `build_helper.sh`, so skill installs get a **persistent** Screen
  Recording grant. Added **`install.ps1`** (Windows parallel of install.sh: find Python → venv →
  `.[whisper]` → smoke → print bin/py). SKILL.md + skills/README.md updated: macOS + Windows are
  both supported (Windows = GDI+/EnumWindows screenshots+logs, mic-fallback audio); dropped the
  stale "Windows in progress" note.
- Specs updated in the same change (mandatory): `docs/specs/permissions-and-signing.md` documents
  the `-legacy`/passphrase requirement and the non-`-v` detection.
**Verification**: smoke **20/20**; `codesign -dvvv helper/audiocap` shows the stable Authority;
helper `--system` run prints `READY ... audio flowing` (grant works). `install.sh`/`install.ps1`
parse-check clean (pwsh unavailable on this mac → PS validated by mirroring init.ps1).
**Note**: meeting-capture helpers + results now live under `~/.capture/` (config.env + bin/ + runs/),
deliberately **outside** the repo. The macOS main-repo helper is now stably signed on this box.
**Next suggested task**: per-process Windows audio (#21), then Whisper-vs-Nemotron benchmark (#23).

---

## Session 8 — 2026-06-07
**Agent**: builder (Windows/NVIDIA box, ultracode)
**Summary**: Built the **live browser-capture → local-ASR pipeline** end to end and ran it on an
8-video YouTube playlist (UE5 C++ Thread-Safe Motion Matching). Net-new this session:
- **faster-whisper large-v3 on CUDA** (native Windows): `whisper_local.FasterWhisper` now auto-detects
  device/compute (`CAPTURE_WHISPER_DEVICE`/`_COMPUTE`), adds the cuBLAS/cuDNN pip DLL dirs to the
  search path so CTranslate2 loads on Windows, and falls back to CPU on a CUDA error.
- **Windows audio (#21 audio half)**: `helper/audiocap_win.py` — WASAPI **system loopback** →
  16 kHz mono s16le on stdout, with **auto-reconnect** on stream error / default-device change (the
  device-change mid-run is what truncated the first attempt at 18 min). Wired into `Win32AudioSource`
  (`mode="loopback"`); helper launched with `CREATE_NO_WINDOW`.
- **DPI-aware screenshots**: `Win32ScreenGrabber` sets per-monitor DPI awareness so whole-screen
  capture isn't cropped on a scaled display; window-targeted `PrintWindow` (+ Chrome `--disable-gpu`)
  gives **occlusion-proof** capture (work with the video in the background).
- **Capture tooling** (`scripts/`): `capture_youtube_playlist.py` (Selenium **attaches** to a
  remote-debug Chrome — avoids YouTube's automation throttle that cut a fresh automated Chrome off at
  ~42 s; mutes/skips ads; one continuous CaptureSession), `transcribe_audio.py` (authoritative offline
  re-transcribe), `playlist_deliverables.py` (per-video split). `run_interactive.ps1` gained `-NoWait`.
- Docs: `docs/asr-benchmark.md` (faster-whisper-vs-Nemotron + the **Docker/WSL2 local-Nemotron** path
  for #23) and `docs/youtube-capture.md`. Deps added to `pyproject.toml` extras.
**Result**: full playlist captured — 51.3 min audio, 582 screenshots, **0 errors**; the 5 narrated
videos transcribed (large-v3 CUDA); videos 6–8 are music/demo with no narration (**verified** against
their source audio via yt-dlp). Deliverables in `capture-runs/playlist2/deliverables/` (gitignored).
**Key lessons**: NeMo/Nemotron is Linux-only → local Nemotron needs WSL2/Docker (documented for #23);
fresh automated Chrome is throttled by YouTube → attach to a real Chrome; capture must run in the
interactive desktop (`WinSta0`); WASAPI loopback can lag wall-clock on long runs → offline re-transcribe
for clean timestamps.
**Known issues / next**: Windows audio is **system loopback, not per-process** (mute other audio for a
clean transcript; true per-process WASAPI loopback is the remaining #21 refinement). Then **#23**:
stand up local Nemotron (Docker/WSL2) and benchmark vs faster-whisper.
**Next suggested task**: per-process Windows audio (#21), then the Whisper-vs-Nemotron benchmark (#23).

---

## Session 7 — 2026-06-07
**Agent**: builder (Windows/NVIDIA box, ultracode)
**Summary**: First run on the **Windows PC** (RTX 4070 Ti SUPER, 16 GB, driver 591.86). The box
had **no Python** — installed 3.12.10 user-scope via winget. Built **feature #20 (platform
abstraction)** and **#22 (Windows bootstrap)**, plus the screenshot/window-discovery half of **#21**.
- **`src/capture_mcp/platform/`**: `base.py` (interfaces `WindowFinder`/`ScreenGrabber`/`AudioSource`
  + `WindowRef` + `fit_box` + `Platform`), `__init__.py` (`current()` factory by `sys.platform`,
  `CAPTURE_PLATFORM` override, cached), `macos.py` (wraps today's `screencapture`/`sips`/Quartz/
  helper/ffmpeg **unchanged** — delegates to the existing `windows.py` Quartz module), `windows.py`
  (zero-dep **GDI+** screenshots: `BitBlt`/`PrintWindow` → scale + encode png/jpg/jpeg/tiff/gif/bmp
  with JPEG quality; **`EnumWindows`** discovery; ffmpeg-dshow mic stub).
- Routed `screenshots.py`/`audio.py`/`session.py` through `platform.current()`; `screenshots.py`
  keeps scheduling/`_last_wid`/count-errors and delegates pixel capture. `proc.py`+`util.py`:
  `split_command` (Windows `CommandLineToArgvW`, POSIX `shlex`) fixes backslash-path launch.
- `pyproject.toml`: gated pyobjc/mlx by `sys_platform == "darwin"` so the base package installs on
  Windows. `tests/smoke.py` made cross-platform (`tempfile` + `sys.executable` commands, no `/tmp`/
  `bash`/`cat`). New `init.ps1` (venv + editable install + smoke).
- **All specs updated** in the same change (mandatory): platform-abstraction.md flipped PLANNED→current,
  plus screenshots/windows/audio/session/process-logs + architecture.md + README.
**Verification**: `init.ps1` → **smoke 20/20 on Windows** through the abstraction (GDI+ whole-screen
capture at `640x480/jpg`, audio chunking, launch logs). Live: factory returns `windows`;
`CAPTURE_PLATFORM=macos` override returns the macOS backend; per-window GDI+ path captured the desktop
HWND to a correct **1024×768 PNG**; window/screen scale+JPEG paths produce valid files. Ran an
adversarial multi-agent review (4 lenses → refute-by-default verify): **7 confirmed / 9 refuted**
(the 9 were spec-drift false positives — verifiers confirmed the specs were already updated). Fixed
the 4 real new-code defects: deselect HBITMAP before `GdipCreateBitmapFromHBITMAP`; lock the encoder
cache; `split_command("")`→`[]`; no silent full-res fallback when scaling fails. Re-verified after.
**Real-window verification (interactive desktop):** the agent shell runs in a non-interactive
*service* window station (`Service-0x0-…`, blank 1024×768 desktop, 0 visible windows), so real
windows aren't reachable from it directly. Added **`scripts/run_interactive.ps1`** (runs a command in
the logged-on user's `WinSta0` session via a transient Interactive-logon scheduled task) and used it
to verify the real path end-to-end: on the actual 1536×864 desktop, `EnumWindows` found Chrome/
Terminal/Notepad, `primary(app_name="notepad")` resolved the Notepad window, and the GDI+ grabber
captured **real Notepad content at 1152×594** plus the full 1536×864 desktop (244 KB). So Windows
screenshots + window discovery (the #21 screenshot half) are verified against real windows.
**Known issues / env**:
- Per-app audio on Windows (WASAPI process loopback) is **not implemented** (#21 audio half) — Windows
  `AudioSource` returns no per-app source; mic needs ffmpeg + `CAPTURE_DSHOW_AUDIO`.
- Pre-existing latent bug (NOT this change; byte-identical in HEAD): `session._start_audio` ASR-unavailable
  note never fires (`status.startswith("asr-unavailable")` vs the `"running (asr-unavailable: …)"` prefix).
- `windows.primary_window` is now unused (macOS finder uses `find_windows`); kept as documented helper.
**Next suggested task**: Feature **#21** per-app **WASAPI process loopback** for Windows audio (emit the
same 16 kHz mono s16le contract), then **#23** Whisper(CUDA)-vs-Nemotron benchmark on captured audio.

---

## Session 6 — 2026-06-07 (branch: feat/distributable-skill)
**Agent**: builder
**Summary**: Authored a **redistributable** skill `skills/capture/` (separate from the
dev-only `.claude/skills/`) so anyone can load one skill and: (1) install capture-mcp + deps if
missing (`scripts/install.sh` — clone → venv → ASR backend → build helper, prints bin/py paths);
(2) create/merge a project `.mcp.json` (`scripts/configure_mcp.py`, preserves other servers);
(3) run quick actions — capture a browser video, launch & capture a process, change/prefetch the
ASR model (`scripts/set_model.py`), edit per-project config (`references/quick-actions.md`).
Validated with skill-creator `quick_validate`; packaged with `package_skill.py` (→ `.skill`
bundle). Installer tested end-to-end against a local clone: fresh venv exposed all 3 MCP tools and
built the helper. Feature #24 added (passes:true). `skills/README.md` documents loading/packaging.
Renamed the skill `capture-mcp-setup` → **`capture`** (it operates, not just sets up). Added
**end-user bug reporting**: `scripts/report_issue.py` collects safe diagnostics (version, OS/arch,
the session's `audio_status`/errors; **secrets/env values redacted** — only MCP server names),
previews by default, and posts a tracked issue to `github.com/alex-nax/capture` only with
`--create` + user consent (gh, or a prefilled URL fallback). Plus `.github/ISSUE_TEMPLATE/bug_report.md`.
Verified preview output does NOT leak a planted `CAPTURE_RIVA_API_KEY`.
**Status**: PR #1 (`feat/distributable-skill` → main) **MERGED** (c44d8f6).
**Next suggested task**: the Windows platform work (#20→#21→#23).

---

## Session 5 — 2026-06-07
**Agent**: builder
**Summary**: Made the harness portable to other machines. Installed **skill-creator** at
`~/.claude/skills/skill-creator` and authored three repo-local skills under `.claude/skills/`
(travel with the code): **capture-continue** (per-session dev loop), **capture-audit**
(health/spec-drift), **capture-spec** (mandatory-spec authoring). All pass skill-creator's
`quick_validate`. Documented the next chapter spec-first: `docs/specs/platform-abstraction.md`
(PLANNED) for Windows/NVIDIA support + the Whisper-vs-Nemotron benchmark, and added features
#20 (platform abstraction), #21 (Windows backends), #22 (Windows bootstrap/init.ps1), #23
(Whisper vs Nemotron-3.5 benchmark, closes #13 when done). AGENTS.md lists the bundled skills.
**Context for next machine:** follow-up runs on a **Windows PC with an NVIDIA card**; today's
code is macOS-only — start with feature #20. To continue: open the repo and run `capture-continue`.
**Next suggested task**: Feature #20 — platform abstraction layer (then #21 Windows backends).

---

## Session 4 — 2026-06-07
**Agent**: builder
**Summary**: Made documentation a first-class, mandatory step. Wrote a per-scope spec for
**every** scope under `docs/specs/` (mcp-server, session, screenshots, process-logs, audio,
asr, windows, screencapturekit-helper, permissions-and-signing) — each with a consistent
section template (Purpose/Files/Public contract/Behavior/Invariants/Failure modes/Outputs/
Configuration/Open items/Tests) and a live open-items backlog — plus `docs/specs/README.md`
as the index + template. Added a **"SPECS ARE MANDATORY"** rule to `AGENTS.md` and a pointer
in `docs/architecture.md`: update the matching spec in the SAME change as any behavior change
(spec = intent, code = reality, they must agree).
**Next suggested task**: work the open-items backlogs in the specs, or Feature #15
(verify stable-cert grant persistence on a clean machine).

---

## Session 3 — 2026-06-07
**Agent**: builder
**Summary**: Cracked the per-app audio `-3805` problem and proved the full audio→ASR
path end to end. `-3805` (`failedApplicationConnectionInterrupted`) is a *transient*
connection interruption, NOT a permission denial — `SCShareableContent` enumerates fine
and the next attempt succeeds. Added **auto-reconnect** to the helper (rebuild stream +
backoff on `-3805`; genuine `-3801`/`-3803` permission errors are reported, not retried).
After that, a per-app capture of the Chrome video produced **1.74 MB of audio** and a real
timestamped Whisper transcript (`capture-motion-match_1/transcript.md`).
**Also**: cross-Space window discovery (Session 2) confirmed; `scripts/setup_codesign.sh`
creates a stable self-signed signing identity so the Screen Recording grant persists across
rebuilds (portable to other machines). README `-3805` section rewritten.
**Gotcha:** a transcription attempt failed because `CAPTURE_WHISPER_MODEL=mlx-community/whisper-base`
does not exist on HF (401) — use a valid repo (`mlx-community/whisper-tiny`, or the default
`whisper-large-v3-turbo`).
**Next suggested task**: Feature #15 — verify the stable-cert grant persists across a rebuild
on a clean machine (needs the one-time Screen Recording approval click).

---

## Session 2 — 2026-06-07
**Agent**: builder
**Summary**: Initialized the harness (AGENTS.md, features.json, claude-progress.md,
init.sh, docs/architecture.md; git init + first commit) and ran "test case 1":
captured the YouTube video *UE5 C++ MotionMatching Performance Test* in Chrome via
the tool and organized it into `./capture-motion-match_1/` (README summary, transcribed
`AnimInstanceBase.cpp`, 5 key frames, capture-session.json); deleted the raw /tmp captures.
**Bug fixed**: Screenshotter fell back to whole-screen (capturing the wrong/foreground
window) when the target's window left the current Space — e.g. a video player going
fullscreen. Now caches the last-known CGWindowID (`_last_wid`) and keeps targeting it
(`screencapture -l` grabs it regardless of Space/focus).
**Known issues**: per-app audio still hits SCStreamError -3805 here (ad-hoc rebuild
drops the TCC grant) — feature #15. The capture summary is therefore vision-only.
**Next suggested task**: Feature #15 — stable-signed helper + verified per-app audio.

---

## Session 1 — 2026-06-07
**Agent**: initializer / builder
**Summary**: Built capture-mcp from scratch and hardened it through two adversarial
multi-agent review rounds. The MCP server captures a target process's window
(timestamped screenshots, configurable format/resolution), stdout/stderr (timestamped
logs), and per-app audio transcribed by a pluggable ASR backend, all on demand via
`capture_start` / `capture_stop` / `capture_status`.

**Features completed (verified)**: #1–#12 (see features.json).
- MCP server + 3 async tools; launch-mode logs; Quartz window discovery; grid-scheduled
  screenshots with configurable format/resolution/jpeg-quality; audio chunking→ASR with
  first-byte-anchored absolute timestamps; local Whisper ASR (mlx verified on real speech);
  session orchestration with graceful degradation; bounded registry; smoke suite (20/20).
- Swift ScreenCaptureKit helper (#9) builds, enumerates content, reaches `startCapture`,
  prints `READY`, and produced real PCM (a review subagent captured 120320 bytes via
  `--system`); clean SIGTERM/SIGINT exit.

**Review**: First round found/fixed 33 confirmed issues (lifecycle leaks, event-loop
blocking, audio threading, swift converter/EPIPE/stream-retention, etc.). Second round
verified the fixes and surfaced 16 more; applied the HIGH (asr-unavailable status clobber)
plus the meaningful medium/low items.

**Known issues / environment**:
- Per-app audio intermittently fails with SCStreamError **-3805** in this environment:
  each ad-hoc rebuild changes the binary's cdhash and drops the Screen Recording TCC grant.
  Mitigation = build with a stable `CODESIGN_IDENTITY` (feature #15). Capture degrades
  gracefully (screenshots + logs continue; failure shown in `audio_status`).
- System `python3` is x86_64 (Rosetta); the project venv is uv-managed **arm64** so
  mlx-whisper installs. faster-whisper modern wheels did not resolve on x86_64.
- ASR is fixed-window/offline, not streaming — boundary words can split (#16).
- Riva/Nemotron adapter (#13) and mic fallback (#14) are coded but unverified live.

**Next suggested task**: Feature #15 — codesign the helper with a stable identity and
verify per-app audio end-to-end against an app that is actively playing audio.

---
