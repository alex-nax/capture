#!/usr/bin/env bash
# Phase 3: THE SPIKE QUESTION — grant Screen Recording and verify attribution.
# Interactive: follow the prompts; screenshots are the evidence.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

log "Current daemon status:"
show_status 10 || true
cp -f "$STATUS_JSON" "$RESULTS/status_before_grant.json" 2>/dev/null || true

cat <<'EOM'

================================ ACTION NEEDED =================================
1. Open System Settings → Privacy & Security → Screen Recording.   Deep link:
     open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"

2. *** RECORD WHAT YOU SEE (screenshot: Cmd+Shift+4, space, click the window): ***
     - Is there an entry for this spike? What is it CALLED?
       (CaptureSpike / captured / audiocap / your terminal — the NAME that appears
        IS the attribution answer.)
     - Save the screenshot; 05_collect.sh will pick up ~/Desktop/*.png from today.

3. Enable the toggle for the spike's entry. If macOS asks to quit & reopen the
   app, accept (the daemon respawns the helper automatically anyway).

4. DO NOT enable your terminal app. The whole point is that the grant must work
   WITHOUT the terminal being involved.
=================================================================================

EOM

# Agent-driven runs set CAPTURE_SPIKE_NONINTERACTIVE=1: instead of blocking on a
# TTY `read`, open the Settings deep link and POLL status.json until the human
# enables the toggle (audio starts flowing) or a timeout. The grant itself still
# requires a human click — an agent can't toggle it — but the script no longer
# hangs waiting for Enter, so Claude can run the whole spike and just relay step 2.
if [ -n "${CAPTURE_SPIKE_NONINTERACTIVE:-}" ]; then
  open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" 2>/dev/null || true
  log "NON-INTERACTIVE: enable the toggle in System Settings now — polling up to 4 min for audio..."
  agent_kickstart 2>/dev/null || true
  wait_for_audio 240 || true
else
  read -r -p "Press Enter once the toggle is ON..."
  log "Restarting the daemon (launchctl kickstart) to pick the grant up cleanly..."
  agent_kickstart
  sleep 6
fi

log "Status after grant:"
show_status 20 || true
cp -f "$STATUS_JSON" "$RESULTS/status_after_grant.json" 2>/dev/null || true

if python3 -c "import json,sys; sys.exit(0 if json.load(open('$STATUS_JSON'))['audio_flowing'] else 1)" 2>/dev/null; then
  log "VERDICT: audio is flowing from a launchd-spawned signed daemon — ATTRIBUTION WORKS."
else
  warn "Audio is NOT flowing yet. Wait ~10s (helper respawn cycle) and run ./03_check.sh again."
  warn "If it still fails: screenshot the Screen Recording list and run ./05_collect.sh anyway —"
  warn "a negative result is exactly what this spike exists to catch BEFORE building #31/#32."
fi

cat <<'EOM'

Also record (macOS 15.x only): over the following days, note when the periodic
"CaptureSpike is requesting to bypass the system private window picker" /
"...still wants to record your screen" re-approval dialog appears and what it
attributes to. Screenshot it. That cadence is finding #4 of the spike.

Next: ./04_update_sim.sh  (grant persistence across an app update)
EOM
