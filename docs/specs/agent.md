# Spec: menu-bar agent (`CaptureBar`, native)

_Status: current as of 2026-06-16. Source of truth = the code; update this spec in the same change as the code._

## Purpose

The **always-resident menu-bar agent** — the packaged app's entry point and the
home of everything that must outlive the GPUI window: the persistent menu-bar
presence, the **daemon lifecycle**, and launching the window on demand. Per the
owner's decision (product-architecture.md), this is a **native per-OS agent**:
- **macOS:** `CaptureBar` — a Swift `NSStatusItem` app (`LSUIElement`, no Dock icon).
- **Windows:** a sibling native tray agent is **planned** (roadmap #36), same role.

It is a thin **peer client** of the daemon like the CLI/MCP/GUI: it reads
`~/.capture/daemon.json` for the endpoint + bearer token and polls `/v1`. No
capture/ASR logic lives in the agent.

Motivation: the GPUI window can't be the persistent menu-bar presence — gpui 0.2.2
forces `ActivationPolicy::Regular` (a Dock app, no `LSUIElement`), and keeping a
heavyweight GPUI process resident just for a tray is wasteful. The agent is tiny
(~110 KB) and always-on; the GPUI window launches only when wanted. It also fixes
the "the running daemon pins the .app so you can't delete/replace it" problem: the
agent stops the daemon on a clean Quit.

## Files

- `agent/macos/CaptureBar.swift` — the whole agent (AppKit; `swiftc -O`). A
  `Daemon` enum (discover + a synchronous `/v1` request helper) and an `Agent`
  (`NSApplicationDelegate`) owning the status item, a 2 s poll timer, and the menu.
- `packaging/build_macos_dmg.sh` — compiles it (`swiftc`) and makes it the bundle's
  `CFBundleExecutable` (+ `LSUIElement`), with `capture-gui` as a sibling helper in
  `Contents/MacOS/`.

## Bundle layout (macOS)

```
Capture.app/Contents/
  MacOS/CaptureBar          ← entry point (CFBundleExecutable, LSUIElement)
  MacOS/capture-gui         ← the GPUI window, launched on demand
  Resources/captured/       ← frozen daemon (+ audiocap helper) + mlx ASR runtime
  Resources/skill/          ← the capture skill
```

## Public contract

- Consumes only the daemon `/v1` API: `GET /v1/health`, `GET /v1/sessions`,
  `POST /v1/sessions/{id}/stop`, `POST /v1/admin/shutdown`. Adds no backend surface.
- Launches `capture-gui` with **`CAPTURE_AGENT=1`** in its environment — the contract
  that tells the GPUI process "you are just the window": skip your own tray, skip the
  daemon auto-spawn, and **exit when the window closes** (so each Open is a fresh,
  non-lingering window). The full env (incl. `CAPTURE_DAEMON_JSON`) is inherited so
  all three processes agree on the same daemon.

## Behavior

- **Launch:** create the status item; `ensureDaemon()` (start the bundled daemon if
  none answers `/v1/health`); open the window once; start a 2 s poll timer.
- **Poll (2 s, off the main thread):** `GET /v1/health` + `/v1/sessions`; update the
  status-item **icon** (`applyIcon`: an SF Symbol template image — `record.circle` idle,
  `record.circle.fill` + the running count while capturing; falls back to text if the symbol
  is missing) and the menu labels/enabled state. An icon (not text) so it's actually findable
  in a crowded / notched menu bar. UI mutation hops to the main thread.
- **Restart-on-update (`restartSelf`):** each poll also checks for `restart.request` (sibling of
  `daemon.json`, written by the GUI's "Restart to finish update" button — see gui.md). When present, the
  agent removes it and restarts the WHOLE app on the just-installed bundle: spawn a detached relauncher
  (`bash` that waits for `CaptureBar` to exit, then `open`s the .app — it reparents to launchd on our
  death), then `quit()`. This exists because the in-app updater **can't** kill the agent from inside the
  process tree (a LaunchServices app resists SIGKILL from its own descendants), but the agent can always
  terminate ITSELF — so the updater stages the new bundle + restarts the daemon, and the agent finishes.
- **Menu:** a disabled header (`daemon: stopped|running · idle|running · N capturing`),
  **Open Window** (focus the existing `capture-gui` window if its process is still
  running — tracked as `guiProcess`, via `NSRunningApplication.activate` — else launch a
  new one; no duplicate windows), **Stop All Captures** (POST stop for each running
  session; enabled only when N>0), **Start/Stop Daemon** (toggles on state), **Quit Capture**.
- **One-shot `--request-mic`:** when launched with this flag the agent does NOT run the menu
  app — it calls Swift `AVCaptureDevice.requestAccess(.audio)`, waits for the answer, and
  exits. The GUI's Microphone "Grant" spawns this (the headless daemon can't prompt; this
  Swift one-shot can, and shares the bundle's Team ID so the grant reaches the daemon).
- **Daemon lifecycle:** `ensureDaemon()` spawns `Contents/Resources/captured/captured`
  with `CAPTURE_AGENT=1`. It first checks `/v1/health` so it never double-starts (a CLI- or
  previously-started daemon is adopted), and **debounces** (`lastSpawn`, 6 s) so a slow startup
  doesn't trigger a second spawn. **Auto-respawn:** the 2 s poll re-spawns the daemon whenever it's
  down **unless the user explicitly stopped it** (`userStoppedDaemon`, set by "Stop Daemon", cleared by
  "Start Daemon") — crash recovery (robust to however the daemon first started), and what
  makes the GUI's "Restart daemon" work (the GUI POSTs `/v1/admin/shutdown` to apply a new
  Screen Recording grant; the agent brings it back).
- **Closing the agent closes the whole app (matches the Windows tray agent, #48 follow-up).** macOS has
  no kill-on-close job object (see [agent-windows.md](agent-windows.md)), so the equivalent is two parts:
  (a) **`Quit`** gracefully stops the daemon (`/v1/admin/shutdown`) and terminates the GUI window
  (`guiProcess.terminate()`); (b) the daemon and the GUI both call `capture_core::exit_when_parent_dies()`
  at startup — gated on `CAPTURE_AGENT` — which watches `getppid()` and exits when the agent dies, so a
  **force-quit or crash** of the agent doesn't leave orphans either. A CLI-started daemon (no
  `CAPTURE_AGENT`) keeps running. (Previously the daemon was spawned "detached to survive force-quit" and
  Quit left the window + a busy daemon running — the orphans the user hit.)

## Invariants & constraints

- **Thin peer, no engine** — the agent never imports/links the engine; it only spawns
  the opaque daemon binary and talks `/v1`. Same daemon-peers rule as the GUI/CLI/MCP.
- **One menu-bar presence** — under the agent the GPUI app builds **no** tray
  (`CAPTURE_AGENT=1`), so there's never a duplicate status item.
- **Native per-OS** (owner's decision) — macOS Swift here; a Windows agent is the
  planned sibling (#36). Cross-process coordination is only via the daemon `/v1` API,
  so the agents share no code but share the contract.

## Failure modes & handling

- No bundled daemon (e.g. a dev `.app` without the freeze) → `ensureDaemon()` no-ops;
  the menu shows `daemon: stopped` and offers **Start Daemon** (still a no-op without
  a binary) — capture is simply unavailable until a daemon is started elsewhere.
- `capture-gui` missing beside the agent → **Open Window** logs and does nothing.
- Daemon not yet ready when the window opens → the window's own poll loop re-discovers
  and connects within ~1–2 s (it shows the "no daemon" hint briefly).

## Outputs / artifacts

- None of its own. State lives in the daemon; captures write to the engine's session dirs.

## Configuration

- `CAPTURE_DAEMON_JSON` — daemon discovery file (shared with the daemon/CLI/GUI).
- Inherited by the daemon and the window it spawns, so all three agree on one daemon.

## Known limitations / open items

- **Quit-stops-idle-daemon is a heuristic** — if a capture is running, Quit leaves the
  daemon (and thus the running capture) alive, which means the `.app` stays pinned
  until that capture ends. Acceptable (don't kill a capture on Quit), documented.
- **No global hotkey in the agent yet** — ⌃⌘R still lives in the GPUI window (works
  while it's open). Moving it to the agent (Carbon `RegisterEventHotKey`) so it works
  window-less is a follow-up.
- **Menu-bar icon** is an SF Symbol (`record.circle` / `record.circle.fill`) — done. A
  custom branded glyph could replace it later.
- **Login-item registration** (SMAppService, launch at login) is not wired yet.
- **Windows agent (#36)** not built — only the macOS agent ships today.
- **Ad-hoc signed / not notarized** (same as the rest of the bundle) — Developer ID is #31.

## Tests

- Verified on macOS 2026-06-16: launching `Capture.app/Contents/MacOS/CaptureBar`
  stays resident, spawns the bundled daemon (`/v1/health ok`), and launches
  `capture-gui` (`CAPTURE_AGENT=1`) for the window; a graceful Quit path stops the idle
  daemon. The menu-bar UI itself (clicks) is a manual check — no headless harness for
  `NSStatusItem`.
