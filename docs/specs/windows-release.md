# Spec: Windows release (packaging, installer, daemon lifecycle, signing, auto-update)

_Status: **partially implemented**, 2026-06-17 — core portability, per-process audio, GUI usability, the
native tray agent, and the **Inno Setup installer** have landed + are verified on the Windows box (Phases
0–4); cross-platform **auto-update** is implemented and the **Windows updater is verified end-to-end**
(v0.2.5 → v0.2.6 silent in-place upgrade, 2026-06-17). Remaining blocker: on **Smart App Control**
machines the unsigned GUI binary is blocked at launch (see §5), so a signing/Store decision gates broad
distribution. Source of truth = the code; this
spec marks **[current]/[done]** vs **[planned]** per section. (`features.json` #34, with the tray agent
#36 in [agent-windows.md](agent-windows.md).)_

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
- `helper/audiocap_win_rs/` (Rust, `windows-rs`) → `audiocap_win.exe` — native **per-process** WASAPI
  loopback helper (the #34 audio refinement; landed + verified 2026-06-17). `Win32AudioSource` prefers
  it when a target pid is known; see [helper-contract.md](helper-contract.md).
- `agent/windows/` (Rust) → `Capture.exe` — the native tray agent (#36; landed + verified 2026-06-17),
  and `packaging/register_logon_task.ps1` — interactive logon-task registration. See
  [agent-windows.md](agent-windows.md).
- `packaging/captured_main.py` — the PyInstaller entry (shared; calls `multiprocessing.freeze_support()`).
- `packaging/build_windows.ps1` — the parallel of `build_macos_dmg.sh` (**landed + verified 2026-06-17**):
  builds GUI + agent + native helper, PyInstaller-freezes the daemon, stages the install tree,
  (optionally) signs, and compiles the Inno Setup script → `dist/CaptureSetup-<v>-x64.exe`.
- `packaging/capture.iss` — Inno Setup script (**landed**): per-user install, Start-Menu/desktop
  shortcuts, logon-task registration, uninstall + cleanup.

**[planned]**
- `packaging/winget/` — winget manifest referencing the GitHub-release `.exe`.
- `gui/src/update.rs` — **done**: cross-platform updater (macOS `.dmg` + Windows `.exe` silent install).
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

### 1. Prerequisites — core-portability fixes [done 2026-06-17]

Three shared-core spots leaked POSIX/macOS assumptions and gated a clean Windows daemon run; all
three are now fixed (verified on the Windows box: smoke 67/67 + a live `capture daemon start/status/stop`
round-trip):

- **`cli/__init__.py` `daemon start`** spawned `python -m capture_mcp.daemon` with
  `start_new_session=True` (POSIX-only). **Fixed:** branches on `sys.platform == "win32"` to use
  `creationflags=subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW` (no console flash,
  survives the parent), else `start_new_session=True`.
- **`vision_client._encode_image`** downscaled via `sips` only (raw-PNG fallback otherwise). **Fixed:**
  now chains `sips` (macOS, `_downscale_sips`) → **Pillow** (`_downscale_pillow`, lazy `PIL` import) →
  raw PNG. Multimodal indexing works on Windows; with Pillow installed the payload is downscaled JPEG,
  without it the raw-PNG fallback still works (Pillow is not yet a declared dep — see open items).
- **`import_media.import_file`** lazily imports the macOS `helper_path`. **Fixed:** a `sys.platform !=
  "darwin"` guard raises a clear `NotImplementedError` (capture_import is macOS-only until a Windows
  ffmpeg extraction path) instead of a confusing `ImportError`.

Already correct (no change): `audio.py` spawns the audio helper with `creationflags=_NO_WINDOW`;
`util.descendant_pids` guards on `os.name` (POSIX `ps` only); `Path.home()/".capture"` resolves to
`C:\Users\<user>\.capture` (works; idiomatic `%APPDATA%` is an open item).

### 2. Packaging / freeze [done 2026-06-17]

`packaging/build_windows.ps1` (parallel to `build_macos_dmg.sh`):
1. `cargo build --release` the GUI (`gui/`) → `capture-gui.exe`.
2. `cargo build --release` the tray agent (`agent/windows/`) → `Capture.exe`.
3. PyInstaller **onedir** freeze of `packaging/captured_main.py` → `captured\captured.exe`, **LEAN
   (#58): NO ASR engine bundled** — Windows hidden-imports (`platform.windows`, `asr.whisper_local`,
   `asr.openai_compat`, `asr.runtimes`, `indexer`, `frames`, `vision_client`, `import_media`),
   `--collect-all huggingface_hub` (model downloads), and **`--exclude-module faster_whisper`/
   `ctranslate2`/`mlx`**. The engine arrives later as a **runtime pack** the user installs (see
   [asr-runtimes.md](asr-runtimes.md); built by `packaging/build_runtime_packs.ps1`). `freeze_support()`
   in `captured_main.py` is required (multiprocessing).
4. `cargo build --release` the native audio helper (`helper/audiocap_win_rs/`) → `audiocap_win.exe`;
   copy it **and** `helper/audiocap_win.py` (fallback) beside the frozen daemon; bundle the `skill/`;
   embed a `.ico` + an application manifest; assemble the install tree (§Public contract).
5. **(optional) sign** every `.exe`/`.dll` (§5) if `CAPTURE_WIN_SIGN_*` is set.
6. Compile `packaging/capture.iss` (Inno Setup) → `dist/CaptureSetup-<v>-x64.exe`; sign the installer.
7. Best-effort ASR self-test (`captured.exe --asr-selftest`), non-fatal (like macOS).

**No ASR engine is frozen in (#58).** The installer ships lean; the user installs a **runtime pack**
matching their hardware (CPU / NVIDIA-CUDA / remote; AMD later) — see [asr-runtimes.md](asr-runtimes.md).
Packs are built by `packaging/build_runtime_packs.ps1` and **hosted as GitHub release assets**; the
daemon downloads + activates one (the keystone — a frozen daemon loading an external pack — is
validated). No silent CPU fallback. (This supersedes the earlier "bundle faster-whisper" plan.)

### 3. Installer — Inno Setup [done 2026-06-17]

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

### 4. Daemon lifecycle [agent + logon task done 2026-06-17; installer wiring planned]

At **runtime** the tray agent (`Capture.exe`, [agent-windows.md](agent-windows.md)) owns the daemon —
it spawns/adopts it and stops it on Quit (verified). At **logon** an **interactive logon task** starts
the agent (which spawns the daemon) — mirroring macOS launchd → `CaptureBar`.
`packaging/register_logon_task.ps1` registers/unregisters that task (`-AtLogOn`, `LogonType
Interactive`, no time limit; verified register→unregister, no admin); the installer will call it at
install/uninstall. **[planned]** a `capture daemon install|uninstall` CLI wrapper for the from-source
path (the launchd/systemd analogue, a [planned] item in [daemon.md](daemon.md)).

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
- **Smart App Control / WDAC blocks unsigned binaries outright — proven 2026-06-17.** On a box with
  Smart App Control **On** (`HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy` →
  `VerifiedAndReputablePolicyState=1`), Code Integrity **blocks** the unsigned `capture-gui.exe` at
  launch (`Microsoft-Windows-CodeIntegrity/Operational` event **3077**, SAC policy
  `{0283ac0f-fff1-49ae-ada1-8a933130cad6}`): the tray agent + daemon start, but the GPUI window never
  opens and the user gets a "blocked" toast. This is **not** SmartScreen (no Mark-of-the-Web — the
  install-dir files are local) and **cannot be bypassed locally** — SAC is non-configurable by design
  (no allowlist, no folder/file exception, no per-app "Run anyway"). Empirically ruled out on this box:
  a **self-signed cert** (Authenticode *Valid*, root imported into Trusted Root) → still 3077; a
  **Defender AV folder + process exclusion** → still 3077 (AV exclusions skip the *scanner*, not Code
  Integrity). Why the other binaries ran: ISG cloud *file-reputation* admits the ubiquitous PyInstaller
  bootloader (`captured.exe`) and the tiny Rust agent (`Capture.exe`), but the unique ~15 MB GPUI binary
  has no reputation → blocked. **Paths that actually pass SAC:** (a) turn SAC **Off** — free but
  *irreversible without a Windows reset*; (b) an **EV** code-signing cert — reliable, and clears
  SmartScreen cold-start too; (c) **Microsoft Store** distribution — Store-signed packages are
  SAC-trusted (a self-distributed MSIX signed with a self-signed cert is **not**); (d) a standard **OV**
  cert that organically accrues ISG reputation — slow and *not* a dependable SAC fix. The earlier
  dev-time `os error 4551` on Cargo **build-script** probes was the same SAC; workaround is building into
  an already-cleared `target/` so Cargo skips re-running cleared build scripts (hence the helper/agent
  build via the GUI's target dir on this box). **Updater note:** the in-app silent in-place upgrade
  itself works under SAC (the installer, agent, and daemon all ran) — SAC blocks specifically the
  unreputed GPUI window binary.

### 6. Auto-update — cross-platform [done 2026-06-17]

`gui/src/update.rs` is now cross-platform (was `.dmg`-only):
- `UpdateInfo.dmg_url` → `asset_url`; `check()` selects the asset by OS via `#[cfg(target_os)]`
  `asset_matches` (`.dmg` on macOS, `CaptureSetup*.exe` on Windows). `parse_semver` / latest-release
  query / the confirm-modal UX are unchanged and shared (`app.rs` reads only `info.version`).
- `download_and_install` splits into `install_macos` (existing bash/hdiutil flow) and **`install_windows`**:
  download the installer to `%TEMP%`, write a detached **PowerShell** updater (`UPDATER_PS1`), spawn it
  hidden (`CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`); the app then exits. The updater stops
  `Capture`/`capture-gui`/`captured`, runs `CaptureSetup…exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
  /SP-` (Inno upgrades the per-user install in place by AppId), then relaunches `Capture.exe`.
- **Verification needs a newer release to update *to*** — tested as part of the release flow (install
  version A → publish A+1 → the installed app offers + applies it). No throwaway version: the real
  next release is the update target.
- **Skill auto-update** already uses cross-platform `dirs` — only a Windows smoke check remains.
- **[planned] polish:** rollback on a failed installer exit (back up the install dir first).

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
  for everything downstream. **Status: DONE (Phase 0 + Phase 2, 2026-06-17)** — `process_group` (Phase
  0) and the file picker / folder / URL / privacy / mic-grant + graceful renderer (Phase 2) are all
  gated; the GUI builds and renders on Windows. Remaining GUI polish: an `.ico` tray glyph + the native
  tray agent (#36).
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
- **Per-process audio — native helper landed (dev, 2026-06-17).** `audiocap_win.exe`
  (`helper/audiocap_win_rs/`, WASAPI Process Loopback + process-tree) isolates one app's audio; verified
  capturing a playing process (rms ~2113) and through an integrated daemon capture (`audio.s16le` rms
  ~1526). `audiocap_win.py` (system mix) remains the fallback. Remaining: ship the **signed** exe in the
  installer; a multi-app A/B isolation check; mic enumeration. See
  [helper-contract.md](helper-contract.md) / [platform-abstraction.md](platform-abstraction.md).
- **Mic device enumeration** returns `[]` on Windows (no `--list-mics` analog); needs WASAPI
  enumeration so the GUI mic selector (#37) works on Windows.
- **`pyaudiowpatch`** is a non-PyPI third-party fork — vendoring / a maintained-fork contingency, or
  replacement by the native Rust loopback helper.
- **`~/.capture`** works but is non-idiomatic on Windows (`%APPDATA%`/`%LOCALAPPDATA%`); a
  platform-aware data dir is a low-priority cleanup.
- **Windows-on-ARM** is out of scope (x64 only for v1).
- **GPUI on Windows: compiles + renders (verified 2026-06-17).** gpui 0.2.2 builds on Windows (MSVC,
  ~2m) and creates its **DirectX** renderer + window in the interactive desktop session (smoke via
  `scripts/run_interactive.ps1` → `RENDERER_OK`). Two caveats: (a) the renderer needs an **interactive
  GPU session** — launched from a non-interactive/service shell it fails with
  `DXGI_ERROR_NOT_CURRENTLY_AVAILABLE (0x887A0022)` (the same WinSta0 requirement as screen capture;
  the logon-task / normal desktop launch satisfies it); (b) `gui/src/main.rs:59` `unwrap()`s renderer
  creation, so that failure currently panics — make it graceful (surface the interactive-desktop hint)
  before shipping. CI build/run is still TODO.
- **Signing/cert decision** deferred — unsigned v1 with the `signtool` hook ready. **Sharpened
  2026-06-17:** unsigned + self-signed + OV are all *not* dependable on Smart App Control machines (see
  §5 — proven: SAC blocks the unsigned GPUI binary and has no local override). The only reliable paths
  to SAC trust are an **EV** cert or **Microsoft Store** distribution; without one, fresh-Win11 SAC users
  are blocked while the SAC-disabled majority get only a one-time SmartScreen prompt.
- **CUDA bundling** explicitly out; non-NVIDIA alternatives in [asr.md](asr.md).
- The **import-a-file** feature (`capture_import`) is macOS-only until a Windows extraction path.

## Tests

- **[current]** `tests/smoke.py` passes **67/67 on Windows** (was 20/20 pre-V2; the darwin-only
  helper-path test is skipped) through the abstraction; live Sessions 6–7 captured real windows and an
  8-video YouTube playlist end-to-end (see [platform-abstraction.md](platform-abstraction.md) §Tests).
- **[current, Phase 0, 2026-06-17]** On the Windows box: smoke 67/67 after the core-portability fixes;
  a live `capture daemon start/status/stop` round-trip via the new Windows spawn branch; the GPUI app
  builds (`cargo build`) and **renders** (window + DirectX, `RENDERER_OK`) in the interactive session.
- **[current, Phase 1, 2026-06-17]** Native per-process audio helper (`audiocap_win.exe`) verified:
  standalone capture of a playing process (rms ~2113, correct `READY`) and an **integrated daemon
  capture** (`capture start --pid … --no-screenshots`) wrote a non-silent `audio.s16le` (rms ~1526) via
  `Win32AudioSource` → native helper. `command()` returns the native helper for `app`+pid and the
  (path-fixed) Python system-loopback fallback otherwise. smoke 67/67.
- **[current, Phase 2, 2026-06-17]** GUI runtime macOS-isms `#[cfg]`-gated for Windows (file picker →
  PowerShell `OpenFileDialog`, folder reveal → `explorer`, privacy → `ms-settings:`, mic-grant →
  Settings deep-link, launch help text per-OS) and `main.rs` renderer creation made **graceful** (logs +
  clean exit, no panic). GUI rebuilds clean and re-renders (`RENDERER_OK`) in the interactive session;
  macOS paths are unchanged (cfg-gated).
- **[current, Phase 3, 2026-06-17]** Native tray agent (`agent/windows/` → `Capture.exe`) verified in the
  interactive session: stays resident, **adopts** a running daemon (no double-spawn), and launches one
  `capture-gui.exe` (`CAPTURE_AGENT=1`) — `agent_alive=true, gui_count=1, daemon_running=true`.
  `register_logon_task.ps1` register/verify/unregister round-trips clean. Tray icon/menu visuals are a
  manual check.
- **[current, Phase 4, 2026-06-17]** `build_windows.ps1` produced **`CaptureSetup-0.2.5-x64.exe`
  (74 MB)** — PyInstaller freeze (faster-whisper CPU, no CUDA libs) + Inno compile (67 s). Verified
  end-to-end: silent install lays out the full tree (agent + GUI + frozen daemon + native helper + skill
  + logon-task script), the **frozen daemon runs** and serves `/v1/health` (`version=0.2.5,
  platform=win32` — SAC does **not** block it), and the uninstaller cleans up. (Built from DEBUG binaries
  via `CAPTURE_WIN_DEBUG=1`; a release build is the same script with the flag off.)
- **[planned] packaged-bundle acceptance** (manual checklist; no GPUI UI harness): install from
  `CaptureSetup-<v>-x64.exe` on a clean Windows 10/11 box → tray icon appears and persists across
  closing the window → `/v1/health` ok → `list_windows` returns real windows → a ~5 s self-test
  capture writes screenshots + a transcript → publish a `+1` test release → the in-app update
  downloads, installs silently, relaunches, and `/v1/health` reports the new version → a forced
  installer failure rolls back → `Install skill` lands in `%USERPROFILE%\.claude\skills\capture`.
