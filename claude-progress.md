# Progress Log

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
