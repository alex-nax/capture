# Progress Log

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
