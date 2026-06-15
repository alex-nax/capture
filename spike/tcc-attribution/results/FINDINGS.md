# TCC Attribution Spike — FINDINGS (feature #30)

**Run:** 2026-06-15, spare Mac, **macOS 26.5.1 (build 25F80), arm64**. Evidence in this dir
(`status_*.json`, `sysinfo.txt`, `launchctl_print.txt`); raw bundle
`tcc-spike-results-20260615-1507.tar.gz`.

## VERDICT: ✅ PASS — the daemon-peers architecture is validated.

A launchd user-agent (`gui/501/com.capturemcp.spike`, `state = active`) spawned the signed
`CaptureSpike.app` PyInstaller daemon, which spawned `audiocap` — and **audio flowed from that
chain with no terminal involved**. So a launchd-spawned, code-signed bundle **is** the
Screen-Recording TCC-responsible process. This is the load-bearing assumption behind #31/#32.

### 1. Attribution works
`status_after_grant.json`: `audio_flowing: true`, `permission_error: false`,
`ready_line: "READY rate=16000 channels=1 fmt=s16le target=system"`, `last_stderr: "audio flowing"`.
`launchctl print` confirms the daemon ran from `…/CaptureSpike.app/Contents/MacOS/CaptureSpike`
as an active launchd agent. The terminal was never granted.

### 2. Grant SURVIVES a same-identity update
`status_after_update.json`: after rebuilding (new binary/cdhash), re-signing with the **same**
identity + bundle id (`com.capturemcp.spike.app`), and restarting → `daemon_version: 1.0.1`,
`audio_flowing: true`, **`respawns: 0`**, helper still alive (`helper_rc: null`). **No re-prompt.**
→ App updates that preserve the signing identity + bundle id keep the grant.

### 3. Negative control confirms the grant keys to the signing identity
`status_after_rotation.json` / `status_final.json`: re-signing the SAME bundle id with a
**different** identity (`1.0.2-rotated`) → `audio_flowing: false`,
`last_stderr: "startup failed: The user declined TCCs for application, window, display capture"`.
→ The grant is keyed to the **code-signing identity**, not the path/bundle-id alone.

## Consequences for the product (fold into #31/#32)
- **A stable signing identity across updates is mandatory.** Ship the real engine/daemon/helper
  with a **Developer ID** cert (stable Team ID + `CFBundleIdentifier`); never rotate it casually —
  a cert/identity change re-prompts every user. (The spike used a self-signed identity with no Team
  ID and it still persisted across same-identity updates and broke on rotation — so the mechanism is
  the designated requirement / identity, exactly as `product-architecture.md` assumed.)
- **macOS 26 caveat — `SCShareableContent` enumeration is intermittently flaky.** In the original
  (1.0.0) phase, `status_before_grant.json` shows audio flowing **but** `respawns: 10` and last
  `helper_rc: 5` (audiocap's "shareable content enumeration failed" exit). The respawn loop rode
  through it (net audio flowed), and post-update (1.0.1) it was stable (`respawns: 0`). **Follow-up:**
  give `audiocap.swift` a bounded retry on the `SCShareableContent` enumeration failure (currently
  `exit 5`) instead of dying, so the real helper doesn't lean on a supervisor's respawn on macOS 26.

## Note
The spike intentionally used a self-signed identity + no hardened runtime (one variable at a time;
Developer ID + notarization + hardened-runtime entitlements are #31 packaging concerns). The
attribution + persistence conclusions transfer to Developer ID (which only *strengthens* the
designated requirement by pinning Team ID).
