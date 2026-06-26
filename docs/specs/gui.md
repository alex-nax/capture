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
    `/v1/events` SSE line reader). Plus `resolve_daemon()` → `bundled_daemon()` (the
    `Contents/Resources/captured/captured` in the packaged app) **or, in dev, the
    `captured` built beside `capture-gui` in the shared workspace target** (v3: the GUI
    flips onto the Rust daemon — `cargo run -p capture-gui` self-spawns it), and
    `spawn_detached()` (launch it in its own process group so captures outlive the GUI).
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
  - `gui/src/main.rs` — `Application::new().with_assets(Assets).run(...)`, opens one window.
  - `gui/src/assets.rs` — `Assets`, a gpui `AssetSource` serving the embedded SVG icons
    (`include_bytes!` of `gui/assets/icons/*.svg`, Lucide/MIT). Wired via `with_assets`.

**Icons:** real SVG (not emoji/Unicode glyphs) via gpui's `svg()` element, which rasterizes
each icon to an alpha mask tinted by `text_color`. The `icon(name, size, color)` helper in
`app.rs` renders `svg().path("icons/<name>.svg")`. Add an icon by dropping an SVG in
`gui/assets/icons/` and listing it in `assets.rs::ICONS`.

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
- **Agent mode (`CAPTURE_AGENT=1`):** when launched by the native menu-bar agent
  ([agent.md](agent.md)) the GPUI process is *just the window* — it builds **no** tray
  (the agent owns the menu bar), does **not** spawn the daemon (the agent owns the
  lifecycle), and **exits when its window closes** (`main.rs` registers
  `on_window_closed → cx.quit()`, since GPUI doesn't auto-quit on last-window-close). Run
  standalone (dev, no agent) it keeps its own tray + daemon auto-spawn and persists.
- **Poll loop:** `cx.spawn` + `Timer::after(1.5s)`; each tick **re-discovers** the daemon
  (so it attaches to the just-spawned bundled one) and fetches health + sessions on the
  **background executor** (blocking ureq off the main thread), updating the view via
  `WeakEntity::update` + `cx.notify()`. Ends when the view drops.
- **Window picker (grouped, multi-select):** the `/v1/windows` list **grouped by application**
  (first-seen order). Each group is a header (`App  (N)` + a 🎤 mic radio) followed by a **checkbox
  row** per window (`☑/☐` + title). `checked: HashSet<window_id>` holds the selection; you can check
  windows across **several apps** at once. "Refresh windows" re-fetches (and the mic device list). A
  blank title shows `(untitled window)` — macOS redacts `kCGWindowName` for processes **without Screen
  Recording permission**.
- **Mic selector:** a `Mic:` row of chips — **No mic** + one chip per input device from
  `GET /v1/audio/mics` (`mics`, fetched once a daemon appears + on Refresh). The chosen device id
  (`mic_device`) persists in `gui-settings.json`. The 🎤 radio on a group header picks the single app
  (`mic_app`) the mic attaches to — see Start.
- **Start (multi-session):** "Start capture" POSTs **one `/v1/sessions` per checked window** into
  `~/.capture/runs` (sequentially), each with `pid`, **`window_id`** (pins screenshots to the exact
  window — pid alone can't disambiguate two windows of one process, e.g. two Chrome windows),
  `audio_source:"app"`, and the persisted capture-quality. **Audio is deduped per app**: only the
  first checked window of each `pid` gets `capture_audio:true` (macOS audio is per-app — see
  `audio.md`); the rest are screenshots-only. If a device is selected and the window's app matches the
  🎤 `mic_app`, that first session also gets `mic_device` (a **separate** mic track). The shared
  registry means every new session appears in the list (and to the CLI / any MCP agent) on the next
  poll. `checked` clears on success.
- **Launch & Capture (process/URL):** a minimal single-line input (`cmd_input`, a
  focusable `div` + `track_focus`/`on_key_down`: printable `key_char`, backspace,
  ⌘V paste, Enter = launch — no IME/selection) + a button POST `/v1/sessions` with
  `command` (launch mode). A URL is just a command, e.g. `open https://…`.
- **Import… (audio/video → session, #43):** an **Import** row whose button runs the native
  macOS file picker (`osascript choose file`, off the UI thread) and POSTs the chosen path to
  `/v1/sessions/import`. The daemon extracts audio/frames + runs ASR in the background; progress
  streams over SSE into `LiveState.import_progress` (shown inline as `<phase> <pct>%`), and on
  `import_done` the poll loop opens the new session (which arrives in the same `/v1/sessions` poll).
- **Sessions list:** newest first, all sessions (no 6-row truncate; the page scrolls).
  Each row has compact **SVG icon** actions (Lucide, via the `AssetSource` — see Icons):
  **folder** (`open` the session's `dir` in Finder), **clipboard** (copy a ready-to-paste
  summarization prompt for a coding agent; `cx.write_to_clipboard`), and **stop** (running)
  or **trash** (finished). Clicking the row body **opens the Playback screen** (below).
- **Delete confirmation:** the trash icon sets `confirm_delete` (it does NOT delete
  immediately); a modal overlay (an occluding dim backdrop + a centered card with
  **Cancel** / **Delete**) confirms, then `POST .../delete`.
- **Playback screen (3rd top-level screen):** clicking a session opens `playback` (with a
  `← Back` header). It tracks the session for live SSE (backfill via `GET .../transcript`,
  then `/v1/events` appends `transcript_segment` + updates `screenshot_taken` into
  `LiveState`). **Running** sessions show the live latest screenshot (`img(PathBuf)`) + the
  recent transcript ("● live"). **Finished** sessions load `screenshots/` + `transcript.jsonl`
  + `mic_transcript.jsonl` from disk off the main thread (filenames/segment `start`/`end` are
  ISO stamps parsed to epoch by `parse_iso_epoch`, no `chrono`) and render a **video-style
  scrubber**: a draggable timeline + transport (`skip-back` / `rewind −5s` / `play`·`pause` /
  `fast-forward +5s` / `skip-forward` + `m:ss / m:ss`) that moves the shown screenshot (last
  frame ≤ playhead) and the active subtitle (segment spanning the playhead; mic lines marked
  with the mic icon) through time. Play auto-advances via a ~200 ms ticker. The scrubber maps
  mouse-x to time using the content width (`window.viewport_size()`), drag handled on the root
  like the scrollbar (`pb_dragging`).
- **Live mic switcher (#46, playback screen, running sessions):** a "Mic" chip row (`switch_mic`
  → `POST /v1/sessions/{id}/mic`) with **Off** + each input device from `self.mics`, highlighting the
  session's active `mic_device`. Switching/turning the mic on/off happens live (no restart) and appends to
  the mic track.
- **Manage (playback screen, finished sessions):** capability **status icons** (image/volume,
  dimmed when pruned, from the session's `has_screenshots`/`has_audio` flags) plus action buttons:
  **Halve frames** / **Delete frames** / **Remove audio** (`prune` → `POST .../prune`; the destructive
  ones go through the shared confirm modal — `ConfirmKind::Prune`) and **Re-transcribe** (`retranscribe`
  → `POST .../retranscribe`, disabled when `can_retranscribe` is false). Re-transcribe progress streams
  over SSE into `LiveState.retranscribe` (shown as a %); on `retranscribe_done` the poll loop reloads the
  open session so its new transcript appears. The session-list **trash** delete and these prune actions
  share one modal (`confirm: Option<ConfirmKind>`). The Manage panel also has **Build index** (#44 —
  `index_session` → `POST .../index`, the `list-tree` icon, enabled only when `can_index` **and**
  `index_status.available`; progress streams over SSE into `LiveState.index_progress`, and the **root
  summary + node count** of a built index render below the actions, loaded via `GET .../index` on open).
- **Index endpoint config (Settings, #52/#53):** a **provider** selector (LM Studio / Ollama / OpenAI /
  Custom) + **host** + **port** (a single base-URL field for Custom) + an **API-key** field shown only for
  providers that need one (OpenAI), persisted to `gui-settings.json` (legacy free-text `index_url` is
  migrated to host:port). The **model** field is a **dropdown** populated from `GET /v1/index/models`
  (`daemon.index_models`), refreshed on provider/host/port change or a Refresh chip; the build sends the
  structured `provider/host/port` (the daemon composes the endpoint). A provider selector was built rather
  than deferred.
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
  live `↓ NN%` while downloading. While a download is in flight the row also shows a
  thin **determinate progress bar** (a `relative(fraction)`-width fill) under the
  header, so progress is visible at a glance, not just as a percentage. The action is
  a **Download** button for any not-yet-downloaded model (including an un-downloaded
  active one), a **Use** button for a downloaded-but-inactive model, and a **Remove**
  button (amber-red) for any downloaded model. **Download** POSTs
  `/v1/asr/models/download` (the daemon fetches in the background); progress arrives as
  `asr_download` events on the same SSE stream and is accumulated into
  `LiveState.asr_progress` (repo → fraction) — these events have **no `session_id`**, so
  the SSE thread handles them *before* the session filter that would drop them.
  **Use** POSTs `/v1/asr/model` to set the active model. **Remove** POSTs
  `/v1/asr/models/delete` to free the model's weights from the HF cache (removing the
  active model just reverts it to "active · needs download"); the poll loop then flips
  the row back to **Download**. The runtime lives in the
  daemon (mlx); if a daemon lacks it, `backend_available:false` shows a "runtime
  unavailable" note instead of the list. Weights download on demand (never bundled).
- **Three screens:** a header button (top-right) shows a **settings** icon ("Settings") on
  the dashboard and a **chevron-left** ("Back") on a sub-screen. Exactly one of **dashboard**
  (Refresh/Start, Launch input, windows/sessions lists), **Settings**, or **Playback** renders
  — gated by `let playback = self.playback.is_some(); let sett = settings && !playback; let dash
  = !settings && !playback;` (panels use `sett.then(|| …)` / `dash.then(|| …)` / `playback.then(||
  render_playback)`). Settings holds: **Capture quality** (`chip` toggles —
  **Screenshots On/Off** `capture_screenshots` (off ⇒ an audio-only capture: the body sends
  `capture_screenshots:false`, no `screenshots/`), PNG/JPEG `shot_format`, resolution `shot_res_ix`
  over `RES_PRESETS`, JPEG quality when jpeg; merged into the `/v1/sessions` body via
  `shot_settings()` for new captures), and the **Index endpoint** (#44 — the LM Studio chat URL
  `index_url` + a reachability dot polled via `GET /v1/index/status` on a slow separate timer;
  Enter/Check re-probes). These
  prefs (and the selected `mic_device`) **persist across relaunch**: each change writes
  `~/.capture/gui-settings.json` (`save_settings`) and `new()` seeds the fields from it
  (`load_settings`) — they live in the window process (a UI default), not the daemon. Settings also holds the
  **App update** row (#48 — `update.rs`: at startup `update::check()` queries GitHub
  `repos/alex-nax/capture/releases/latest`, semver-compares the tag to the running `CARGO_PKG_VERSION`,
  and finds the `.dmg` asset; Settings shows `vX · up to date` or `vY available → Update…`. Update goes
  through the shared confirm modal (`ConfirmKind::Update`); on confirm, `download_and_install` fetches the
  notarized dmg and spawns the updater. **macOS restart model (important):** the updater runs inside the
  app's own process tree (agent → gui → updater) and from there CANNOT reliably kill the menu-bar agent — a
  LaunchServices app resists being SIGKILLed by its own descendants (the daemon, a non-ancestor, dies fine;
  the agent survived every in-app attempt, incl. `setsid`- and `nohup`-detached variants). So the updater
  deliberately does the minimum that works from in-tree: mount the dmg, replace `/Applications/Capture.app`,
  reset `~/.capture/daemon.json`, and `pkill` only the **daemon** so the agent's auto-respawn brings it back
  **from the new bundle (new version)**. The GUI then sees the daemon report a version newer than its own
  (`update_staged()` = `parse_semver(health.version) > CURRENT`) and `section_updates` swaps the install
  loader for a **"Restart to finish update"** button. Clicking it (`request_restart`) drops
  `~/.capture/restart.request` (sibling of `daemon.json`, via `daemon::restart_request_path`); the agent
  polls for that flag and restarts the WHOLE app **itself** — a process can always terminate itself, so this
  succeeds where killing-from-outside-in didn't (see agent.md `restartSelf`). While the daemon is being
  replaced the row shows "installing update…" (NOT a 0 % bar); once the new daemon answers, it becomes the
  Restart button — never any action without confirmation. (Windows keeps its full-restart updater — its
  tray agent's kill-on-close job object makes in-tree stop/relaunch work, so `update_staged` stays false
  there and the button doesn't appear.)),
  the **Whisper model manager**, a **Transcription** panel (#45 — `language_field` + `chunk_chips`: the language
  and chunk-length settings, written to the DAEMON via `POST /v1/asr/language`/`/v1/asr/chunk` and read back
  from the polled catalog, since the engine — not the window — consumes them; the language field is mirrored
  in the playback pane for on-the-fly correction), the **Permissions** panel, and the **skill installer**. Each
  panel is rendered via `settings.then(|| …)` / `(!settings).then(|| …)` so only one
  screen's panels exist at a time.
- **Layout & scrolling:** the window is **one** vertically-scrolling column (`#root`,
  `track_scroll(&root_scroll)` + `overflow_y_scroll`); the content (windows/sessions,
  model manager, live detail pane) exceeds the viewport. There is deliberately **no
  nested scroll** — bare gpui 0.2.2 has no scrollbar widget and nested `overflow_scroll`
  regions fight the root for the wheel ("scroll together"), so a single scroll context is
  used. A **custom draggable scrollbar** (`scrollbar()` + `on_scrollbar_drag()`) is drawn
  as an absolute overlay on the right from the `ScrollHandle`'s prior-frame
  `bounds()`/`max_offset()`/`offset()` (thumb size = viewport/content; auto-hidden when
  content fits; drag updates `set_offset`). The detail pane is content-sized
  (`flex_shrink_0`), not `flex_1` (which would grab the scroll container's unbounded axis).
- **Permissions (macOS):** a panel (`perm_row`) with a row each for **Screen Recording**
  and **Microphone**, each showing the daemon's status (`GET /v1/permissions`, polled:
  `✓ granted` / `✗ not granted` amber-with-why / `not requested`) + **Grant** + **Settings**
  (`x-apple.systempreferences:…?Privacy_ScreenCapture|Privacy_Microphone` for grant **or
  revoke**). Neither prompt goes through the headless daemon (it aborts): **Screen
  Recording** is prompted in THIS process (`screen_perm::request()`, CoreGraphics FFI to
  `CGRequestScreenCaptureAccess`); **Microphone** by spawning the bundled **agent one-shot**
  (`<exe dir>/CaptureBar --request-mic` → Swift `AVCaptureDevice.requestAccess`). Both work
  because every binary shares the Developer-ID **Team ID**, so the grant reaches the daemon
  and persists (the whole point of #31). A **Restart daemon** button applies a new Screen
  Recording grant: POSTs
  `/v1/admin/shutdown`; the **menu-bar agent auto-respawns** the daemon (no app quit/reopen).
  The panel is hidden on non-macOS.
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
- **Cross-platform OS integrations (done 2026-06-17, Phase 2).** The window-process helpers are now
  `#[cfg]`-gated per OS so the GUI is usable on Windows (macOS behaviour byte-for-byte unchanged): file
  picker (`osascript` → PowerShell `OpenFileDialog`), folder reveal (`open` → `explorer`), privacy
  settings (`x-apple.systempreferences:` → `ms-settings:`), mic-grant (CaptureBar one-shot → Settings
  deep-link), and daemon spawn (`process_group` → Windows `creation_flags`, Phase 0). `main.rs` no longer
  `unwrap()`s renderer creation — it logs a hint and exits cleanly instead of panicking when the DirectX
  renderer can't be created (Windows needs the interactive desktop — `DXGI_ERROR_NOT_CURRENTLY_AVAILABLE`
  from a service shell). Remaining for Windows: the tray uses a text title (Windows wants an `.ico`); the
  persistent tray + lifecycle is the native agent #36 ([agent-windows.md](agent-windows.md)). See
  [windows-release.md](windows-release.md).
- **No console window on Windows (release).** `main.rs` carries
  `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`, so a **release** build is a GUI
  app with no console — closing a stray terminal can no longer kill the app. Debug keeps the console for
  dev diagnostics, so the shipped installer must use **release** binaries (the agent `Capture.exe`
  carries the same attribute; the daemon is always spawned `CREATE_NO_WINDOW`).
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
