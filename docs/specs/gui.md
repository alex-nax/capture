# Spec: GUI app (`capture-gui`, GPUI)

_Status: current as of 2026-06-15. Source of truth = the code; update this spec in the same change as the code._

## Purpose

The native desktop app (feature #33, M3) — a **GPUI (Rust)** client of the
`captured` daemon, so a normal human can drive captures without the terminal. Per
the owner's fixed constraints (product-architecture.md): **native GPUI, no web UI
ever**, and the MCP/agent path stays first-class. The GUI is a thin peer of the
daemon (like the MCP server and CLI): all capture logic stays in the Python engine
behind `/v1`; the Rust side is UI + a daemon client only.

**Current (#33 slices 1–8, macOS):** a single-window dashboard — daemon health, a
window picker, start/stop, a live-polled session list, a **live session-detail pane**
(screenshot preview + transcript streamed over `/v1/events` SSE), a menu-bar item +
⌃⌘R hotkey, a skill installer, and a **Whisper model manager** (download/select ASR
models). Packaged as a **self-contained** `Capture.app`/`.dmg` that bundles + auto-spawns
a frozen daemon with on-device mlx ASR.

## Files

- `gui/` — a standalone Cargo project (not part of the Python package; its own
  build, `gui/target` gitignored).
  - `gui/Cargo.toml` — `gpui = "0.2.2"` (crates.io), `ureq` (blocking HTTP),
    `serde`/`serde_json`, `dirs`.
  - `gui/src/daemon.rs` — `Daemon` client (mirrors `daemon/client.py`): `discover()`
    from `~/.capture/daemon.json`, `available()` (health probe),
    `health/sessions/windows/start/stop/transcript`, and `open_events()` (the
    `/v1/events` SSE line reader). Plus `bundled_daemon()` (resolves
    `Contents/Resources/captured/captured` in the packaged app) and `spawn_detached()`
    (launch it in its own process group so captures outlive the GUI).
  - `gui/src/app.rs` — `CaptureApp` GPUI view (`Render`) + the poll loop + handlers +
    the background SSE thread feeding a shared `LiveState` (tracked session's
    transcript + latest screenshot path + ASR download progress) + the tray event loop +
    the Whisper **model manager** panel (`daemon.asr_models/asr_download/asr_set_model`).
  - `gui/src/tray.rs` — macOS menu-bar status item (`tray-icon` 0.24 + `muda` 0.19):
    a title that reflects the running-capture count + an Open/Stop-all/Quit menu.
  - `gui/src/hotkey.rs` — global hotkey ⌃⌘R (`global-hotkey` 0.8, Carbon
    RegisterEventHotKey, no accessibility permission) that toggles capture.
  - `gui/src/skill.rs` — install/update the bundled `capture` skill into a coding
    agent's home (`~/.claude/skills/capture`, `~/.codex/skills/capture`), with a
    content-hash status check (not installed / up to date / update available).
- `packaging/build_macos_dmg.sh` — build a **self-contained** `Capture.app` (ad-hoc
  signed) + a `.dmg` for testing (not notarized). PyInstaller-freezes the daemon into
  `Contents/Resources/captured/` (with the signed `audiocap` helper beside it), and
  bundles the skill in `Contents/Resources/skill`. Signs inside-out so the helper keeps
  its stable `com.local.audiocap` identity (TCC-grant persistence).
  - `gui/src/main.rs` — `Application::new().run(...)`, opens one window.

## Public contract

- The GUI consumes only the daemon `/v1` API ([daemon.md](daemon.md)) — it adds no
  new backend surface: `GET /v1/health`, `GET /v1/sessions`, `GET /v1/windows`,
  `POST /v1/sessions`, `POST /v1/sessions/{id}/stop`,
  `GET /v1/sessions/{id}/transcript` (backfill), `GET /v1/events` (SSE, live), and the
  ASR model manager (`GET /v1/asr/models`, `POST /v1/asr/models/download`,
  `POST /v1/asr/model`).
- No CLI/flags yet; the binary opens the window. Reads the same
  `CAPTURE_DAEMON_JSON` discovery file as the CLI.

## Behavior

- **Startup:** `CaptureApp::new` checks for a live daemon (`discover()` + `available()`);
  if none answers and a **bundled daemon** is present (packaged app), it spawns it
  detached (`bundled_daemon()` + `spawn_detached()`) so the app is self-contained — no
  separate `capture daemon start`. It then does a brief blocking initial load (health +
  sessions + windows) and starts a poll loop.
- **Poll loop:** `cx.spawn` + `Timer::after(1.5s)`; each tick **re-discovers** the daemon
  (so it attaches to the just-spawned bundled one) and fetches health + sessions on the
  **background executor** (blocking ureq off the main thread), updating the view via
  `WeakEntity::update` + `cx.notify()`. Ends when the view drops.
- **Window picker:** the `/v1/windows` list; clicking a row selects it (`selected`).
  "Refresh windows" re-fetches.
- **Start:** "Start capture" POSTs `/v1/sessions` for the selected window
  (`pid`, `audio_source:"app"`, 2 s interval) into `~/.capture/runs`; the daemon's
  shared registry means the new session appears in the list (and to the CLI / any
  MCP agent) on the next poll.
- **Stop:** each running session row has a "Stop" button → `POST .../stop`.
- **Live detail pane:** clicking a session (or auto-selecting the newest running one)
  tracks it: a backfill of its transcript via `GET .../transcript`, then a background
  SSE thread on `/v1/events` (which re-discovers the daemon each reconnect, so it
  attaches to the bundled daemon spawned after launch) appends new `transcript_segment`
  text and updates the latest `screenshot_taken` path into a shared `LiveState`. The pane renders the
  latest screenshot via `img(PathBuf)` and the last ~12 transcript lines; the poll
  loop's `cx.notify()` repaints it ~1×/s.
- **Menu-bar (tray):** a status item built on the main thread inside the GPUI run
  loop (`tray.rs`). Its title tracks the running-capture count (`● capture` idle,
  `⦿ N` while N run), updated from the GPUI tray loop (`cx.spawn` + 250 ms `Timer`)
  so all tray UI mutation stays on the main thread. The same loop drains
  `muda::MenuEvent::receiver()` and handles: **Open** (`cx.activate`), **Stop all
  captures** (off-thread `/v1/.../stop` of every running session), **Quit**
  (`std::process::exit`). Menu actions hit the daemon directly — independent of the
  main window.
- **Global hotkey (⌃⌘R):** registered on the main thread in `new()` (`hotkey::build()`);
  `GlobalHotKeyEvent::receiver()` is drained in the same tray loop. On key-down it
  **toggles**: if any capture is running → Stop all; else → start on the selected
  window (or a "select a window first" hint). Works from anywhere (no main window
  focus needed). The manager is held in the view so it stays registered.
- **Install skill:** the bundled `capture` skill (from `Contents/Resources/skill` in
  the .app, or `<repo>/skills/capture` in a dev build) is copied into a coding agent's
  home (`~/.claude/skills/capture`, `~/.codex/skills/capture`), excluding
  `__pycache__`/`.pyc` (clean replace = install or update). A cached per-agent
  `skill_status` (a content-hash compare of bundled vs installed, refreshed on start
  and after install) drives the button label: `— install` / `✓` / `↑ update` — so
  shipped skill updates are visible. Headless: `--skill-status`, `--install-skill <agent>`.
- **Whisper model manager:** a "Whisper models" panel lists the daemon's catalog
  (`GET /v1/asr/models`, polled). Each row shows the model + size and a status:
  `● active` (downloaded), `● active · needs download` in **amber** when the active
  model (e.g. the default `large-v3-turbo`) isn't fetched yet, `✓ downloaded`, or a
  live `↓ NN%` while downloading. The action is a **Download** button for any
  not-yet-downloaded model (including an un-downloaded active one) and a **Use**
  button for a downloaded-but-inactive model. **Download** POSTs
  `/v1/asr/models/download` (the daemon fetches in the background); progress arrives as
  `asr_download` events on the same SSE stream and is accumulated into
  `LiveState.asr_progress` (repo → fraction) — these events have **no `session_id`**, so
  the SSE thread handles them *before* the session filter that would drop them.
  **Use** POSTs `/v1/asr/model` to set the active model. The runtime lives in the
  daemon (mlx); if a daemon lacks it, `backend_available:false` shows a "runtime
  unavailable" note instead of the list. Weights download on demand (never bundled).
- **Layout:** the single window is one vertically-scrolling column
  (`#root` + `overflow_y_scroll`) — the content (windows/sessions, model manager, live
  detail pane) exceeds the viewport, so it scrolls rather than clipping. The detail
  pane is content-sized (`flex_shrink_0`), not `flex_1` (which would grab the scroll
  container's unbounded main axis).
- All daemon calls run off the main thread (background executor / a dedicated SSE
  thread); failures land in the status line, never crash the UI.

## Invariants & constraints

- **GUI is a thin daemon client** — no capture/ASR logic in Rust; it never imports or
  reimplements the engine. It only *launches* the daemon (the bundled frozen binary, as
  an opaque subprocess); it never links it. In a dev build with no daemon running and no
  bundled binary, it shows "no daemon — run: capture daemon start" and stays usable
  (read-only/empty); the packaged app auto-spawns its bundled daemon instead.
- **No web UI** — pure GPUI native rendering.
- **macOS-first, gpui 0.2.2 from crates.io** for slice 1 (deliberate): 0.2.2 lacks
  the wgpu Linux renderer + AccessKit, which are M5 (Linux) / a11y concerns. Migrate
  to a pinned zed git rev when Linux/accessibility is tackled (product-architecture.md).
- The daemon owns the Screen Recording grant; the GUI inherits working audio via the
  daemon (once the daemon is the packaged signed TCC-responsible process, #31/#30).

## Failure modes & handling

- No daemon / discovery file → header shows the "start the daemon" hint; lists empty.
- A daemon call errors → `unwrap_or_default()` / status-line message; the poll loop
  continues; the UI never blocks (all blocking I/O is off-thread).
- `start`/`stop` failures surface the daemon's `{"error": …}` message in the status line.

## Outputs / artifacts

- None of its own — captures write to the engine's session dirs (under
  `~/.capture/runs` by default here). The GUI is a viewer/controller.

## Configuration

- `CAPTURE_DAEMON_JSON` — daemon discovery file (shared with the CLI/MCP).
- Default capture output dir: `~/.capture/runs`.

## Known limitations / open items

- **Screenshot preview uses `img(PathBuf)`** (URI-cached). Timestamped filenames
  never repeat, so over a long capture the image cache grows — switch to
  `RenderImage` with eviction (product-architecture.md) before long-run use. The live
  transcript/preview itself is **done** (SSE).
- **Tray/hotkey latency** ~250 ms (the drain interval) — fine for clicks/presses; lower
  it or use `set_event_handler` if it ever feels laggy.
- **Hotkey "start" needs a window selected** in the picker (the "stop from anywhere"
  direction works unconditionally); a frontmost-window default would need engine
  z-order (the daemon's window list is largest-first, not front-to-back).
- A **proper menu-bar icon** (vs the text title) + `LSUIElement` (hide the Dock icon,
  needs the .app Info.plist) are still pending.
- **Packaging is ad-hoc signed (not notarized).** `packaging/build_macos_dmg.sh`
  produces a **self-contained** `Capture.app` + `.dmg` that bundles a PyInstaller-frozen
  daemon (`Contents/Resources/captured/`) + the **mlx on-device ASR runtime** + the
  signed `audiocap` helper + the skill — but it is **not Developer-ID signed / not
  notarized**, so Gatekeeper blocks first launch (README documents the bypass).
  Developer-ID signing + notarization is #31. The freeze excludes torch/faster-whisper/
  riva (CUDA/cross-platform); Whisper **weights** download on demand via the in-app model
  manager (never bundled). The mlx runtime makes the bundle large: **DMG ≈ 166 MB**
  (mlx's `.metallib` is ~125 MB and barely compresses), `.app` ≈ 400 MB. The frozen
  entry calls `multiprocessing.freeze_support()` — numba (a mlx_whisper dep) uses
  multiprocessing and a frozen child would otherwise re-run the entry and spawn a rogue
  second daemon.
- **No start-options UI** beyond per-app audio + 2 s interval (ASR **model** picking is
  done — the model manager); no per-session ASR backend toggle, no output-dir chooser.
- **gpui 0.2.2 → zed git rev** migration owed before Linux (M5) / accessibility.
- **No automated UI tests** (GPUI has no public UI-test harness); the daemon client
  logic is the testable seam (kept thin).

## Tests

- Manual end-to-end (slice 1): with a daemon running, launch `capture-gui`; the
  header shows daemon health, the window picker lists targets, "Start capture" on a
  selected window produces a session that appears in the list and in `capture status`,
  and "Stop" ends it. Verified on macOS (gpui 0.2.2) — see `claude-progress.md`.
- The daemon `/v1` surface the GUI depends on is covered by the Python contract +
  smoke suites (daemon.md Tests); the GUI client mirrors those shapes.
- Self-contained bundle (slice 7): after `packaging/build_macos_dmg.sh`, the frozen
  daemon copied OUT of `Capture.app/Contents/Resources/captured/` boots, writes its
  discovery file, answers `/v1/health` (`ok:true`, platform darwin), and `/v1/windows`
  returns windows (Quartz works in the frozen binary); `codesign --verify --strict` of
  the .app passes and the `audiocap` helper keeps its `com.local.audiocap` identity.
  Verified on macOS 2026-06-15. The in-app auto-spawn path (GUI launches the bundled
  daemon) is a manual launch check (no headless harness for the GPUI window).
- Model manager (slice 8): the daemon `/v1/asr/*` routes are verified live — `GET
  /v1/asr/models` lists the catalog with `downloaded`/`active`/`downloading` flags;
  `POST /v1/asr/model` persists the active model to `config.json`; `POST
  /v1/asr/models/download` of `whisper-base-mlx` streamed `asr_download` progress
  (fraction 0→1) then `asr_download_done`, after which the model reads `downloaded:true`;
  bad/dup requests handled. The **bundled** daemon (with mlx) reports
  `backend_available:true` and runs as a single process (no rogue daemon —
  `freeze_support`). The GUI panel itself is a manual check (no GPUI test harness).
