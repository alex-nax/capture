# Spec: Windows tray agent (`Capture.exe`, native — #36)

_Status: **implemented (dev)**, 2026-06-17 — the agent binary (`agent/windows/` → `Capture.exe`) and the
logon-task registration (`packaging/register_logon_task.ps1`) exist and are verified on the Windows box;
the tray icon/menu **visuals** remain a manual check (no harness, same as macOS). The Windows sibling of
macOS [agent.md](agent.md) (`CaptureBar`). (`features.json` #36; packaged install + manual-tray
verification land with the M4 installer, [windows-release.md](windows-release.md).)_

## Purpose

The **always-resident system-tray agent** — the Windows install's entry point and the home of
everything that must outlive the GPUI window: the persistent tray presence, the **daemon lifecycle**,
and launching the window on demand. Per the owner's decision (product-architecture.md), the agent is
**native per-OS**; the Windows agent is written in **Rust** (`windows-rs` + the `tray-icon`/`muda`
crates already used by the GUI). It shares **no code** with the Swift macOS agent but shares the same
**`/v1` contract** — it is a thin peer client like the CLI/MCP/GUI (reads `daemon.json`, polls `/v1`,
holds no capture/ASR logic).

Why a separate agent rather than the GPUI app owning the tray: identical to macOS — a resident GPUI
process is too heavy for an always-on tray, and (on macOS) gpui forces a Dock app. The agent is tiny
and always-on; the window launches only when wanted. It also lets a clean Quit stop an idle daemon so
the install can be updated/uninstalled without being pinned.

## Files

**[current]**
- `agent/windows/Cargo.toml`, `agent/windows/src/main.rs` — the whole agent → `Capture.exe`. A small
  `Daemon` `/v1` client (`ureq`), a `tray-icon` + `muda` tray/menu, and an `Agent` state machine driven
  by a **minimal Win32 message loop** (`GetMessageW` + a 2 s `WM_TIMER` poll; menu clicks via
  `muda::MenuEvent`). Tray icons are generated in code (gray dot idle / red dot recording) — no asset
  file yet. Deps are pinned to the GUI's versions (`tray-icon` 0.24.1 / `muda` 0.19.2 / `ureq` 2 /
  `windows` 0.61) so building into the **shared `gui/target`** reuses already-built artifacts and dodges
  the Smart App Control build-script block (see windows-release.md §5).
- `packaging/register_logon_task.ps1` — register/unregister `Capture.exe` as an **interactive logon
  task** (the Windows daemon-lifecycle entry; the installer calls it; also usable from source).

**[planned]**
- `packaging/build_windows.ps1` — `cargo build --release` the agent → `Capture.exe` at the install root.
- `packaging/capture.iss` — makes `Capture.exe` the Start-Menu target and calls `register_logon_task.ps1`.

## Bundle layout (Windows)

```
%LOCALAPPDATA%\Programs\Capture\
  Capture.exe            ← THIS agent (entry point + logon task)
  capture-gui.exe        ← the GPUI window, launched on demand (CAPTURE_AGENT=1)
  captured\captured.exe  ← frozen daemon, spawned by the agent
  captured\audiocap_win* ← audio helper beside the daemon
  skill\                 ← the capture skill
  icons\capture-idle.ico, capture-rec.ico  ← tray state icons
```

(Parallel to macOS `Capture.app/Contents/{MacOS/CaptureBar, MacOS/capture-gui,
Resources/captured, Resources/skill}` — see [agent.md](agent.md).)

## Public contract

- Consumes only the daemon `/v1` API: `GET /v1/health`, `GET /v1/sessions`,
  `POST /v1/sessions/{id}/stop`, `POST /v1/admin/shutdown`. Adds **no** backend surface.
- Launches `capture-gui.exe` with **`CAPTURE_AGENT=1`** in its environment — the same contract the
  macOS agent uses and the GUI **already honors on every OS** (`gui/src/main.rs` checks the env var:
  skip its own tray, skip the daemon auto-spawn, and exit on window-close). The full env (incl.
  `CAPTURE_DAEMON_JSON`) is inherited so agent + daemon + window agree on one daemon.
- Discovers the bundled daemon at `current_exe().parent()\captured\captured.exe`; `CAPTURE_DAEMON_BIN`
  overrides (dev/test).

## Behavior

Mirrors `CaptureBar` ([agent.md](agent.md)) route-for-route:

- **Launch:** create the tray icon; `ensure_daemon()` (start the bundled daemon if none answers
  `/v1/health`); open the window once; start a 2 s poll thread.
- **Poll (2 s, background thread):** `GET /v1/health` + `/v1/sessions`; update the tray **icon**
  (`capture-idle.ico` idle, `capture-rec.ico` while capturing — Windows has no SF Symbols, so two
  pre-baked `.ico` assets), the tooltip, and the menu labels/enabled state. UI mutation is marshaled
  to the tray event loop.
- **Menu:** a disabled header (`daemon: stopped|running · idle · N capturing`), **Open Window** (focus
  the existing `capture-gui` window if its tracked process is still alive — via the stored PID +
  `SetForegroundWindow`/`ShowWindow(SW_RESTORE)` — else launch a new one; never duplicate windows),
  **Stop All Captures** (POST stop per running session; enabled only when N>0), **Start/Stop Daemon**
  (toggles on state), **Quit Capture**.
- **Daemon lifecycle:** `ensure_daemon()` spawns `captured\captured.exe` **detached** with
  `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP` and stdio set to null (no console flash, no
  whole-screen-capture pollution, survives an agent force-quit). It first checks `/v1/health` so it
  never double-starts (a CLI- or previously-started daemon is adopted) and **debounces** (~6 s) so a
  slow startup doesn't trigger a second spawn. **Auto-respawn:** the 2 s poll re-spawns the daemon
  whenever it's down **unless the user explicitly stopped it** (`user_stopped_daemon`, set by "Stop
  Daemon", cleared by "Start Daemon") — crash recovery, and what makes the GUI's "Restart daemon"
  work. **Quit** gracefully stops the daemon (`/v1/admin/shutdown`) **iff it's idle** (no running
  captures) — freeing the install for update/uninstall while letting an in-progress capture survive an
  accidental Quit — then exits.
- **Autostart (decided + implemented):** a **Task Scheduler interactive logon task** registered by
  `packaging/register_logon_task.ps1` (`-AtLogOn`, `LogonType Interactive`, no execution-time limit) runs
  `Capture.exe` in the WinSta0 desktop at login — satisfying the window-discovery requirement. Verified
  register → query → unregister with no admin/UAC. The installer invokes it (uninstall unregisters). An
  `HKCU\...\Run` key was the simpler alternative, but the logon task matches the daemon-lifecycle
  decision and is visible in Task Scheduler.

## Invariants & constraints

- **Thin peer, no engine** — never imports/links the engine; spawns the opaque daemon binary and
  talks `/v1`. Same daemon-peers rule as the GUI/CLI/MCP.
- **One tray presence** — under the agent the GPUI app builds **no** tray (`CAPTURE_AGENT=1`), so
  there is never a duplicate tray icon.
- **Native per-OS** (owner's decision) — Rust here, Swift on macOS; the two agents share no code, only
  the `/v1` contract.
- **`CREATE_NO_WINDOW` for every spawn** (daemon, any helper) — a stray console window would steal
  foreground and contaminate whole-screen captures.

## Failure modes & handling

- No bundled daemon (dev layout without the freeze) → `ensure_daemon()` no-ops; the menu shows
  `daemon: stopped` and offers **Start Daemon** — capture is unavailable until a daemon starts.
- `capture-gui.exe` missing beside the agent → **Open Window** logs and does nothing.
- Daemon not ready when the window opens → the window's own poll re-discovers and connects within
  ~1–2 s.
- Daemon spawned but crashes on start (missing dep / corrupt freeze) → poll shows "down"; auto-respawn
  retries (debounced) unless the user stopped it; persistent failure surfaces in the GUI header.

## Outputs / artifacts

- None of its own. State lives in the daemon; captures write to the engine's session dirs.

## Configuration

- `CAPTURE_DAEMON_JSON` — daemon discovery file (shared with daemon/CLI/GUI); inherited by the daemon
  and the window the agent spawns, so all three agree on one daemon.
- `CAPTURE_DAEMON_BIN` — override the bundled-daemon path (dev/test).

## Known limitations / open items

- **Tray icons are generated in code** (gray dot idle / red dot recording) — dependency-free; a branded
  `.ico` glyph is a later polish.
- **Open Window doesn't focus an already-open window** — it no-ops when the GUI child is still alive (so
  it never duplicates). Focusing it (track the PID → `EnumWindows` → `SetForegroundWindow`) is a TODO.
- **Autostart = interactive logon task** (`packaging/register_logon_task.ps1`) — decided + verified.
- **No global hotkey yet** — parity with macOS (⌃⌘R lives in the GPUI window); moving a global hotkey
  into the agent (`RegisterHotKey`) is a follow-up.
- **Microphone permission** — Windows has no TCC; per-app mic access is OS Settings (Privacy →
  Microphone). No one-shot `--request-mic` analog is needed (that is a macOS mechanism). Deferred.
- **No automated tray UI test** — the icon/menu are a manual check, same as macOS; the daemon
  lifecycle (spawn → `/v1/health` → graceful shutdown) **can** be tested headless.

## Tests

- **[current, 2026-06-17]** Verified on the Windows box (interactive session via
  `scripts/run_interactive.ps1`): with a pre-started daemon, launching `Capture.exe` stays **resident**,
  **adopts** the daemon (no double-spawn — it health-checks first), and **launches exactly one**
  `capture-gui.exe` with `CAPTURE_AGENT=1` (`agent_alive=true, gui_count=1, daemon_running=true`).
  `packaging/register_logon_task.ps1` register → verify → unregister round-trips clean (Interactive
  logon task, no admin).
- **[manual]** Tray icon appearance, menu clicks (Open / Stop All / Start-Stop Daemon / Quit), the
  quit-stops-idle-daemon path, and survives-window-close are visual checks — no tray test harness (same
  as macOS). `features.json` #36 flips `true` after the packaged install + a manual tray pass (M4).
