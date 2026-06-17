# Spec: Windows release (packaging, installer, daemon lifecycle, signing, auto-update)

_Status: **PLANNED / design** as of 2026-06-17 — this scope is **not yet implemented**. Source of
truth for *shipped* behavior remains the code; this spec is the agreed **intent** for the M4 Windows
release (`features.json` #34, with the tray agent #36 in [agent-windows.md](agent-windows.md)). Like
[product-architecture.md](product-architecture.md), sections mark **[current]** (true today) vs
**[planned]** (the target). Update each section to **[current]** in the same change that lands it._

## Purpose

Take capture-mcp from "the engine runs on Windows" to a **shippable Windows product**: a packaged
installer published to GitHub Releases, a daemon that starts at logon on the interactive desktop, the
native tray agent ([agent-windows.md](agent-windows.md)), and **cross-platform in-app auto-update** —
**without changing** the `/v1` contract ([daemon.md](daemon.md)), the helper PCM contract
([helper-contract.md](helper-contract.md)), or the session output layout ([session.md](session.md)).
It is the Windows analogue of the mature macOS pipeline (`packaging/build_macos_dmg.sh` +
`.claude/skills/capture-release` + `gui/src/update.rs`).

The engine layer is already ~done and live-verified (see [platform-abstraction.md](platform-abstraction.md)
§Tests: real-window capture + an 8-video YouTube playlist captured end-to-end on Windows via
GDI+ screenshots + WASAPI loopback + faster-whisper CUDA). The gap this scope closes is the **last
mile**: packaging, installer, signing/SmartScreen, daemon lifecycle, and auto-update.

**Decisions locked (owner, 2026-06-17):**
- **Inno Setup** installer (`.exe`) **+ a winget manifest**. (Not MSIX/WiX/NSIS.)
- **Logon task, never a Windows Service** — capture needs the interactive WinSta0 desktop.
- **Rust** tray agent (`windows-rs` + `tray-icon`/`muda`), see [agent-windows.md](agent-windows.md).
- **Don't bundle CUDA.** Ship the ASR runtime; CUDA is the user's responsibility with **CPU and
  remote alternatives** so non-NVIDIA Windows users still work (see [asr.md](asr.md)).
- **v1 ships unsigned** with honest SmartScreen docs, but the build chain has a **`signtool` hook**
  driven by `CAPTURE_WIN_SIGN_*` env so dropping in an OV cert later is config-only, not rework.

## Files

**[current]**
- `init.ps1` — dev bootstrap (venv + editable install + smoke), the Windows parallel of `init.sh`.
- `scripts/run_interactive.ps1` — run a command in the interactive WinSta0 session via a transient
  Interactive-logon scheduled task (the interactive-station escape hatch).
- `src/capture_mcp/core/platform/windows.py`, `helper/audiocap_win.py` — engine backends.
- `packaging/captured_main.py` — the PyInstaller entry (shared; calls `multiprocessing.freeze_support()`).

**[planned]**
- `packaging/build_windows.ps1` — the parallel of `build_macos_dmg.sh`: build GUI + agent, freeze the
  daemon, lay out the install tree, (optionally) sign, compile the Inno Setup script.
- `packaging/capture.iss` — Inno Setup script (layout, shortcuts, logon-task registration, uninstall).
- `packaging/winget/` — winget manifest referencing the GitHub-release `.exe`.
- `agent/windows/` — the Rust tray agent (Cargo crate); see [agent-windows.md](agent-windows.md).
- `gui/src/update.rs` — generalize the macOS-only updater to a cross-platform asset/installer flow.
- `.github/workflows/release.yml` — cross-platform release CI (macOS DMG + Windows installer under
  one tag). **None exist today** (`.github/` holds only an issue template).
- Core-portability prerequisite fixes — see *Behavior §1*.

## Public contract

- **Installed layout (per-user default, no UAC):** `%LOCALAPPDATA%\Programs\Capture\`
  ```
  Capture\
    Capture.exe            ← tray agent, the install's entry point + logon task (agent-windows.md)
    capture-gui.exe        ← the GPUI window, launched on demand (CAPTURE_AGENT=1)
    captured\captured.exe  ← PyInstaller-frozen daemon
    captured\audiocap_win* ← Windows audio helper beside the daemon (engine resolves it relatively)
    skill\                 ← the bundled capture skill
    icons\                 ← tray .ico assets (idle / recording)
  ```
  This mirrors the macOS `Capture.app/Contents/{MacOS,Resources}` layout. The agent/GUI discover the
  bundled daemon relative to their own exe (`current_exe().parent()` → `captured\captured.exe`);
  `CAPTURE_DAEMON_BIN` overrides for dev.
- **GitHub Release carries BOTH OS assets under ONE tag:** `Capture-<v>.dmg` (macOS) and
  `CaptureSetup-<v>-x64.exe` (Windows). Auto-update selects by OS (see *Behavior §6*). The `.exe`
  asset is **load-bearing** for Windows auto-update, exactly as the `.dmg` is for macOS.
- **Silent-install contract** (for auto-update): `CaptureSetup-<v>-x64.exe /VERYSILENT
  /SUPPRESSMSGBOXES /NORESTART` installs/upgrades in place with no UI.
- **Daemon transport** on Windows is `127.0.0.1:<ephemeral>` + bearer token via `daemon.json`
  ([daemon.md](daemon.md)); UDS stays macOS/Linux-only. No change.

## Behavior

### 1. Prerequisites — core-portability fixes [planned]

Three shared-core spots leak POSIX/macOS assumptions and gate a clean Windows daemon run
(verified against the code 2026-06-17):

- **`cli/__init__.py` `daemon start`** spawns `python -m capture_mcp.daemon` with
  `start_new_session=True` (`:53`), which is **POSIX-only**. Windows branch: detect `os.name == "nt"`
  and use `creationflags=subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW` instead
  (no console flash, survives the parent).
- **`vision_client._encode_image`** downscales screenshots via `sips` and **already falls back to the
  raw PNG on any failure** (`:69-70`), so multimodal indexing *works* on Windows — but the raw-PNG
  payload is much larger (token/bandwidth bloat). Add a `Pillow` (`PIL.Image.thumbnail`) downscale
  path before the raw-PNG fallback: chain `sips` (macOS) → Pillow (cross-platform) → raw PNG.
- **`import_media`** imports the macOS `helper_path` **lazily inside the function** (`:36`,
  `# macOS-only; import lazily`) — it does **not** break daemon import on Windows; the
  import-a-file-as-a-session feature is simply macOS-only today. A Windows path needs an ffmpeg-based
  audio/frame extraction (or wait for a cross-platform helper). Until then, `capture_import` should
  return a clear "not supported on Windows yet" error rather than a stack trace.

Already correct (no change): `audio.py` spawns the audio helper with `creationflags=_NO_WINDOW`
(`:39`); `util.descendant_pids` guards on `os.name` (POSIX `ps` only); `Path.home()/".capture"`
resolves to `C:\Users\<user>\.capture` (works; idiomatic `%APPDATA%` is an open item).

### 2. Packaging / freeze [planned]

`packaging/build_windows.ps1` (parallel to `build_macos_dmg.sh`):
1. `cargo build --release` the GUI (`gui/`) → `capture-gui.exe`.
2. `cargo build --release` the tray agent (`agent/windows/`) → `Capture.exe`.
3. PyInstaller **onedir** freeze of `packaging/captured_main.py` → `captured\captured.exe`, with
   Windows hidden-imports (`capture_mcp.core.platform.windows`, `asr.whisper_local`,
   `asr.openai_compat`, `indexer`, `frames`, `vision_client`, `import_media`) and
   `--collect-all faster_whisper` + CTranslate2; **exclude `mlx`/`Quartz`/`AVFoundation`** (the macOS
   freeze does the reverse). `freeze_support()` in `captured_main.py` is required (numba/multiprocessing).
4. Copy `helper/audiocap_win.py` beside the frozen daemon; bundle the `skill/`; embed a `.ico` +
   an application manifest; assemble the install tree (§Public contract).
5. **(optional) sign** every `.exe`/`.dll` (§5) if `CAPTURE_WIN_SIGN_*` is set.
6. Compile `packaging/capture.iss` (Inno Setup) → `dist/CaptureSetup-<v>-x64.exe`; sign the installer.
7. Best-effort ASR self-test (`captured.exe --asr-selftest`), non-fatal (like macOS).

**CUDA is not frozen in.** The faster-whisper Python is bundled, but the cuBLAS/cuDNN runtime
(`nvidia-*-cu12`) is **not** in the installer (keeps it small; product-architecture.md: "CUDA DLL
pack on Windows is an on-demand download, not part of the installer"). At runtime the daemon picks
CUDA if available, else CPU `int8`, else a configured remote endpoint — see [asr.md](asr.md).

### 3. Installer — Inno Setup [planned]

`packaging/capture.iss`:
- **Per-user install** to `%LOCALAPPDATA%\Programs\Capture` by default (no UAC). A per-machine
  (`%ProgramFiles%`) mode is optional and elevates.
- Lay out the install tree; create a Start-Menu shortcut (and optional desktop shortcut) to
  `Capture.exe`.
- **Register the logon task** (Task Scheduler, interactive, runs `Capture.exe` at the current user's
  logon) — NOT a Service (interactive WinSta0 is required for window discovery/screenshots, see
  product-architecture.md §Invariants and [windows.md](windows.md)).
- Add an Apps & Features uninstall entry; uninstall removes the logon task + shortcuts and leaves
  user data (`%USERPROFILE%\.capture`, the HF model cache) unless the user opts to purge.
- `winget` manifest points at the released `.exe`.

### 4. Daemon lifecycle [planned]

At **runtime** the tray agent owns spawn/stop ([agent-windows.md](agent-windows.md)). At **logon**
the installer's logon task starts the **agent**, which spawns the daemon — mirroring macOS
launchd → `CaptureBar`. For the from-source path, `capture daemon install|uninstall` registers/removes
the logon task (the launchd/systemd analogue, currently a [planned] item in [daemon.md](daemon.md)).

**Interactive-desktop preflight:** add a check (daemon health and/or `capture doctor`) that samples
window enumeration and, if the process is in a non-interactive window station (a Service, SSH, or CI),
surfaces a clear diagnostic ("no interactive desktop detected — capture needs the logged-on WinSta0
session; see `scripts/run_interactive.ps1`") instead of silently capturing a blank desktop.

### 5. Code signing / SmartScreen [planned]

- **Pipeline hook:** if `CAPTURE_WIN_SIGN_*` is set (cert + password/thumbprint + RFC-3161 timestamp
  URL), `signtool sign /fd sha256 /tr <ts-url> /td sha256 /a` every `.exe`/`.dll` before packaging,
  then sign the installer itself. The analogue of macOS `CAPTURE_SIGN_IDENTITY`.
- **v1 default: unsigned** + honest docs.
- **SmartScreen reality (verified mechanics):** Defender SmartScreen is an *app-reputation* gate that
  fires when a **downloaded** executable carrying the **Mark-of-the-Web** is launched without
  reputation — i.e. **once, on the first run of the downloaded `CaptureSetup.exe`** ("More info →
  Run anyway"). Files the installer writes to the install dir have no Mark-of-the-Web and are launched
  by the installer/logon-task, so the GUI, daemon, agent, helper — and **captures** — never trigger
  SmartScreen. The one recurring touchpoint is **auto-update**: each newly downloaded installer can
  re-show the prompt until that binary earns reputation. An OV cert (~$200–500/yr) shows the publisher
  name and accrues reputation across downloads (it does **not** clear cold-start); EV front-loads it;
  Azure Trusted Signing needs a ≥3-year org. **UAC is separate** (a per-machine install elevates; the
  default per-user install does not) and is not SmartScreen.

### 6. Auto-update — cross-platform [planned]

Generalize `gui/src/update.rs` (today: `.dmg`-only, `hdiutil`/`/bin/bash`/`/Applications/Capture.app`):
- Rename `UpdateInfo.dmg_url` → `asset_url`; `check()` selects the asset by OS via `cfg!(target_os)`
  (`.dmg` on macOS, `CaptureSetup-*-x64.exe` on Windows). `parse_semver` / latest-release query /
  confirm-modal UX are unchanged and stay shared.
- `download_and_install` **Windows branch:** download the installer to `%TEMP%`; write a detached
  **PowerShell** updater; spawn it hidden (`CREATE_NEW_PROCESS_GROUP`, no window); the GUI then exits.
  The updater: (a) stop the agent + daemon — `POST /v1/admin/shutdown` if idle, else `taskkill /IM
  Capture.exe /IM captured.exe /F`; (b) run `CaptureSetup-<v>-x64.exe /VERYSILENT /SUPPRESSMSGBOXES
  /NORESTART`; (c) wait for it; (d) relaunch `Capture.exe`. **Rollback:** back up the install dir
  first; on a non-zero installer exit, restore it.
- **Skill auto-update** already uses cross-platform `dirs` (`%USERPROFILE%\.claude\skills\capture`,
  `~/.codex/skills/capture`) and content-hash comparison — only a Windows smoke test is needed.

### 7. Release process — cross-platform [planned]

- Extend `bump_version.py` `TARGETS` to include the Windows build's VERSION placeholder so **all**
  artifacts (`__init__.py`, `pyproject.toml`, `gui/Cargo.toml`, `build_macos_dmg.sh`,
  `build_windows.ps1`) move together.
- **One tag → one GitHub Release with both assets.** Either (a) a documented two-box manual flow
  (macOS box builds+notarizes the DMG; Windows box builds+signs the installer; a single
  `gh release create vNEW Capture-<v>.dmg CaptureSetup-<v>-x64.exe` attaches both), or (b) a GitHub
  Actions matrix — `macos-latest` (DMG + notarize) and `windows-latest` (installer) — gating the
  publish step on **both** jobs. `windows-latest` is sufficient because CUDA is not bundled.
- Update `.claude/skills/capture-release/SKILL.md` to the two-artifact flow (Step 3 split into
  macOS-DMG and Windows-installer; Step 5 attaches both assets).

## Invariants & constraints

- **No change to `/v1`, the helper PCM contract, or the session-dir layout** across platforms.
- **Logon task, never a Service** (interactive WinSta0).
- **The GUI must compile and run on Windows.** Every macOS-only call must be `#[cfg]`-gated or
  cross-platform: the CoreGraphics screen-permission FFI, `osascript` file picker, `open` for
  folder/URL, `x-apple.systempreferences:` deep links, the `CaptureBar --request-mic` spawn, and
  `std::os::unix::process::CommandExt::process_group` (a **hard compile error** on Windows). Tracked
  in [gui.md](gui.md) / [agent-windows.md](agent-windows.md). This is the gating compile constraint
  for everything downstream.
- **Weights and CUDA are never bundled** (product-architecture.md): the ASR runtime ships, weights
  download on demand into the HF cache; CUDA is on-demand with CPU/remote fallback ([asr.md](asr.md)).
- **One version everywhere** via `bump_version.py`; never bump for a local build (capture-release rule).
- **Stable install path + app id** across updates (Windows has no TCC, but a stable identity matters
  for winget upgrade, the uninstall entry, and SmartScreen reputation accrual).
- **`CREATE_NO_WINDOW` on every background spawn** (daemon, helper) — a stray console window steals
  foreground and pollutes whole-screen captures.

## Failure modes & handling

- **Non-interactive station** → `EnumWindows` returns 0 user windows; the preflight surfaces a
  diagnostic + points to `run_interactive.ps1` (never capture a blank desktop silently).
- **No NVIDIA GPU / no CUDA wheels** → faster-whisper **CPU `int8`**, or a configured remote
  `openai-compat`/Riva endpoint, or the screenshots-only `minimal` install — never a hard failure
  ([asr.md](asr.md)).
- **Updater interrupted mid-install** → rollback from the pre-update backup; a corrupt install is
  caught on next start (health fails) → offer re-download.
- **SmartScreen on first installer run** → documented Run-anyway; unsigned auto-update may re-prompt
  per release until reputation builds.
- **`pyaudiowpatch` missing** → `Win32AudioSource.command(source="app")` returns `None` (no app
  audio). The installer must vendor/include it (it is **not on PyPI**); the eventual native Rust
  loopback helper (#34/#21 refinement) supersedes it.
- **Installer elevation** only for a per-machine install; the default per-user install avoids UAC.

## Outputs / artifacts

- `dist/CaptureSetup-<v>-x64.exe` (installer) + the installed tree (§Public contract); a winget
  manifest; a GitHub Release carrying **both** OS assets under one tag. User data under
  `%USERPROFILE%\.capture` (`daemon.json`, `runs/`, `config.json`) and the HF model cache.

## Configuration

- **New (signing):** `CAPTURE_WIN_SIGN_CERT` / `CAPTURE_WIN_SIGN_PASSWORD` /
  `CAPTURE_WIN_SIGN_THUMBPRINT` / `CAPTURE_WIN_SIGN_TIMESTAMP_URL` — drive the `signtool` hook in
  `build_windows.ps1`. Unset = unsigned build.
- **Shared:** `CAPTURE_GUI_VERSION` (bundle version), `CAPTURE_DAEMON_BIN` (bundled-daemon override),
  `CAPTURE_DAEMON_JSON` (discovery file).
- **Engine (unchanged):** `CAPTURE_PLATFORM`, `CAPTURE_WHISPER_MODEL`/`_DEVICE`/`_COMPUTE`,
  `CAPTURE_DSHOW_AUDIO`, `CAPTURE_OPENAI_ASR_URL`, `CAPTURE_RIVA_*` (see [asr.md](asr.md)).

## Known limitations / open items

Live backlog for the Windows release (tracked: `features.json` #34, #36):
- **Per-process audio** is the native-helper work (#34 / the #21 refinement): until it lands,
  Windows captures the **system output mix** only (`audiocap_win.py`), so other audio should be muted
  for a clean transcript. See [platform-abstraction.md](platform-abstraction.md).
- **Mic device enumeration** returns `[]` on Windows (no `--list-mics` analog); needs WASAPI
  enumeration so the GUI mic selector (#37) works on Windows.
- **`pyaudiowpatch`** is a non-PyPI third-party fork — vendoring / a maintained-fork contingency, or
  replacement by the native Rust loopback helper.
- **`~/.capture`** works but is non-idiomatic on Windows (`%APPDATA%`/`%LOCALAPPDATA%`); a
  platform-aware data dir is a low-priority cleanup.
- **Windows-on-ARM** is out of scope (x64 only for v1).
- **GPUI DX11** build is unverified in CI.
- **Signing/cert decision** deferred — unsigned v1 with the `signtool` hook ready.
- **CUDA bundling** explicitly out; non-NVIDIA alternatives in [asr.md](asr.md).
- The **import-a-file** feature (`capture_import`) is macOS-only until a Windows extraction path.

## Tests

- **[current]** `tests/smoke.py` passes **20/20 on Windows** through the abstraction; live Sessions
  6–7 captured real windows and an 8-video YouTube playlist end-to-end (see
  [platform-abstraction.md](platform-abstraction.md) §Tests).
- **[planned] packaged-bundle acceptance** (manual checklist; no GPUI UI harness): install from
  `CaptureSetup-<v>-x64.exe` on a clean Windows 10/11 box → tray icon appears and persists across
  closing the window → `/v1/health` ok → `list_windows` returns real windows → a ~5 s self-test
  capture writes screenshots + a transcript → publish a `+1` test release → the in-app update
  downloads, installs silently, relaunches, and `/v1/health` reports the new version → a forced
  installer failure rolls back → `Install skill` lands in `%USERPROFILE%\.claude\skills\capture`.
