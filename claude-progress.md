# Progress Log

## Session 6 — 2026-06-07
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
