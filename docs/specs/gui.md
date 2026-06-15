# Spec: GUI app (`capture-gui`, GPUI)

_Status: current as of 2026-06-15. Source of truth = the code; update this spec in the same change as the code._

## Purpose

The native desktop app (feature #33, M3) — a **GPUI (Rust)** client of the
`captured` daemon, so a normal human can drive captures without the terminal. Per
the owner's fixed constraints (product-architecture.md): **native GPUI, no web UI
ever**, and the MCP/agent path stays first-class. The GUI is a thin peer of the
daemon (like the MCP server and CLI): all capture logic stays in the Python engine
behind `/v1`; the Rust side is UI + a daemon client only.

**This slice (#33 slice 1, macOS):** a single-window dashboard — daemon health, a
window picker, start/stop, and a live-polled session list — proving the
GPUI↔daemon integration end to end.

## Files

- `gui/` — a standalone Cargo project (not part of the Python package; its own
  build, `gui/target` gitignored).
  - `gui/Cargo.toml` — `gpui = "0.2.2"` (crates.io), `ureq` (blocking HTTP),
    `serde`/`serde_json`, `dirs`.
  - `gui/src/daemon.rs` — `Daemon` client (mirrors `daemon/client.py`): `discover()`
    from `~/.capture/daemon.json`, `health/sessions/windows/start/stop`.
  - `gui/src/app.rs` — `CaptureApp` GPUI view (`Render`) + the poll loop + handlers.
  - `gui/src/main.rs` — `Application::new().run(...)`, opens one window.

## Public contract

- The GUI consumes only the daemon `/v1` API ([daemon.md](daemon.md)) — it adds no
  new backend surface. Today: `GET /v1/health`, `GET /v1/sessions`, `GET /v1/windows`,
  `POST /v1/sessions`, `POST /v1/sessions/{id}/stop`. (SSE `/v1/events`, the live
  preview, and the schema-generated types are slice-2 — see Known limitations.)
- No CLI/flags yet; the binary opens the window. Reads the same
  `CAPTURE_DAEMON_JSON` discovery file as the CLI.

## Behavior

- **Startup:** `CaptureApp::new` discovers the daemon (`daemon::discover()`), does a
  brief blocking initial load (health + sessions + windows), and starts a poll loop.
- **Poll loop:** `cx.spawn` + `Timer::after(1.5s)`; each tick fetches health +
  sessions on the **background executor** (blocking ureq off the main thread) and
  updates the view via `WeakEntity::update` + `cx.notify()`. Ends when the view drops.
- **Window picker:** the `/v1/windows` list; clicking a row selects it (`selected`).
  "Refresh windows" re-fetches.
- **Start:** "Start capture" POSTs `/v1/sessions` for the selected window
  (`pid`, `audio_source:"app"`, 2 s interval) into `~/.capture/runs`; the daemon's
  shared registry means the new session appears in the list (and to the CLI / any
  MCP agent) on the next poll.
- **Stop:** each running session row has a "Stop" button → `POST .../stop`.
- All daemon calls run off the main thread; failures land in the status line, never
  crash the UI.

## Invariants & constraints

- **GUI is a thin daemon client** — no capture/ASR logic in Rust; it never imports or
  reimplements the engine. A daemon-less launch shows "no daemon — run: capture
  daemon start" and stays usable (read-only/empty).
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

- **Slice 1 = polling, not push.** Wire `/v1/events` (SSE) for a live transcript /
  screenshot preview (`RenderImage`, not URI-cached `img()` — avoids the hours-long
  cache leak noted in product-architecture.md).
- **No tray / menu-bar presence or global hotkey yet** (`tray-icon` + `muda` +
  `global-hotkey`, per the spec) — slice 2+.
- **No packaging** (.app bundle / DMG, signing) — that's M4-adjacent and needs the
  Developer ID story (#31).
- **No start-options UI** beyond per-app audio + 2 s interval; no ASR/model picker,
  no output-dir chooser.
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
