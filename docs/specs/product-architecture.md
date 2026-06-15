# Spec: Product architecture (GUI, distribution, multi-OS)

_Status: current as of 2026-06-10. Source of truth = the code; update this spec in the same change as the code._

> **Decision record + plan.** Unlike the per-scope specs, most of this spec describes
> **planned** behavior — the agreed target architecture for evolving capture-mcp from an
> agent-only MCP server into an installable product with a native GUI. Sections explicitly
> mark **[current]** (true of the code today) vs **[planned]** (not yet implemented).
> Decided 2026-06-09/10 via a multi-agent design study (3 proposals × 3 judge lenses +
> completeness critique); the losing alternatives and why are summarized under *Behavior*.

## Purpose

Make capture-mcp usable by a normal human — easy install, native GUI, macOS + Windows +
(eventually) Linux — **without** demoting the agent path. Fixed product constraints from
the owner:

- **No web UI of any kind.** No Electron, Tauri, pywebview, webviews, or localhost-browser
  dashboards.
- **The GUI is built with GPUI** (Zed's Rust GPU-accelerated UI framework). Fixed choice;
  design around it.
- **MCP stays first-class.** The existing `capture_start`/`capture_stop`/`capture_status`
  tools keep working unchanged for agents, including headless/CI environments.

## Files

**[current]** M0a (feature #25, 2026-06-10) split the package: the engine lives in
`src/capture_mcp/core/` (`session.py`, `screenshots.py`, `audio.py`, `proc.py`,
`platform/`, `asr/`, plus the new `registry.py` — `SessionRegistry` with disk-backed
history, see [session-registry.md](session-registry.md)); `server.py` is a thin MCP
frontend. `core/` imports no frontend code.

**[planned]** Target layout (names indicative):

- `capture_mcp/core/` — **[current]** engine + `registry.py` + `events.py` (EventBus +
  per-session `events.jsonl`, M0b); **[planned]** `permissions.py` (preflight, M2).
- `capture_mcp/daemon/` — **[current, #32 slice 1]** `captured`: stdlib HTTP `/v1` API on
  127.0.0.1 + bearer token (see [daemon.md](daemon.md)). **[planned]** UDS + WebSocket events.
- `capture_mcp/server.py` — thin MCP server, **[current, #32]** daemon-first with embedded fallback (`CAPTURE_MCP_EMBEDDED=1`).
- `capture/cli/` — `capture` CLI (start/stop/status/tail/doctor/daemon install).
- `gui/` — Rust GPUI app (separate cargo workspace in this repo).
- `packaging/` — PyInstaller specs, signing/notarization, installer definitions.

## Public contract

**[current]** The three MCP tools and their schemas (see `mcp-server.md`); the audio helper
PCM contract (see `screencapturekit-helper.md`); `session.json` layout (see `session.md`).
These are frozen interfaces that every layer below builds on.

**[partly current — see [daemon.md](daemon.md))**

- `/v1` local API (versioned, additive-only within a major). **[current, #32]**:
  `POST /v1/sessions`, `POST /v1/sessions/{id}/stop`, `GET /v1/sessions[/{id}]`,
  `GET /v1/sessions/{id}/transcript?tail=N`, `GET /v1/windows`, `GET /v1/health`,
  `GET /v1/events` (**SSE** fan-out of the EventBus: state, `screenshot_taken`,
  `transcript_segment`, `log_line`, `audio_status` — see [daemon.md](daemon.md)),
  `POST /v1/admin/shutdown`. **[planned]**: `/v1/sessions/{id}/preview` (latest frame),
  `POST /v1/permissions/screen_recording/request`, `POST /v1/asr/preload`,
  `POST /v1/sessions/{id}/retranscribe`. (Event transport is SSE, not WS — one-way fan-out.)
- Endpoint discovery: `~/.capture/daemon.json` `{endpoint, token, pid, api_version}`,
  mode 0600, written by the daemon; bearer token required; UDS peer-uid checked.
- Contract firewall: **[current, #32]** the daemon's `/v1` shapes are pydantic models
  (`daemon/models.py`) that emit JSON Schema (`v1_schema()` / `GET /v1/schema`), pinned by
  a golden contract test that also round-trips live responses through the models.
  **[planned]** generate the GPUI app's Rust types from that schema (typify).
- **[current]** MCP tool `list_windows` (#29, 2026-06-10): agents share the engine's
  window picker (`core.list_windows()`), the same function `/v1/windows` will wrap.

## Behavior

**Architecture decision [planned]: daemon-peers.** The engine runs as one signed,
launchd/Task-Scheduler/systemd-user-managed daemon (`captured`); the GPUI app, the MCP
server, and the CLI are thin peer clients of the same live engine. Chosen over (a) a
GUI-owned Python sidecar — rejected because quitting the GUI would kill a live meeting
capture and the agent TCC pain would persist until the last milestone — and (b) a
progressive Rust port of the engine — rejected for now as a ~2× rewrite premium with
parity risk on hard-won platform behavior; it remains a contract-preserving *option*
behind the stable `/v1` API.

- **Sessions outlive clients [planned]:** the daemon owns the SessionRegistry; an
  agent-started capture is visible live in the GUI; GUI restarts don't touch captures.
- **One TCC grant for everyone [planned]:** the daemon is the TCC-responsible process
  that spawns `audiocap`, so the user grants "Capture" Screen Recording once and the GUI,
  every terminal's MCP agent, the CLI, and cron all inherit working audio. This dissolves
  the documented worst pain (`permissions-and-signing.md`: grant must cover the launching
  process). **Gate: PASSED (#30, 2026-06-15, macOS 26.5.1/arm64).** A launchd user-agent →
  signed `.app` PyInstaller daemon → `audiocap` chain streamed audio with no terminal grant;
  the grant **survived a same-identity update** (rebuild + re-sign, `respawns=0`, no re-prompt)
  and was **lost on identity rotation** (negative control), confirming it keys to the signing
  identity. Evidence: `spike/tcc-attribution/results/FINDINGS.md`.
- **Embedded fallback [current, #32]:** the MCP server proxies its tools to a running
  daemon and, with no daemon present (headless/CI, or `CAPTURE_MCP_EMBEDDED=1`), runs the
  engine in-process exactly as before. See [mcp-server.md](mcp-server.md) (daemon-first
  dispatch) + [daemon.md](daemon.md).
- **Live preview [planned]:** screenshot events carry `{path, seq, ts}` (daemon and GUI
  share a filesystem); the GUI decodes off-thread and renders via `RenderImage` — not
  URI-cached `img()`, which would leak the image cache over an hours-long 1 fps run.
- **GPUI pinning [planned]:** pin a frozen zed git rev (crates.io 0.2.2 predates the wgpu
  Linux renderer and AccessKit) + a compatible `gpui-component` rev; `cargo vendor`;
  scheduled quarterly bump. Tray/menu-bar + global hotkey via `tray-icon` + `muda` +
  `global-hotkey` (the proven standalone-GPUI combo). GUI stays thin: all logic lives in
  the daemon so pre-1.0 churn is contained to the UI layer.

## Invariants & constraints

- **[current]** Capture logic is single-source Python. No capture/ASR logic may be
  duplicated in Rust; the Rust side is UI + API client only. (A future Rust engine must
  replace the daemon *behind the unchanged `/v1` API*, never fork it.)
- **[current]** The helper contract (argv; 16 kHz mono s16le PCM on stdout; status on
  stderr; `-3801`/`-3803` fatal vs `-3805` transient-reconnect) is frozen shared property.
  Windows per-process loopback (#21 refinement) must be a **native helper binary**
  speaking this same contract — async-COM `ActivateAudioInterfaceAsync` from Python
  ctypes is impractical, and Chromium-family apps need process-**tree** loopback (window
  PID ≠ audio-rendering PID).
- **[confirmed #30]** TCC persistence keys on the **code-signing identity + stable bundle
  identifier** (the designated requirement), not the cert serial or path — the spike confirmed
  the grant survives a same-identity rebuild and is lost on identity rotation. So a **stable
  signing identity across updates is mandatory**: ship the engine/daemon/helper with a Developer
  ID cert (stable Team ID + `CFBundleIdentifier`) and never rotate it casually (a change
  re-prompts every user). App, embedded daemon, and helper need deliberate `CFBundleIdentifier`s
  and Info.plists (a bare PyInstaller binary surfaces in System Settings under its file name).
- **[#30 follow-up]** On **macOS 26**, `SCShareableContent` enumeration is intermittently flaky
  (spike saw `audiocap` `exit 5` "enumeration failed" interleaved with healthy audio; the daemon's
  respawn loop rode through it). The real `audiocap.swift` should add a **bounded retry** on the
  enumeration failure instead of exiting, so the helper doesn't depend on a supervisor restart.
- **[planned]** Windows daemon is a **logon task, never a Service** — capture requires the
  interactive WinSta0 desktop (see `windows.md` / `run_interactive.ps1`).
- **[planned]** The GUI must never be required: every capability lands in daemon/CLI/MCP
  first or simultaneously. Headless Linux engine must not link GPUI (no Vulkan requirement
  in the engine artifact).
- **[current]** From-source dev path (`init.sh`, `setup_codesign.sh` self-signed identity)
  remains supported alongside packaged installs.

## Failure modes & handling

**[planned — designed, to be implemented with their milestones]**

- **Daemon lifecycle:** stale `daemon.json`/socket after crash → single-instance flock +
  `/v1/health` handshake before any client call; two daemons after brew + DMG
  double-install → same flock + `capture doctor` diagnosis; version skew → semver'd
  `api_version`, launchd plist/task points *inside* the app bundle so app updates carry
  the matching daemon; client offers "Restart capture engine".
- **Live update of a running daemon:** replacing the bundle on disk invalidates the
  running process — updates need stop→swap→restart choreography and a defined story for
  in-flight captures (finish-then-restart). Undesigned today; must be specced with the
  auto-update milestone, **before** v1 ships to non-technical users.
- **macOS 15+ periodic re-approval:** Sequoia re-prompts "still wants to record your
  screen" for programmatic SCK use; the nag attributes to the daemon. Needs designed
  re-approval UX (and evaluation of `SCContentSharingPicker` flows). "Grant once, forever"
  must not be promised in any user-facing copy.
- **Windows signing reality:** Azure Trusted Signing is restricted to organizations with
  ≥3 years history (individuals waitlisted); OV certs (~$200–500/yr, HSM-token custody)
  do **not** clear SmartScreen cold-start. v1 Windows ships with SmartScreen warnings and
  honest install docs; revisit when eligibility changes.
- **Wayland is semantically different:** the ScreenCast portal cannot target a window by
  app name (picker only, persistent share indicator, single-use restore tokens that must
  be re-persisted per session). `app_name` attach mode degrades on Wayland and the MCP
  tool docs must say so.

## Outputs / artifacts

**[planned]** Per release: macOS per-arch DMGs (+ Homebrew cask) containing the GPUI app,
the PyInstaller-onedir daemon, and the **prebuilt Developer-ID-signed `audiocap`** (end
users never need Xcode CLT); Windows Inno Setup x64 installer (+ winget manifest); Linux
tar.gz + installer script (AUR, Flathub later). PyPI/`uvx capture-mcp` continues for the
agent/dev path. Model weights are **never** bundled: ASR runtime ships, weights download
on demand (resumable, SHA256-verified, honest sizes, disk-space preflight) into the
standard HF cache; CUDA DLL pack on Windows is an on-demand download, not part of the
installer.

## Configuration

**[current]** Engine configured by env vars (`CAPTURE_WHISPER_MODEL`, `CAPTURE_RIVA_*`,
`CAPTURE_PLATFORM`, `CAPTURE_DSHOW_AUDIO`, …) and per-call `capture_start` args;
`output_dir` is a required per-call argument — there is **no machine-wide capture root or
session index**.

**[planned]** A config-file layer (location, schema version, migration) shared by daemon
and GUI, with explicit precedence: per-call args > env vars > config file > defaults.
Env vars keep working for the agent path. A default capture root + registered-roots
session index is required for GUI history (the "scan `capture-*/session.json`" plan
assumes a single root that doesn't exist today); includes retention policy, per-session
delete/reveal, and disk-budget surfacing (1 fps PNG + raw PCM ≈ 0.5–2 GB/hour).

## Known limitations / open items

Live backlog for this scope (roadmap features #25–#35 in `features.json`):

- **M0a done 2026-06-10** (#25): package split, SessionRegistry + disk-backed
  history (`CAPTURE_SESSION_INDEX`), `start()` lock fix (`"starting"` state).
- **M0b done 2026-06-10** (#26): EventBus + per-session `events.jsonl`
  (state transitions + counter snapshots; see [events.md](events.md)).
- **M0c done 2026-06-10** (#27): `tests/contract/` (tools/list, session-dir layout,
  PCM chunk-math goldens + `--regen`), frozen [helper-contract.md](helper-contract.md),
  `audiocap.swift` READY comment fixed (scan stderr, not line 1), `audiocap_win.py`
  shutdown NameError fixed.
- **Done 2026-06-10** (#28): `asr/openai_compat.py` (stdlib-only) + `minimal` extra —
  any `/v1/audio/transcriptions` endpoint (incl. the Nemotron WSL2 lab) is a plain
  remote backend via `CAPTURE_OPENAI_ASR_URL`.
- **Done 2026-06-10** (#29): `list_windows` MCP tool over `core.list_windows()`.
- **PASSED 2026-06-15** (#30): TCC attribution spike ran on macOS 26.5.1/arm64 —
  launchd→signed-bundle daemon owns the grant, persists across same-identity update,
  lost on identity rotation. Unblocks #31. Evidence + verdict:
  `spike/tcc-attribution/results/FINDINGS.md`; distro on the `tcc-spike` branch.
- **M1** packaged signed engine, no GUI: PyInstaller + Developer ID + notarization +
  prebuilt helper + `capture doctor` + brew tap (#31).
- **M2** `captured` daemon + `/v1` + CLI; MCP daemon-first mode (#32).
- **M3** GPUI app v1 on macOS, onboarding ends with a visible 5-second self-test capture (#33).
- **M4** Windows release; per-process loopback native helper closes the #21 refinement (#34).
- **M5** Linux: engine backends (X11 → Wayland portal, PipeWire per-app audio with a
  reconnect-tracking helper), then GUI (#35).
- Undesigned, must be specced before the corresponding milestone ships: auto-update
  choreography; uninstall/cleanup (login items, model caches, stale `.mcp.json`);
  migration of existing from-source users (duplicate TCC entries); audio export/playback
  (`audio.s16le` is unplayable raw PCM); sleep/lock behavior during long captures (power
  assertion); privacy/consent posture (local-only statement, recording-indicator
  guidance); crash reporting; GUI testing story (GPUI has no public UI-test harness);
  Windows-on-ARM matrix; model-preset naming unified across mlx/CT2/ggml backends.

## Tests

**[current]** Hermetic smoke suite (`tests/smoke.py`, 20/20) covers the engine; nothing
covers MCP-protocol or packaging.

**[planned]** `tests/contract/`: golden MCP `tools/list` schema dump; recorded StubASR
session-dir fixture (`session.json` + `transcript.jsonl` + `events.jsonl` layout); PCM
chunk/offset fixtures for the 8 s-window logic — the regression gate for the M0 refactor
and every later layer. JSON-Schema round-trip contract tests (pydantic ⇄ generated Rust
types) in CI. Per-release packaged-bundle smoke: run a real capture from the installed
artifact on all three OSes. TCC spike (#30) documented as a repeatable checklist, not CI.
