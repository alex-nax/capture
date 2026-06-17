# Progress Log

## Session 56 — 2026-06-17
**Agent**: builder (**Windows box**, branch **windows-support**) — **Phase 0** of the Windows port:
core-portability fixes + make the GUI build/run on Windows. Verified on this box.
- **Python core-portability (3 fixes)**: `cli/__init__.py` `daemon start` now branches
  `start_new_session` (POSIX) vs `creationflags=CREATE_NEW_PROCESS_GROUP|CREATE_NO_WINDOW` (Windows);
  `vision_client._encode_image` chains `sips` → **Pillow** (`_downscale_sips`/`_downscale_pillow`, lazy
  PIL) → raw-PNG; `import_media.import_file` raises a clear `NotImplementedError` on non-macOS (no more
  confusing lazy-`platform.macos` ImportError). Corrected the earlier audit: import_media's macOS import
  was already lazy (no daemon-import crash) and vision already had a raw-PNG fallback.
- **GUI compile/run (Rust)**: `gui/src/daemon.rs` `spawn_detached` is now `#[cfg]`-branched (unix
  `process_group(0)` vs Windows `creation_flags(0x0800_0200)`) — this was the one **hard compile error**
  on Windows; `bundled_daemon`/`skill_source` paths are per-OS for the planned Windows layout
  (`captured\captured.exe`, `skill\` beside `capture-gui.exe`). The CG screen-perm FFI was already
  `#[cfg(target_os="macos")]`-gated.
- **Verified on Windows (this box, Python 3.12 venv + Rust 1.95 MSVC)**: smoke **67/67**; live
  `capture daemon start → status(running, platform=win32) → stop`; `cargo build` of the GUI clean
  (~2m20s, gpui 0.2.2 + DirectX); the GUI **renders** (window + DirectX renderer, `RENDERER_OK`) when
  launched in the interactive desktop via `scripts/run_interactive.ps1`. From a NON-interactive shell
  the GUI renderer fails with `DXGI_ERROR_NOT_CURRENTLY_AVAILABLE (0x887A0022)` — the documented WinSta0
  requirement (the daemon needs no GPU). `gui/src/main.rs:59` `unwrap()`s renderer creation → make it
  graceful before shipping (noted in windows-release.md).
- **Finding**: gpui 0.2.2 uses a **DirectX** renderer on Windows (so #34's "DX11 backend" is right;
  blade-graphics is just a compiled dep). Specs synced (windows-release.md §1 → done + §Tests/open-items,
  platform-abstraction.md, asr.md, daemon.md).
- **Next (Phase 1/2)**: per-process audio native helper (#21/#34); GUI runtime macOS-isms (file picker
  → rfd, open folder/URL → explorer/start, privacy deep-link → ms-settings, mic-grant) for functional
  parity; then packaging (Phase 4) + auto-update/release (Phase 5).

---

## Session 55 — 2026-06-17
**Agent**: builder (**Windows box**, new branch **windows-support**) — full-cycle Windows support:
audit + planning + **specs-first** (no code yet, per request).
- **Branch**: cut `windows-support` from `main`. Reconciled state: this box's `main` already carries the
  V2 + Windows groundwork (12 unpushed commits from `162222a V2 … multi-OS groundwork`); `origin/v2` has
  nothing `main` lacks; the macOS box's 0.2.x sessions (40–54) are committed only on **that** machine and
  are NOT here.
- **Audit**: ran a 9-investigator parallel workflow over every subsystem (backends, GUI, tray agent,
  packaging, auto-update, release/CI, ASR, roadmap, core portability). Verdict: engine layer ~done +
  live-verified (GDI+/EnumWindows/WASAPI loopback + faster-whisper CUDA captured an 8-video playlist
  end-to-end); the gap is the **last mile** — installer, signing/SmartScreen, daemon lifecycle, native
  tray agent (#36), GUI macOS-isms (won't compile on Windows: `process_group`, CG FFI), and a
  Windows auto-update path.
- **Decisions locked (owner)**: Inno Setup + winget; **logon task, never a Service**; **Rust** tray
  agent (windows-rs + tray-icon); **don't bundle CUDA** but provide CPU-int8 / remote openai-compat /
  Riva / minimal alternatives for non-NVIDIA boxes; **unsigned v1** with a `signtool` hook
  (`CAPTURE_WIN_SIGN_*`) — SmartScreen fires once on the downloaded installer's first run, never on
  captures or post-install launches.
- **Specs written**: NEW `docs/specs/windows-release.md` (packaging/freeze, Inno Setup installer +
  winget, daemon logon-task lifecycle, interactive-desktop preflight, signing/SmartScreen, cross-platform
  auto-update, release/CI) and `docs/specs/agent-windows.md` (Rust tray agent, #36). Updated `asr.md`
  (non-NVIDIA alternatives + planned platform-aware catalog / CUDA preflight / `/v1/asr/backend`),
  `platform-abstraction.md` (per-process native helper plan, mic enumeration, core-portability leaks,
  pointers), `windows.md` (disambiguation: it's the macOS GUI-window module), `docs/specs/README.md`
  (two new rows + clarified the `windows.md` row), and `features.json` (#34/#36 → spec pointers +
  decisions + auto-update/non-NVIDIA/core-portability acceptance criteria). `features.json` validates
  (55 features).
- **Corrected two audit overstatements against the real code**: `import_media` imports the macOS helper
  **lazily inside the function** (macOS-only feature, does NOT crash daemon import on Windows);
  `vision_client._encode_image` **falls back to the raw PNG** when `sips` is absent (indexing works on
  Windows, just with fatter payloads). Confirmed real prereqs: `cli/__init__.py:53`
  `start_new_session=True` (POSIX-only) and `Win32AudioSource.command(source="app")` → `None` without
  pyaudiowpatch (system-loopback only).
- **Next (Phase 0, when greenlit)**: fix the three core-portability leaks + `#[cfg]`-gate the GUI
  macOS-isms so it compiles, then verify the daemon + a real capture on this box. No release/version
  bump (specs-first; nothing shipped).

---

## Session 54 — 2026-06-17
**Agent**: builder (macOS box, branch **v2**) — skill optimization + 0.2.2 deploy + local commit.
- **Skill (skill-creator):** rewrote `skills/capture/SKILL.md` Step 1 to the **app-first distribution**
  (Capture.app via GitHub Releases, auto-updating, owns the daemon + Screen Recording grant; MCP is
  daemon-first so it shares the app's daemon; `install.sh` remains for the MCP command / headless).
  Added a **"fix a wrong/hallucinated transcript"** cookbook recipe (§8) + the full tool set + pushier
  trigger description. Ran the skill-creator **description-optimization loop** (20 trigger/no-trigger
  queries, `claude -p`): negatives all correctly rejected (precision 100%), but it found **no improvement**
  over the hand-written description — the should-trigger queries are install/how-to/"fix it" asks that
  Claude answers directly without consulting a skill (a documented trigger-eval limitation), so recall
  pinned at 0% and `best_description` == current. Kept the hand-optimized description.
- **Deploy:** bumped to **0.2.2** (pyproject / `__init__` / Cargo / packaging), full re-freeze + GUI +
  helper, Developer-ID signed, **notarized (Accepted) + stapled**, installed to `/Applications`, launched.
  Verified live: daemon `version=0.2.2`, `/v1/asr/models` carries language+chunk, schema has `IndexRequest`
  + `SessionSummary.mic_device`. (No GitHub release, no git push — per request.)
- **Commit:** the whole #40–#48 batch + skill + 0.2.2 committed **locally** on `v2` (`drive_nolf.py`
  excluded). Not pushed.

---

## Session 53 — 2026-06-17
**Agent**: builder (macOS box, branch **v2**) — feature **#46** (switch the mic on a live capture).
- `AudioCapture` gained an **`append`** mode (open files `a`/`ab`). `CaptureSession.set_mic_device(device)`:
  on a running session, stop the `_mic` track + start a new one with the new device, appending to
  `mic.s16le`/`mic_transcript.*` (continuous; each capture's epoch is real wall-clock so iso timestamps
  align). `device`/`"default"` = on/switch, `None`/`""` = off. `mic_device` now in `summary()` + `SessionSummary`.
- Parity: `POST /v1/sessions/{id}/mic`, `DaemonClient.set_mic`, MCP **`capture_set_mic`** (11th tool), GUI
  live **Mic** chip row in the playback pane (running sessions) highlighting the active device.
**Verification**: daemon e2e — unknown→404, switch ON (mic.s16le created by the helper), switch OFF
(mic_status=off), switch on a stopped session→400. smoke 68/68, contracts 4/4 (golden: +`capture_set_mic`,
`SessionSummary.mic_device`). GUI `cargo build --release` clean. **feature #46 → passes:true.**
Also this session: **#47** + **#48** (below).

### #47 — bundled skill update + version check
Updated `skills/capture/SKILL.md` + `references/quick-actions.md` to document the full tool set
(`list_windows`, `list_audio_devices`, `capture_set_mic`, `capture_prune`, `capture_retranscribe`,
`transcription_settings`, `capture_import`, `capture_index`) + new `capture_start` options + the
transcription/hallucination guidance. The GUI's skill version-check was ALREADY hash-based
(`skill::status` → `UpdateAvailable` when bundled ≠ installed, shown "↑ update", refreshed at startup +
after install) — editing the bundled skill makes installed copies surface the update (verified: the edited
skill differs from the installed copy). DMG build already bundles it into `Resources/skill`. **#47 passes:true.**

### #48 — in-app GitHub auto-update (with confirmation)
`gui/src/update.rs`: `check()` GETs `repos/alex-nax/capture/releases/latest`, semver-compares `tag_name` to
`CARGO_PKG_VERSION`, finds the `.dmg` asset. Settings **App** row: `vX · up to date` / `vY available →
Update…`. Update → shared confirm modal (`ConfirmKind::Update`) → `download_and_install` (fetch notarized
dmg) → a **detached updater script** quits the app+daemon, mounts the dmg, replaces `/Applications/Capture.app`,
strips quarantine, relaunches. Never without confirmation. **Verified against the real API**: 0.2.0 vs
v0.2.0 → no offer; the `.dmg` asset URL resolves correctly. GUI builds clean. **#48 passes:true.**
NOTE: the update only *offers* once a release newer than the deployed build exists — cut a 0.3.0 release
(with the version bumped) to exercise it live.

---

## Session 52 — 2026-06-17
**Agent**: builder (macOS box, branch **v2**) — feature **#45** (transcription quality + settings).
**Trigger**: a Russian capture (session 0c5c5a) transcribed as 18× "Thank you." — Whisper hallucinating.
**Root cause** (diagnosed on the real audio): the app audio was fine (rms ~3000, clean Russian on a 30 s
re-chunk) but the live **8 s chunks** + auto-language made Whisper mis-detect short/pause chunks → phantom
"Thank you."/"Obrigado."/"Спасибо."; the **silent mic** (rms 43) looped "RSSSS…". Proven: 4 s/8 s/16 s
chunks hallucinate, **30 s is clean**.
**Fixes** (all GUI↔MCP↔daemon parity):
- Backend guards (mlx + faster): `condition_on_previous_text=False` + `no_speech`/`logprob`/`compression`
  thresholds.
- **Silence gate** `asr.is_silent` (int16-RMS < `SILENCE_RMS16` 70, env `CAPTURE_ASR_SILENCE_RMS`) — skip
  near-silent chunks in `audio.py` + `retranscribe.py` (offsets still advance).
- **Language** as a persisted, ON-THE-FLY setting (`manager.active_language`; resolved per `transcribe()`
  call so a running capture's next chunk picks it up). `POST /v1/asr/language`; client; GUI field (Settings +
  playback pane); MCP `transcription_settings` + `capture_retranscribe(language=…)`. NOT env.
- **Chunk length** as a persisted setting (default **30 s**, bounds 1–120; `manager.active_chunk_seconds`).
  `POST /v1/asr/chunk`; GUI chips; `StartSessionRequest.audio_chunk_seconds` default `None`→config;
  `CaptureSession` + `retranscribe` resolve from it; re-transcribe uses the CURRENT setting, not the old
  session's 8 s. `capture_retranscribe(chunk_seconds=…)`.
**Verification**: smoke 68/68 (updated to feed non-silent tone — the gate now skips silence), contracts 4/4
(goldens: +`transcription_settings`, `AsrModelsResponse.language`/`chunk_seconds`, `audio_chunk_seconds`
nullable). Daemon routes (set/clamp/validate/reflect language+chunk), MCP `transcription_settings`
(read/set/clear) verified. **Re-transcribed the real session 0c5c5a → 287 coherent Russian segments**
(garbage kept as `transcript.prev.*`); the user's active language is now `ru`. GUI `cargo build --release`
clean. **feature #45 → passes:true.** NOT yet redeployed — the installed app still runs the pre-#45 daemon,
so NEW captures need the rebuild to get the fix.
NOTE: the mic track for 0c5c5a was a genuinely dead mic (rms 43); its `mic_transcript` still holds the old
pre-gate garbage (re-transcribe only redoes the app `audio.s16le`), but the gate prevents it going forward.

---

## Session 51 — 2026-06-17
**Agent**: builder (macOS box, branch **v2**) — feature **#44** (hierarchical multimodal index).
**Summary**: A session's screenshots are captioned by a remote **LM Studio** vision LLM and the timeline
is summarized as a **binary tree** (vision at leaves → combine up to a root summary), full GUI↔MCP parity.
Built per the approved `docs/specs/indexing.md` decisions; **disabled unless a working endpoint is configured**.
- **`core/vision_client.py`**: stdlib-only OpenAI `/v1/chat/completions` client — `caption_image` (sips
  downscale→JPEG→base64 data URI, raw-PNG fallback), `combine` (text), `available()` (`/v1/models` preflight),
  retries. Env `CAPTURE_INDEX_URL/MODEL/KEY/TIMEOUT/MAX_IMAGE_PX` (+ per-request `endpoint`/`model` override).
- **`core/frames.py`**: list screenshots (fs_stamp→offset), `select_leaves` by tunable **sampling rate**
  (keep every `round(1/rate)`-th frame, default 0.5) + `max_leaves` cap.
- **`core/indexer.py`**: balanced binary tree by midpoint; vision-caption leaves, combine children up,
  **fuse the time-aligned transcript** (capped feed) into each combine; every node keeps **raw artifacts**
  (`vision_caption`, `transcript_slice`) beside the fused `summary`. Writes `index.json` + `index_summary.txt`,
  **checkpointed per node** (resume reuses done nodes). `load_index`. `can_index` added to `session_capabilities`.
- **Daemon**: `IndexRequest`, `start_index` (background + SSE `index`→`index_done`/`_error`),
  `POST /v1/sessions/{id}/index`, `GET …/index`, `GET /v1/index/status` (availability probe); 503 when the
  endpoint is unset/unreachable. `DaemonClient.index/get_index/index_status`. MCP `capture_index` (9th tool).
- **GUI**: Settings **Index endpoint** URL + **model** fields (persisted `index_url`/`index_model`) + a
  reachability dot (slow separate `index_status` poll, since the probe can time out); playback Manage
  **Build index** button (`list-tree` icon, gated on `can_index` + `index_status.available`); SSE progress →
  `LiveState.index_progress`; the built index's **root summary + node count** render in Manage (via `get_index`).
  A model field is needed because a box can have several models loaded (the user's has qwen + gemma + an embedder).
**Verification**: `tests/indexing_hermetic.py` (18 checks — tree 2n-1, vision-only-at-leaves, transcript
fusion, raw artifacts, 8 vision + 7 text calls, **checkpoint resume recomputes only the missing node**);
daemon e2e (fake endpoint — `index/status` available, `can_index`, POST 202, GET index complete, audio-only → 400).
**LIVE-VERIFIED against the real `qwen/qwen3.5-9b` on LM Studio @ 192.168.31.217:1234**: a 12 s screen recording
→ import (6 frames) → index → 6 accurate captions + a coherent root summary (11-node tree, ~230 s); and a
**model-in-request-body** run (the exact GUI path, no env) built with `model=qwen/qwen3.5-9b`. NOTE: model ids
carry a publisher prefix (`qwen/qwen3.5-9b`). smoke 68/68, contracts 4/4, GUI `cargo build --release` clean.
**feature #44 → passes:true.** Not yet deployed to the app (no re-freeze this round) — the new index modules
are in the PyInstaller hidden-imports for the next build.

---

## Session 50 — 2026-06-17
**Agent**: builder (macOS box, branch **v2**) — features **#42** (screenshot toggle) + **#43** (import
audio/video), plus a new **ispec #44** (remote multimodal indexing) queued for later.
**Summary**: Two capture-flexibility features, full GUI↔MCP parity, verified on the **frozen** artifact.
- **#42 — screenshot toggle (audio-only capture)**: the daemon already accepted `capture_screenshots`;
  added a GUI **Screenshots On/Off** chip in Settings→Capture quality (`capture_screenshots` field,
  persisted in `gui-settings.json`, merged into the start body via `shot_settings()`). Off ⇒ no `screenshots/`.
- **#43 — import a file as a session**: new helper modes `audiocap --extract-audio <file>` (AVAssetReader →
  s16le on stdout, exit 3=no-audio-track/4=read-fail) and `--extract-frames <file> --out <dir> --interval`
  (AVAssetImageGenerator → `<ms>.png`); linked ImageIO+UniformTypeIdentifiers. `core/import_media.py`
  (`import_file`: mint id at import-time epoch, extract audio→`audio.s16le`, frames→`screenshots/` renamed to
  `fs_stamp(base+ms/1000)` so frames+subtitles share one timeline, write session.json `audio_source="import"`,
  reuse `retranscribe_session` for ASR; a **silent video imports as frames-only**, audio-only file ⇒ no frames).
  `registry.add_recovered`; `daemon.start_import` (background+SSE `import`→`import_done`/`_error`),
  `POST /v1/sessions/import`, `ImportMediaRequest`, `DaemonClient.import_media`, MCP `capture_import` (8th tool),
  GUI **Import…** row (native `osascript` file picker off-thread → `import_media`; SSE → `LiveState.import_progress`).
- **#44 ispec (queued, not built)**: `docs/specs/indexing.md` — hierarchical binary-tree multimodal index of a
  session's screenshots via a remote **LM Studio** vision LLM (Qwen, OpenAI-compatible) on `192.168.31.217:1234`.
  Approved decisions: tunable **index sampling rate** (0<rate≤1, default 0.5) + configurable capture interval;
  **vision at leaves, combine up**; **fuse transcript but keep raw per-node artifacts**; plain-text nodes;
  resumable background job + SSE; **DISABLED by default unless a working endpoint is configured**. Tracked #44.
**Verification (FROZEN daemon + bundled helper + ASR, isolated env)**: `--extract-audio` (3.41s s16le),
`--extract-frames` (4 PNGs by ms / audio-only→0), exit codes 3 vs 4; via `POST /v1/sessions/import`: audio →
1 segment epoch-aligned, **silent video → 4 fs_stamp frames** (no audio); bad path → 400; **MCP `capture_import`
→ daemon**; **#42** `capture_screenshots:false` → `has_screenshots:false`, `screenshots:0`. smoke 68/68,
contracts 4/4 (regenerated goldens: +`capture_import`, +`ImportMediaRequest`, capability flags), GUI `cargo
build --release` clean. **Built Developer-ID-signed `Capture-0.2.0.dmg` (166M).** **features #42 + #43 → passes:true.**
**Open**: notarize+staple the new DMG / cut a release if desired (offered, not yet done). `drive_nolf.py` is
pre-existing + untracked — keep it out of commits.

---

## Session 49 — 2026-06-16
**Agent**: builder (macOS box, branch **v2**) — features #40 + #41 (the calm-dazzling-harbor leftovers).
**Summary**: Session **pruning + capability flags** (#40) and **re-transcribe** (#41), with full GUI↔MCP
parity (owner: every feature reaches both surfaces). Also tracked new ideas: **#42** (toggle visual
capture) + **#43** (import audio/video — needs an AVFoundation helper, no ffmpeg).
- **Capability flags** (`session.session_capabilities`, disk-computed): `has_screenshots`/`has_audio`/
  `has_mic`/`can_retranscribe`, on every summary (live via `CaptureSession.summary`, recovered via
  `registry._with_caps`) so pruning is reflected immediately. Added to `SessionSummary` + GUI `Session`.
- **Prune** (#40): `session.prune_session_dir` (delete-all / halve-cadence screenshots / remove audio),
  `registry.prune_session` (+ updates count & session.json), `POST /v1/sessions/{id}/prune`, `DaemonClient.prune`,
  MCP `capture_prune`, GUI `daemon.rs prune` + a playback "Manage" section (status icons + Halve/Delete/Remove
  buttons via the shared `ConfirmKind` modal).
- **Re-transcribe** (#41): `core/retranscribe.py` replays `audio.s16le` offline (re-chunked like audio.py;
  audio-epoch recovered from the old transcript's first record so subtitles still align; backs up to
  `transcript.prev.*`), `daemon.start_retranscribe` (background thread + SSE `retranscribe`→done/error),
  `POST .../retranscribe`, `DaemonClient.retranscribe`, MCP `capture_retranscribe`, GUI button (uses the
  active model; SSE %→`LiveState.retranscribe`; poll-loop reloads the session on done).
- **Icons**: added image/volume/volume-x/refresh/scissors SVGs.
**Verification**: prune+caps unit (halve→4, audio→can_retranscribe False, delete→has_screenshots False);
re-transcribe pipeline unit (3 chunks, offsets 0/8/16s, epoch-aligned, backup kept); HTTP routes (caps in
/v1/sessions, prune 404/400 validation); GUI `cargo build` clean (no warnings); daemon imports + smoke 68/68.
**DEPLOYED + verified (frozen daemon, notarized):** prune via HTTP on a synthetic session (halve→4 frames,
remove-audio→`can_retranscribe` false, summary refreshed, retranscribe then 400); **re-transcribe ran
end-to-end on the deployed daemon** (SSE `retranscribe` 0.70→1.0→done, `transcript.prev.jsonl` backup,
new transcript). spctl → Notarized Developer ID. **features #40 + #41 → passes:true.**
NOTE: the `capture-notary` keychain profile was missing + unreadable from the detached build shell; the
fix was inline notarytool creds — `xcrun notarytool submit <dmg> --apple-id pr0fedt@gmail.com --team-id
YH3QP44ST4 --password "$(cat .notary-password)" --wait` (run in the foreground; never echo the password).

---

## Session 48 — 2026-06-16
**Agent**: builder (macOS box, branch **main**) — feature #39 (plan: calm-dazzling-harbor.md).
**Summary**: Session-list UX — proper SVG icons, a delete-confirmation modal, and a session **playback
screen** with a video-style scrubber. GUI-only (no daemon change). Pruning + capabilities + re-transcribe
deferred to tracked features #40/#41.
- **Proper SVG icons (not emoji)**: Alex pushed back on emoji. Added a gpui **`AssetSource`**
  (`gui/src/assets.rs`, `include_bytes!` of `gui/assets/icons/*.svg`, Lucide/MIT), wired via
  `Application::with_assets(Assets)` in `main.rs`, and an `icon(name,size,color)` helper rendering
  `svg().path("icons/<name>.svg")` (gpui masks + tints by `text_color`). Session-row actions
  (folder/clipboard/stop/trash), the mic radio, and the header (settings/chevron-left) now use SVG.
- **Delete confirmation**: trash icon sets `confirm_delete`; a modal overlay (occluding `rgba` backdrop +
  centered Cancel/Delete card) confirms before `delete_session`.
- **Playback screen** (3rd top-level screen via `playback`/`sett`/`dash` gating + a `← Back` header):
  clicking a session opens it. Running → live latest shot + transcript. Finished → reads `screenshots/` +
  `transcript.jsonl` + `mic_transcript.jsonl` off-thread (`load_playback_data`; ISO stamps → epoch via
  `parse_iso_epoch`, no chrono) and renders a **scrubber** (`pb-track` drag + click-seek via
  `window.viewport_size()`) + transport (start/−5s/play·pause/+5s/end + `m:ss`), moving the
  screenshot-at-playhead + active subtitle; play auto-advances (`pb_start_ticker`, 200 ms).
- **Tracked**: features.json **#39** (this work, flip true after deploy verify), **#40** (session pruning +
  capability status icons — daemon prune endpoint + summary flags), **#41** (re-transcribe saved session
  with a chosen model). Specs synced (`gui.md`).
**Verification**: `cargo build --release` clean (no warnings); standalone GUI ran 4 s with no panic
(render incl. SVG icons + playback executed). Deploy (skip-freeze, GUI-only) + visual check: end of session.

---

## Session 47 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: **Reverted the mic echo-cancellation (Session 45)** — it broke playback. Alex: "When I
start capture now I do not hear anything (but it adds to transcript) and microphone transcript is empty."
- **Root cause**: the Session-45 AEC used the macOS voice-processing unit
  (`AVAudioEngine.inputNode.setVoiceProcessingEnabled(true)`). It DOES echo-cancel, but it engages a
  system **communication audio mode** that ducks/mutes other apps' output (→ "I don't hear anything")
  and over-cancels the mic to near-silence (→ empty mic transcript). Confirmed: reverted `--mic` RMS=224
  (real audio) vs the AEC build's near-silent mic.
- **Fix**: reverted `audiocap.swift --mic` to plain **`AVCaptureSession`** (the Session-44 path that
  doesn't touch the output): removed AVAudioEngine/VPIO + the CoreAudio device-ID lookup + the
  `--aec`/`--no-aec` flags + the AudioToolbox/CoreAudio frameworks. Mic now records cleanly; the user
  hears the app again. **Headphones are the interim answer for echo.**
- **Tracked as features.json #38** (mic echo/noise cancellation WITHOUT breaking playback — proper fix
  likely offline NLMS using the captured app audio as the reference, or a non-invasive AEC config).
- Helper-only change → fast build (skip freeze). Specs reverted (`helper-contract.md`, `audio.md`).
**Verification**: reverted helper `--mic` → real audio (rms 224), no voice-processing line; built OK.
Deploy: see end of session.

---

## Session 46 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: MCP-surface parity for the picker/mic work + confirmed echo cancellation end-to-end
through MCP. Alex: "start capture on chrome window through mcp (new session in gui) with mic, see if
handled correctly."
- **MCP `capture_start` predated the GUI's window_id/mic work** — added `window_id` and `mic_device`
  params (forwarded via `DaemonClient.start(**kwargs)` → `/v1/sessions`), plus a new **`list_audio_devices`**
  MCP tool (+ `DaemonClient.audio_mics()` → `GET /v1/audio/mics`). MCP server runs from source (not the
  frozen daemon), so live immediately. `mcp-server.md` synced (now 5 tools).
- **Verified through the actual MCP tool path** (daemon-first → deployed daemon → GUI): `list_windows`
  + `list_audio_devices` + `capture_start(pid, window_id, mic_device="default")` on a Chrome window.
  Session appeared in `/v1/sessions` (so in the GUI), recording all 4 tracks: `audio.s16le` (1 MB) +
  `transcript.jsonl`, `mic.s16le` (1 MB, AEC) + `mic_transcript.jsonl`, 17 per-window screenshots.
- **Echo cancellation CONFIRMED**: with the Batumi YouTube video on speakers, the **app** transcript had
  the Russian narration; the **mic** transcript had only Whisper's silence-hallucination ("Thank you.")
  — i.e. the speaker bleed was cancelled, not transcribed into the mic track. (If AEC had failed the mic
  transcript would mirror the app's.) Known: Whisper hallucinates on true silence — a VAD/silence gate is
  a possible follow-up.
**Status**: MCP path + AEC verified on the deployed/notarized build. Specs synced (`mcp-server.md`).

---

## Session 45 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Echo cancellation for the mic track. Alex (on a laptop): the mic caught the speaker audio.
- **Root cause**: laptop built-in mic picks up its own speaker output (acoustic echo) — the Session-44
  mic path used `AVCaptureSession`, which has no echo cancellation.
- **Fix (`audiocap.swift`, helper-only — no daemon change)**: rewrote `--mic` to use **`AVAudioEngine`**
  with the system **voice-processing unit** (`inputNode.setVoiceProcessingEnabled(true)`) = acoustic
  echo cancellation + noise suppression + AGC, **ON by default**; `--no-aec` for a raw capture. Device
  selection now maps the AVCaptureDevice uniqueID → CoreAudio `AudioDeviceID` and sets it on the
  engine's input unit (falls back to default). `AudioSink` refactored to a shared `convertAndWrite()`
  used by both the SCStream callback and the engine tap. Linked CoreAudio + AudioToolbox.
- Because only the **helper** changed (the daemon's `--mic <device>` command is unchanged), the build
  **skips the freeze** (re-bundle the new helper + re-sign + notarize).
**Verification**: `--mic` → "voice processing (echo cancellation) enabled", READY `aec=true`, 59 KB PCM;
`--mic --no-aec` → `aec=false`, raw; `--list-mics` clean. Echo-removal *quality* is Alex's ear-test with
speakers playing. Specs synced (`helper-contract.md`, `audio.md`). Deploy: see end of session.

---

## Session 44 — 2026-06-16
**Agent**: builder (macOS box, branch **main**) — feature #37, planned in `~/.claude/plans/calm-dazzling-harbor.md`.
**Summary**: Grouped multi-app window picker (checkboxes) + microphone selector (separate mic track,
helper-native, no ffmpeg). Resolved with Alex: multiple apps captured simultaneously; **one audio per
app**; mic attaches to **one** app as a **separate track** (not mixed); **no ffmpeg**.
- **Helper (`audiocap.swift`)**: new `--list-mics` (AVCaptureDevice discovery → JSON lines
  `{id,name,default}`) and `--mic [<id>]` (AVFoundation **AVCaptureSession** mic-only capture → same
  s16le/READY contract as the app path; needs only Microphone permission). Shared CMSampleBuffer→s16le
  `process()` between the SCStream and AVCapture delegates. Verified: list prints devices; `--mic`
  emitted 64 KB PCM in ~2 s (48k→16k resample).
- **Daemon**: `MacAudioSource.command(mic_device=…)` now builds `--mic` for `source="mic"` (**dropped
  the ffmpeg branch**); `list_input_devices()` shells `--list-mics`; `GET /v1/audio/mics`. `AudioCapture`
  gained `track`/`mic_device` (own filenames: `mic.s16le`/`mic_transcript.*`). `CaptureSession` starts a
  **second** AudioCapture for the mic; `mic_device` on `StartSessionRequest` + `mic_status`/`mic_segments`
  in the summary. Verified end-to-end: a session with `mic_device` wrote `mic.s16le(121844)` +
  `mic_transcript.jsonl` alongside the app track.
- **GUI**: picker grouped by app with checkbox rows (`checked: HashSet<window_id>`, multi-app) + a per-app
  🎤 radio (`mic_app`); a `Mic:` device-selector row (`mics` from `/v1/audio/mics`, `mic_device` persisted
  in gui-settings.json); `start_capture` spawns one session per checked window, dedupes app audio per pid
  (`capture_audio` only on the first window of each pid), and sets `mic_device` on the first window of the
  mic app. `daemon.rs`: `AudioDevice`/`AudioDevices` + `audio_mics()`. `cargo build --release` clean.
- **Docs**: `audio.md`, `daemon.md`, `gui.md`, `platform-abstraction.md`, `helper-contract.md`, the
  `audiocap.swift` header. **features.json #37** added (`passes:false` → flip after deployed verify). Smoke 68/68.
**Verification (DEPLOYED, frozen daemon)**: re-froze + signed + bundled the new helper (Developer ID,
`--mic`/`--list-mics` work) + redeployed. `GET /v1/audio/mics` returns devices via the frozen daemon;
a real capture of a Chrome window recorded **`audio.s16le`=190 KB (app) + `mic.s16le`=192 KB (separate
mic track) + 6 per-window screenshots together** — all three tracks at once. Smoke 68/68; GUI clean.
**features.json #37 → passes:true.** DMG notarization finished after the local cp-deploy, so the
installed `.app` was stapled separately (local cp has no quarantine, launches fine regardless). The
GUI picker checkboxes / 🎤 radio / mic chips are a visual check, but the full daemon+helper path is proven.

---

## Session 43 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Diagnosed an audio-duplication report. Alex: started sessions 60b42b + 3db935
simultaneously on different videos in different windows, but "they have apparently captured one audio."
- **Root cause = a hard macOS limit, not a bug.** Both sessions targeted the **same pid 1235** (two
  windows of one browser). Evidence: 60b42b's window was the Batumi video, 3db935's was the NixOS
  video, but BOTH transcripts contain the Batumi audio ("Чисто Дубай на берегу Черного моря"). The
  `audiocap` helper builds `SCContentFilter(display:, including:[app], exceptingWindows:[])` —
  ScreenCaptureKit (and Core Audio process taps) capture audio **per-application**, never per-window.
  Two windows of one process ⇒ one shared audio stream. There is no per-window audio API.
- **Improvement (honest surfacing, not a "fix"):** `server._start_session` now detects when a new
  app-audio session's `pid` matches a live session already capturing that pid's audio, and appends a
  session **note** ("audio: app pid N is already captured by session …; macOS captures audio per-app,
  not per-window … Capture from separate processes for distinct audio."). So the duplication is
  visible instead of looking like a bug. NOTE: screenshots ARE per-window now (Session 42 window_id);
  only audio can't be split.
- **Workaround for Alex**: distinct audio needs distinct processes — e.g. two different browser apps
  (Chrome + Safari), not two windows of one.
- **Specs synced**: `audio.md` (per-app limitation + the overlap note).
**Verification**: predicate unit-tested with real `SessionRegistry`+`CaptureSession` — same-pid+app
→ warns; different pid / new-session-mic / new-session-no-audio → no warn. py_compile OK.
**NOT yet deployed** — daemon change needs a re-freeze; holding for Alex's go-ahead (the core finding
is "can't be done per-window", so the deploy only adds the warning note).

---

## Session 42 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Fixed a window-targeting bug. Alex: "I have 2 chrome windows… selected the second one
for capture, but screenshots were for the first one, although the sound transcribed."
- **Root cause**: `StartSessionRequest` only carried `pid`/`app_name` — never a `window_id`. The
  screenshotter's `_resolve_window_id()` called `_finder.primary(pid=…)`, which returns the app's
  *primary* window. Chrome's windows share one process pid, so "primary" ≠ the picked window → shots
  of the wrong window. **Audio was right** because it's keyed per-process (pid), not per-window.
- **Fix (carry `window_id` end-to-end)**: `StartSessionRequest.window_id` (optional refinement, not a
  target — audio still per-pid) → `CaptureSession(window_id=…)` (also re-labels `window_title` from
  the picked window) → `Screenshotter(window_id=…)`. `_resolve_window_id()` now returns the explicit
  id verbatim every tick (a CGWindowID is stable for the window's lifetime; `primary()` never
  consulted). GUI `start_capture` sends `"window_id": w.window_id` from the selected picker row.
- **Also closed GitHub #2 (the "proper resolution" Alex pointed to)** — same bug class, deeper root:
  a Wine game (shell→wine→`lithtech.exe`) screenshotted the whole desktop. The window belongs to a
  **child process** (different pid than the launched shell), so `primary(pid=launcher)` found nothing
  → whole-screen fallback. Fix: **descendant-pid discovery** — `util.descendant_pids(pid)` walks the
  process tree (POSIX `ps -axo pid=,ppid=`); `screenshots._descendant_primary` returns the largest
  window owned by any descendant. `_resolve_window_id` tries it when the pid owns no window. POSIX
  only (Windows returns empty — its windows are owned by their own pid; Wine chains are the mac case).
- **Specs synced**: `screenshots.md` (resolve order + descendant fallback), `daemon.md` (start-body
  `window_id`), `gui.md` (picker sends window_id).
**Verification**: (1) unit — `_resolve_window_id` returns the explicit id and asserts `primary()` is
NOT called when window_id is set; falls back to `primary()` otherwise. (2) OS-level — captured two
real windows by id (Zed 1280×720 landscape, Slack 754×945 portrait); each shot matched its target's
aspect (2560×1440 / 1508×1890) → exact per-window capture. (3) #2 — `descendant_pids` found a real
python→sleep child; a launcher owning no window resolved to its child's window (not whole-screen).
(4) smoke 68/68; GUI build clean; daemon py_compile OK. NOTE: with the new GUI sending `window_id`, an
OLD frozen daemon would 400 (extra field) — GUI+daemon ship together so they stay in sync; just don't
run a new GUI against a stale daemon. (build4 failed at codesign — Apple TSA "timestamp not found",
transient; rebuilt.)

---

## Session 41 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Two follow-up bugs from Alex on the Session-40 model manager + settings.
- **Bug A — capture-quality settings reverted on GUI relaunch.** They lived only in
  `CaptureApp`'s in-memory fields (re-init'd to png/native/80 every launch). Fix: persist to
  `~/.capture/gui-settings.json` — `save_settings()` writes `{shot_format, shot_res_ix,
  jpeg_quality}` on every quality `chip` click; `load_settings()` seeds the fields in `new()`.
  GUI-local (a UI default in the window process), not daemon-side. No re-freeze needed for this one.
- **Bug B — "Whisper Large v3 Turbo" (1.6 GB) progress bar flashes, never "downloads"; the 4-bit
  quant (464 MB) works.** Root cause: `is_downloaded()` only checked `config.json` + `weights.npz`,
  but `mlx-community/whisper-large-v3-turbo` ships **`weights.safetensors`** (the q4 ships
  `weights.npz`). So even after a *successful* 1.6 GB fetch the row read "not downloaded" and kept
  offering Download; the bar "flashed" because `snapshot_download` returns instantly once it's
  already cached (verified: 1614 MB on disk, returns <1s). Fix: `is_downloaded()` accepts
  config.json + **either** `weights.npz` *or* `weights.safetensors` (`_WEIGHT_FILES`). This is
  daemon-side → needs a re-freeze (deployed app runs the frozen daemon).
- **Specs synced**: `gui.md` (settings persistence), `daemon.md` (downloaded = npz-or-safetensors).
**Verification**: `is_downloaded('whisper-large-v3-turbo')` → **True** after fix (was False with the
1.6 GB already cached); `base-mlx` still True. GUI `cargo build --release` clean. Re-froze + signed +
notarized + redeployed (see end of session).

---

## Session 40 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Deployed the Session-39 Settings build, then fixed ASR model-download **progress** +
added model **removal**. Alex: "When I download new voice recognition model — no progress is being
displayed. I think we should also give users the possibility of removing them."
- **Deployed the Settings/quality build** (meeting over): reused freeze, signed, installed to
  `/Applications/Capture.app`. **Screen Recording grant persisted** across the reinstall (same Team ID
  YH3QP44ST4 — no re-grant); `/v1/permissions` → `screen_recording: granted`.
- **Root cause of "no progress":** `hf_xet` is installed, so downloads used the **xet backend**,
  which streams content-addressed chunks into a *separate* cache and only materializes the final
  blob at the end. The daemon's progress = on-disk cache-size poll vs Hub total → it read ~0 % then
  jumped to 100 %. Verified live (xet off: `0.0→0.07→0.22→0.44→0.66→0.88→1.0`).
- **Fix (`core/asr/manager.download`)**: force the plain HTTP backend with
  `constants.HF_HUB_DISABLE_XET = True` (read live by hf_hub at download time, so import order is
  irrelevant). The plain backend grows a `<blob>.incomplete` file the existing poll already tracks.
- **NEW removal**: `manager.delete(repo)` `rmtree`s the repo's HF-cache dir (catalog-validated;
  returns `freed_bytes`). Route `POST /v1/asr/models/delete` (409 while downloading, 400 if unknown).
  Deleting the *active* model just reverts it to "active · needs download".
- **GUI (`app.rs`)**: each model row is now header + a thin **determinate progress bar**
  (`relative(fraction)`-width fill) while busy; a **Remove** button (amber-red) on any downloaded
  model → `delete_model` → `daemon.asr_delete`. New client method `Daemon::asr_delete`.
- **Specs synced**: `daemon.md` (delete route + xet-disable rationale), `gui.md` (progress bar +
  Remove). No `features.json` flip — this is a slice of the in-progress **#33 (M3 GPUI app)**, which
  stays `passes:false` until the whole milestone lands.
**Verification**: GUI `cargo build --release` clean. Daemon round-trip tested end-to-end on the source
daemon: `delete(whisper-base-mlx)` freed 143 MB → `is_downloaded:false`; re-`download` emitted 5
intermediate fractions (**progress visible**) → `is_downloaded:true`. Re-froze daemon + signed +
notarized DMG, reinstalled, and re-verified in the running app (see end of session).

---

## Session 39 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Settings screen (declutter the main view) + capture-quality controls. Alex: "settings
should not bloat main screen … manage voice recognition model, permissions, and capture quality
(png/jpeg + resolution)". **Did NOT deploy** — Alex has a live meeting transcribing on the running
daemon ("do not terminate current process"); code is built but not reinstalled/relaunched.
- **`show_settings` toggle** (`⚙ Settings` / `← Back`, top-right): the window is now two screens —
  the **capture dashboard** (Refresh/Start, Launch input, windows/sessions, live detail) and a
  **Settings** screen (Capture quality + Whisper model manager + Permissions + skill installer).
  Each panel renders via `settings.then(|| …)` / `(!settings).then(|| …)` (only one screen's panels
  exist at a time) — chose in-place conditional wrapping over extracting the big inline blocks.
- **NEW Capture quality** (`chip` helper): screenshot format PNG/JPEG (`shot_format`), resolution
  (`RES_PRESETS`: Native/1440p/1080p/720p, `shot_res_ix`), JPEG quality 60/80/95 (jpeg only). Merged
  into the `/v1/sessions` body via `shot_settings()` for `start_capture`/`launch_command` — uses the
  **existing** daemon `StartSessionRequest` fields (`screenshot_format`/`_resolution`/`_jpeg_quality`),
  so NO daemon change/freeze needed; the running daemon already supports them.
**Verification**: GUI builds clean (`Finished`, no warnings). Untested visually (no relaunch during
the meeting). **TODO when the meeting ends**: rebuild signed+notarized DMG (the screen grant persists
— same Team ID), reinstall, verify the Settings toggle + quality + mic Grant.

---

## Session 38 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: **#31 TCC fix VERIFIED** — Developer-ID signing makes the daemon inherit the app's
Screen Recording grant. The whole permissions saga is solved.
- Alex installed a **Developer ID Application** cert (Team **YH3QP44ST4**). Built signed via
  `CAPTURE_SIGN_IDENTITY` (hardened runtime + `packaging/entitlements.plist` + secure timestamp).
- **The signed+hardened frozen daemon BOOTS** (mlx/numba survive hardened runtime with the JIT /
  library-validation entitlements). All 4 binaries (`CaptureBar`, `capture-gui`, `captured`,
  `audiocap`) share **Team YH3QP44ST4** → ONE TCC identity.
- **Proof**: after Alex granted Screen Recording once, the daemon reports `screen_recording=granted`,
  and a FRESH daemon (after restart AND a full app relaunch) keeps it → **15 windows, 13 titles**,
  real screenshots. The grant **persists** (Developer-ID identity is stable, not cdhash) — no
  re-granting, no crash. Answered Alex's "restart daemon not app": restarting just the daemon is
  enough (same identity); the macOS "restart the app" nudge is ignorable.
- **Notarization**: `xcrun notarytool store-credentials capture-notary` (Apple ID **pr0fedt@gmail.com**
  — alex.d.nax@ gave a 401; corrected). Submitted the signed DMG (in progress). Secrets gitignored
  (`.asp.capture`, `.notary-password`); `.asp.capture` deleted (keychain holds the creds now).
- Also shipped: agent **Open Window focuses** the existing window vs. relaunching (`guiProcess` +
  `NSRunningApplication.activate`).
**Next**: confirm notarization + staple; then the rest of M1 (brew tap, prebuilt helper, capture
doctor). Re-freeze owed at some point so the daemon's permissions.py fixes (mic) ship signed.

---

## Session 37 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Hit (and started fixing) the ad-hoc **TCC wall**, + a menu-bar focus fix.
- **The wall (definitively diagnosed)**: an ad-hoc bundle's daemon binary (`captured`) is a SEPARATE
  TCC identity from the granted app ("Capture"). Confirmed: Alex granted "Capture" in Settings, a
  FRESH daemon still reports `screen_recording: denied`, 0 window titles, and a test capture wrote
  **no screenshot**. The GUI's `CGRequest` grants the app's identity, which the differently-signed
  daemon can't share. This is unfixable in ad-hoc — it needs Developer-ID signing (shared Team ID).
- **Alex has a paid Apple Developer account → doing #31 properly.** Prepared the infra:
  `packaging/entitlements.plist` (the frozen daemon needs `allow-jit` / `allow-unsigned-executable-
  memory` / `disable-library-validation` / `allow-dyld-environment-variables` for mlx+numba JIT and
  the many .so, + `device.audio-input`); `build_macos_dmg.sh` grew a **`CAPTURE_SIGN_IDENTITY`** path
  (hardened runtime + entitlements + `--timestamp`, inside-out, re-signs the helper with the shared
  Team ID) and **`CAPTURE_NOTARIZE_PROFILE`** (notarytool submit + staple), + `NSMicrophoneUsageDescription`.
  **BLOCKED**: keychain has 0 signing identities — Alex needs to create a "Developer ID Application"
  cert (Xcode ▸ Settings ▸ Accounts ▸ Manage Certificates ▸ + ▸ Developer ID Application) and
  `xcrun notarytool store-credentials`. Then: build with the identity → verify the daemon inherits
  the grant → notarize.
- **Open Window focus**: the agent now tracks `guiProcess` and **focuses** the existing window
  (`NSRunningApplication.activate`) instead of launching a duplicate; only launches when none is open.
**Verification**: agent compiles; `bash -n` the build script OK; `plutil -lint` the entitlements OK.
**Superseded**: the earlier ad-hoc workaround ideas (setdisclaim, daemon self-registration, GUI-as-
wrapper) — Developer-ID signing makes them unnecessary (the daemon shares the app's grant cleanly).

---

## Session 36 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Two more install-test fixes from Alex.
- **Microphone Grant ALSO crashed the daemon** (the "permission request failed: Unexpected EOF"
  in the screenshot was the *mic* Grant, not screen — screen already uses the GUI FFI). Turns out
  `AVCaptureDevice.requestAccessForMediaType` aborts a headless/background-only process too when it
  must show the dialog (my earlier "mic is safe" test ran in an already-granted context, so it
  no-op'd). Fix: mic now mirrors screen — `request_microphone()` returns status without prompting;
  the GUI mic row has **no Grant button** (`can_prompt:false`), only **Settings** (there's no
  block-free Rust mic prompt, and macOS auto-prompts mic the first time the ffmpeg fallback opens
  the device). Removed the now-unused Rust `request_permission` client method.
- **No visible menu-bar tray on launch** — the agent WAS running (daemon + window came up), but the
  text label `○ capture` was too easy to miss. Replaced with an **SF Symbol icon** (`applyIcon`:
  `record.circle` idle / `record.circle.fill` + count capturing; text fallback) — far more findable
  in a crowded/notched menu bar.
**Verification**: daemon mic+screen requests survive (return status, no crash); smoke 68/68; GUI +
agent compile clean. Fast rebuild (reused freeze — the daemon's dormant mic-request code is never
hit now since the GUI doesn't POST it) and **reinstalled to /Applications**; relaunched → agent +
daemon + GUI up, healthy, permissions report denied/undetermined as expected.

---

## Session 35 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: FIX — clicking **Grant (Screen Recording) crashed the daemon** ("permission request
failed: Unexpected EOF" → "no daemon"). Plus robustness for the "GUI fails to reconnect on restart".
- **Root cause**: `CGRequestScreenCaptureAccess` needs a window-server connection; calling it from
  the **headless daemon** SIGABRTs the process. The daemon was calling it on `POST
  /v1/permissions/request {screen_recording}`.
- **Fix**: the daemon **never prompts** for screen recording — `request_screen_recording()` returns
  status only (`CGPreflight` is safe). The **GUI** shows the prompt itself via **CoreGraphics FFI**
  (`screen_perm::request()` → `CGRequestScreenCaptureAccess`); the GUI is a real GPUI app with a
  window-server connection, so it won't crash. **Microphone stays in the daemon** —
  `requestAccessForMediaType` is dispatch-queue-based and headless-safe (verified it survives).
- **Reconnect/respawn**: the GUI poll already re-discovers each tick (reconnect was fine); the report
  was the daemon being **down** from the crash. Verified the agent auto-respawns a `kill -9`'d daemon
  in ~2 s. Made it more robust: respawn is now gated on **`userStoppedDaemon`** (set by "Stop Daemon")
  instead of `weStartedDaemon`, so it recovers regardless of how the daemon first started.
**Verification**: daemon `request('screen_recording')` survives (returns status, no crash); smoke
68/68; GUI + agent compile clean; **re-froze** + rebuilt (165M). The in-GUI prompt itself (denied →
dialog → no crash) is a manual check — my shell is already granted so `CGRequest` is a no-op here.
**Caveat**: TCC attribution of a GUI-triggered grant to the daemon (different binary, same bundle) is
the ad-hoc #31 limitation; "Open Settings" is the reliable fallback.

---

## Session 34 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Finished the permissions UI ("we need both"): **Microphone** permission + **auto-restart
after a grant**. #33 slice 12.
- **Microphone**: added `pyobjc-framework-AVFoundation` (pyproject + freeze `--collect-all
  AVFoundation/CoreMedia` — verified it loads **frozen**: the bundled daemon returns a real mic
  status, not `unknown`). `core.permissions` gained `microphone_status`/`request_microphone`
  (`AVCaptureDevice` authorizationStatus/requestAccess) + a `request(kind)` dispatcher. `GET
  /v1/permissions` now returns `screen_recording` + `microphone`; the request route handles
  `kind=microphone`. GUI: a reusable `perm_row` (status + Grant + Settings) for both, Settings
  deep-linking `Privacy_ScreenCapture`/`Privacy_Microphone`.
- **Auto-restart**: a new Screen Recording grant needs the daemon to restart. GUI **Restart daemon**
  POSTs `/v1/admin/shutdown`; the menu-bar **agent auto-respawns** it — `CaptureBar` poll: if the
  daemon is down AND `weStartedDaemon`, `ensureDaemon()` (debounced 6 s via `lastSpawn` so a slow
  startup doesn't double-spawn). An intentional **Stop Daemon** clears `weStartedDaemon`, so it's
  not respawned. Also gives crash recovery. No app quit/reopen needed.
**Verification**: routes live (sr+mic, bad kind→400); bundled (frozen) daemon serves both; smoke
68/68; GUI + agent compile clean. **Re-froze** + rebuilt the .app (166M) with AVFoundation. Mic grant
applies immediately; SR needs the daemon restart (now one click).
**Caveat**: TCC attribution/persistence for the ad-hoc daemon is still #31.

---

## Session 33 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Two fixes from Alex testing + (next) a permissions UI request.
- **Sessions list was incomplete** — 16 folders on disk in `~/.capture/runs` but only 8 showed.
  The registry rebuilt history from the **index** (`sessions.jsonl`) only; 7 older folders predated
  the current index (it had been reset) → orphaned/invisible. Added `_scan_runs_dir()` to
  `_load_history`: after the index load, scan `$CAPTURE_RUNS_DIR` (else `~/.capture/runs`) for
  `capture-*/session.json` and recover any sid not already covered (index wins; idempotent; re-bounds
  to max_sessions). Verified: a fresh registry now returns **15** (was 8). NOTE: the **bundled**
  daemon is frozen from old code — needs a **re-freeze** to take effect (batched with permissions).
- **Window rows showed "Chrome — "** (dangling em-dash) — all 14 window titles came back EMPTY
  because macOS redacts `kCGWindowName` for a process **without Screen Recording permission** (the
  bundled daemon lacks the grant). Cosmetic fix: GUI shows just the app name when the title is empty.
  The real cause (daemon needs Screen Recording) → the permissions UI Alex just asked for + TCC
  onboarding (#31).
- **Permissions UI (Alex: "setting up/revoking permissions should be in the gui")**: new
  `core/permissions.py` (Quartz `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`)
  + daemon `GET /v1/permissions` + `POST /v1/permissions/request`. GUI Permissions row: Screen
  Recording status (polled), **Grant** (the *daemon* — the grantee — triggers the system prompt;
  needs an app restart to apply), **Open Settings** (deep-links the `Privacy_ScreenCapture` pane for
  grant OR revoke — apps can't toggle a TCC right). The daemon is the right grantee since it does the
  screen capture. Mic (AVFoundation) deferred — not in the venv. Apps can't grant/revoke TCC directly,
  so the GUI = status + prompt + Settings deep-link.
**Verification**: smoke 68/68; GUI builds clean; runs-dir scan live (15 sessions); permissions route
live (granted/denied, bad kind→400). **Re-froze** the daemon + rebuilt the .app (166M) so the Python
changes (registry scan + permissions) ship in the bundle.
**Caveat**: TCC attribution/persistence for the ad-hoc unsigned daemon is the #31 problem; a granted
Screen Recording right needs the app relaunched.

---

## Session 32 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: GUI usability batch from Alex testing the running app — six asks. #33 slice 10.
- **Scrollable lists**: the Windows picker and Sessions lists now scroll (`max_h` 200 +
  `overflow_y_scroll`, `#win-scroll`/`#sess-scroll`) and show ALL rows (dropped the top-6 slices).
- **Launch a process/URL**: a minimal single-line text input (focusable `div` +
  `track_focus`/`on_key_down` — `key_char`/backspace/⌘V-paste/Enter; no IME/selection, ~40 lines)
  + "Launch & Capture" → `POST /v1/sessions` with `command` (launch mode). A URL is just a command
  (`open https://…`). Confirmed the gpui-0.2.2 APIs (KeyDownEvent.key_char, Modifiers.platform=⌘,
  cx.read/write_to_clipboard, window.focus) before building.
- **Per-session actions**: **Folder** (`open` the dir in Finder), **Prompt** (copy a ready-to-paste
  summarization prompt pointing a coding agent at the dir's transcript/screenshots/logs —
  `cx.write_to_clipboard`; Alex: "deepen this flow later"), **Del** (finished only).
- **Delete backend (new)**: `POST /v1/sessions/{id}/delete` → `registry.delete()` (drop history/live
  record + **rewrite** the append-only `sessions.jsonl` index, atomically) + `rmtree` the dir
  **guarded** by a `session.json` presence check (never nukes an arbitrary path); **400 if live**
  (stop first). Python + Rust clients gained `delete()`; `Session` wire gained `dir`.
**Verification**: delete route live-tested end-to-end (running→400 "stop first"; stopped→200
`{deleted:true}` + dir gone + removed from the index + subsequent GET 404). smoke 68/68, contracts
4/4 (no schema change — delete response is ad-hoc, like admin/shutdown). GUI builds clean (no
warnings). The input + buttons are a manual visual check (no GPUI test harness).
**Caveat**: open-folder uses macOS `open` (Windows/Linux branch owed); no delete-confirm yet.
**Scroll fix (same session, Alex)**: the per-list `max_h`+`overflow_y_scroll` panes "had no scrollbar
and scrolled together with the main view" — bare gpui 0.2.2 has no scrollbar widget and nested
`overflow_scroll` regions fight the root for the wheel. Reverted to a **single** page scroll
(`#root` `track_scroll`+`overflow_y_scroll`) with a **custom draggable scrollbar** (`scrollbar()` +
`on_scrollbar_drag()`): an absolute right-edge thumb sized from the `ScrollHandle`'s prior-frame
`bounds()`/`max_offset()`/`offset()` (auto-hidden when content fits; mouse-drag → `set_offset`).
Builds clean; the thumb geometry/drag feel is a manual visual check (no GPUI harness).

---

## Session 31 — 2026-06-16
**Agent**: builder (macOS box, branch **main**)
**Summary**: Added a **native macOS menu-bar agent** so the tray + daemon persist independent of the
GPUI window (Alex: "whenever the daemon is running a tray icon should persist"; then chose a
**native per-OS agent** over an in-GPUI tray). #33 slice 9; Windows sibling = new #36.
- **Why native, not in-GPUI**: gpui 0.2.2 forces `ActivationPolicy::Regular` (no `LSUIElement`
  menu-bar mode) and a resident GPUI process is too heavy for an always-on tray. (The in-GPUI
  approach IS feasible — verified the GPUI APIs — but the owner chose native per-OS agents.)
- **`agent/macos/CaptureBar.swift` (new)**: AppKit `NSStatusItem` + `LSUIElement` app, ~110 KB
  (`swiftc -O`). Polls `/v1` every 2 s → title `○ / ● / ⦿ N`. Menu: Open Window, Stop All
  Captures, Start/Stop Daemon, Quit. Spawns the bundled `captured` detached; **Quit gracefully
  `/v1/admin/shutdown`s the daemon when idle** → fixes the "running daemon pins the .app, can't
  uninstall" problem. Launches `capture-gui` with `CAPTURE_AGENT=1`.
- **GPUI app under the agent (`CAPTURE_AGENT=1`)**: builds no tray, doesn't spawn the daemon, and
  **exits on window-close** (`main.rs` `on_window_closed → cx.quit()`; GPUI doesn't auto-quit on
  last-window-close — confirmed in the gpui-0.2.2 source). Standalone/dev keeps its own tray + spawn.
- **Bundle restructure** (`build_macos_dmg.sh`): `CFBundleExecutable=CaptureBar` + `LSUIElement`;
  `MacOS/{CaptureBar,capture-gui}`; compiles the agent with `swiftc`; signs both.
- **Specs**: new `docs/specs/agent.md`; product-architecture decision record + roadmap #36;
  gui.md agent-mode note; specs/README row; README "what the app does".
**Verification**: agent compiles (arm64). Launched the rebuilt `.app` via LaunchServices (`open`):
CaptureBar stays **resident**, spawns the daemon (`/v1/health ok`), and opens the GPUI window
(`capture-gui` running). The menu-bar UI clicks are a manual check (no `NSStatusItem` test harness).
DMG rebuilt 164M (reused freeze). **Diagnosed Alex's "no tray" report**: no `Capture.app` was
installed — the prior test was the GPUI-only build whose tray dies with the window; the agent build
fixes exactly that.
**Remaining**: move ⌃⌘R into the agent (window-less hotkey); a real menu-bar icon image; login-item
(SMAppService); Windows agent (#36); Developer-ID signing (#31).

---

## Session 30 — 2026-06-15
**Agent**: builder (macOS box, branch **main**)
**Summary**: In-GUI **Whisper model manager** + **on-device ASR in the self-contained app**
(Alex: "we should be able to download whisper model through our gui"). The installed app now
transcribes locally; the GUI downloads model *weights* on demand. #33 slice 8.
- **Decision (asked Alex)**: bundle the mlx runtime in the app (true on-device ASR; weights
  downloaded via the GUI, not bundled) — chosen over keeping the bundle lean.
- **De-risked first**: PyInstaller CAN freeze mlx — a frozen `--asr-selftest` confirmed the Metal
  kernel compiles from the bundled 125 MB `mlx.metallib` AND whisper-tiny transcribes, *frozen*.
- **`core/asr/manager.py` (new)**: curated catalog of **verified** mlx-community repos (naming is
  inconsistent — `whisper-tiny` but `whisper-base-mlx`; `whisper-base`/`-small`/`-large-v3` 404).
  `runtime_available`, `is_downloaded` (HF-cache), `active_model`/`set_active_model`, `download`
  (backend-agnostic progress by polling cache-dir size vs Hub total — hf_hub's xet/hf_transfer
  bypass the tqdm hook). **`core/config.py` (new)**: persisted `~/.capture/config.json`.
  `whisper_local` now resolves model arg→env→config→default.
- **Daemon `/v1/asr/*`**: `GET models`, `POST models/download` (background, dup-guard, SSE
  `asr_download`/`_done`/`_error` — **no session_id**), `POST model` (persist active). Pydantic
  contract models added + golden regenned. Python client + Rust client methods.
- **GUI**: a Whisper-models panel (Download/Use buttons, live `↓NN%` from SSE `asr_progress`;
  SSE thread handles the session-less asr events *before* the session filter). Polls the catalog.
- **Packaging**: `build_macos_dmg.sh` now BUNDLES mlx (`--collect-all mlx mlx_whisper
  huggingface_hub tiktoken numba`); **`captured_main.py` adds `multiprocessing.freeze_support()`**
  — numba uses multiprocessing and a frozen child was re-running the entry → **rogue 2nd daemon**
  (found + fixed). Best-effort `--asr-selftest` runs during the build.
**Verification**: daemon routes live-tested (catalog flags; set-active persists to config.json;
`whisper-base-mlx` download streamed progress 0→1 then `asr_download_done`, then `downloaded:true`;
bad/dup → 400/`started:false`). Full DMG rebuilt (**166 MB**, mlx bundled); in-build self-test
printed `mlx Metal OK` + `mlx_whisper OK` with **no hang** (freeze_support). Bundled daemon (out of
the .app) reports `backend_available:true` and runs as a **single** process. App signs `--strict`;
helper keeps `com.local.audiocap`. Contracts 4/4, smoke 68/68, GUI builds clean.
**Post-test fixes (Alex, from the running app)**: (1) the **window now scrolls** (`#root` +
`overflow_y_scroll`; the detail pane is `flex_shrink_0`, not `flex_1`, which would grab the scroll
container's unbounded main axis) — content was clipped below the fold. (2) An **active-but-not-
downloaded** model (the default `large-v3-turbo`) now shows `● active · needs download` in amber
beside its Download button, instead of a bare `● active` that looked ready when it wasn't.
**Known caveat**: mlx_whisper does an online HF revision-check on cached models (can be slow
offline) — offline-on-cached polish deferred (noted in features #33 remaining).
**#33 status**: window + client + picker + start/stop + live SSE + tray + hotkey + skill +
self-contained bundle + **on-device ASR + model manager** — done. **Remaining**: onboarding,
RenderImage eviction, offline-transcribe polish, Developer-ID signing/notarization (#31), gpui→zed.

---

## Session 29 — 2026-06-15
**Agent**: builder (macOS box, branch **main**)
**Summary**: Made the macOS app **self-contained** — the `.app` now bundles a frozen daemon and the
GUI auto-spawns it, so there's no venv to set up and nothing to start by hand (Alex: "We should make
the daemon installable with this app"). #33 slice 7.
- **`packaging/build_macos_dmg.sh`**: now **PyInstaller-freezes the daemon** (onedir) into
  `Capture.app/Contents/Resources/captured/`, copies the **signed `audiocap` helper** beside the
  frozen binary, and signs **inside-out** (nested dylibs/.so + frozen binary + GUI ad-hoc, then the
  bundle last with NO `--deep`) so the helper KEEPS its stable `com.local.audiocap` identity (audio
  TCC-grant persistence). Freeze excludes mlx/torch/faster_whisper/riva (lazy + huge). DMG → 28 MB.
  `CAPTURE_SKIP_FREEZE=1` reuses an existing freeze for fast GUI-only iteration.
- **`packaging/captured_main.py` (new)**: PyInstaller entry (`from capture_mcp.daemon.server import
  main`).
- **`gui/src/daemon.rs`**: `available()` (health probe), `bundled_daemon()` (resolves
  `<exe>/../Resources/captured/captured`), `spawn_detached()` (own process group → outlives the GUI).
- **`gui/src/app.rs`**: `new()` auto-spawns the bundled daemon if none is running; the **poll loop**
  and **SSE thread** now **re-discover** each tick/reconnect (so they attach to the daemon spawned
  after launch) and the poll loop sets `v.daemon`.
- **`src/capture_mcp/core/platform/macos.py`**: `helper_path()` resolves `audiocap` beside
  `sys.executable` (the frozen binary) so per-app audio works from the bundle (override → beside-exe
  → repo `_HELPER`).
**Verification**: GUI builds clean (release). Full DMG build runs end-to-end. The frozen `captured`
copied OUT of the `.app` **boots**: writes `daemon.json`, `/v1/health` → `ok:true` (platform darwin),
`/v1/windows` → 4 windows (Quartz works frozen). `codesign --verify --strict` of the `.app` passes;
the bundled helper still shows `Identifier=com.local.audiocap`, `Authority=capture-mcp-codesign`.
Python smoke 68/68 (arm64 venv). The in-app GUI→daemon auto-spawn is a manual launch check (no
headless GPUI harness).
**Caveat surfaced**: the frozen daemon does **capture + raw audio but not local ASR** (mlx excluded);
transcription needs a remote backend (`openai_compat`) or the repo daemon → motivates an in-GUI ASR
model manager (next).
**#33 status**: window + daemon client + picker + start/stop + live SSE transcript/preview + tray +
hotkey + skill install/update + **self-contained .app/.dmg (bundled daemon)** — done. **Remaining**:
ASR model manager / Settings (in-GUI whisper download), onboarding, RenderImage eviction, Developer-ID
signing + notarization (#31), gpui→zed-git for Linux/a11y.

---

## Session 28 — 2026-06-15
**Agent**: builder (macOS box 15.7.3, branch **v2**)
**Summary**: Two distribution features for the GUI: a **macOS .app/.dmg packaging script** and an
in-GUI **"install/update the capture skill into a coding agent's home"** option (with a status
check), both at Alex's request.
- **`packaging/build_macos_dmg.sh` (new)**: builds the GUI release binary → `Capture.app`
  (Info.plist, `com.capturemcp.gui`) → **ad-hoc signs** it (NOT Developer-ID/notarized — that's
  #31) → wraps it in `dist/Capture-0.1.0.dmg` (4.8 MB, with an `/Applications` symlink). Bundles
  the `capture` skill into `Contents/Resources/skill`.
- **`gui/src/skill.rs` (new)**: locate the skill source (bundled in the .app, else `<repo>/skills/
  capture`), copy it into `~/.claude/skills/capture` / `~/.codex/skills/capture` (clean replace =
  install OR update; excludes `__pycache__`/`.pyc`); **`status()`** content-hash-compares bundled
  vs installed → NotInstalled / UpToDate / UpdateAvailable.
- **`gui/src/app.rs`**: a "Skill →" row with a status-aware button per agent (`— install` / `✓` /
  `↑ update`); clicking installs/updates and refreshes status. **`main.rs`** gained headless flags
  `--skill-status` and `--install-skill <agent>`.
- **README**: new **"Installing the macOS app (unsigned test build)"** section — build the DMG,
  drag-install, and an explicit **Gatekeeper bypass** (right-click → Open / Sequoia "Open Anyway" /
  `xattr -dr com.apple.quarantine`) with an honest "you're choosing to run an app Apple hasn't
  checked" note; plus the skill-install/update docs. "Run it manually" GUI subsection links to it.
- Specs: gui.md (skill + packaging files/behavior/limitations); features.json #33 slices 5–6.
**Verification**: GUI builds clean (release); DMG built (4.8 MB), skill confirmed bundled in
`Resources/skill` + ad-hoc signature verifies; **skill status verified headlessly**: fresh→not
installed, install→up to date, tamper→update available, reinstall→up to date; skill installs to a
temp HOME with `__pycache__`/`.pyc` excluded; GUI screenshot shows the "Skill → Claude Code / Codex"
row. `dist/` is gitignored (DMG/app not committed). Python untouched (68/68).
**#33 status**: window + daemon client + picker + start/stop + live session list + SSE live
transcript/preview + tray + hotkey + **skill install/update** + **.app/.dmg packaging** — done.
**Remaining**: Developer-ID signing + notarization + self-contained bundle (#31, needs Alex's
Developer ID), onboarding/Settings, RenderImage eviction, gpui→zed-git for Linux/a11y.

---

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
