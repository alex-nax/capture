#!/usr/bin/env bash
# Phase 4: simulate an app UPDATE — rebuild the daemon (new binary, new cdhash),
# re-sign with the SAME identity + bundle id, swap the bundle, restart.
# Expectation (the thing we're verifying): the TCC grant PERSISTS — no new
# prompt, audio flows again without touching System Settings.
#
# Optional negative control:  ./04_update_sim.sh --rotate-identity
#   re-signs with a DIFFERENT identity instead; expectation: the grant is LOST
#   (audio stops; Settings shows the toggle off or a fresh entry). This proves
#   the grant really is keyed to the signing identity, not the path.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

ROTATE=0
[ "${1:-}" = "--rotate-identity" ] && ROTATE=1

[ -d "$DIST" ] || die "build dir missing — run ./01_setup.sh first"
python3 -c "import json,sys; sys.exit(0 if json.load(open('$STATUS_JSON'))['audio_flowing'] else 1)" 2>/dev/null \
  || warn "audio is not currently flowing — finish ./03_check.sh first for a meaningful result"

# -- rebuild with a bumped version (binary genuinely changes) ---------------------
NEWVER="1.0.1"; [ "$ROTATE" = 1 ] && NEWVER="1.0.2-rotated"
log "Rebuilding daemon v$NEWVER (PyInstaller)..."
echo "$NEWVER" > "$WORK/build/version.txt"
"$VENV/bin/pyinstaller" --noconfirm --onedir --windowed --name CaptureSpike \
  --osx-bundle-identifier "$BUNDLE_ID_APP" \
  --add-data "$WORK/build/version.txt:." \
  --distpath "$WORK/build/dist" --workpath "$WORK/build/work" \
  --specpath "$WORK/build" "$KIT/captured_spike.py" >"$RESULTS/pyinstaller_update.log" 2>&1
cp "$KIT/audiocap" "$DIST/Contents/MacOS/audiocap"; chmod +x "$DIST/Contents/MacOS/audiocap"

SIGN_AS="$IDENTITY"
if [ "$ROTATE" = 1 ]; then
  SIGN_AS="capture-spike-rotated"
  log "NEGATIVE CONTROL: creating + using a different identity '$SIGN_AS'..."
  CAPTURE_SPIKE_IDENTITY="$SIGN_AS" bash "$KIT/02_install.sh" >/dev/null 2>&1 || true
fi

# -- swap the bundle (stop -> replace -> re-sign -> start), like an updater would --
log "Stopping agent, swapping the app bundle..."
agent_bootout
rm -rf "$APP"
cp -R "$DIST" "$APP"

log "Re-signing with '$SIGN_AS'..."
codesign --force --deep --sign "$SIGN_AS" --identifier "$BUNDLE_ID_APP" "$APP" \
  2>>"$RESULTS/sign_warnings.log"
codesign --verify --deep --strict "$APP" || die "bundle signature does not verify"
record codesign_after_update codesign -dvvv "$APP"

log "Restarting agent..."
agent_bootstrap
sleep 8
show_status 20 || true
SUFFIX="update"; [ "$ROTATE" = 1 ] && SUFFIX="rotation"
cp -f "$STATUS_JSON" "$RESULTS/status_after_$SUFFIX.json" 2>/dev/null || true

if python3 -c "import json,sys; sys.exit(0 if json.load(open('$STATUS_JSON'))['audio_flowing'] else 1)" 2>/dev/null; then
  if [ "$ROTATE" = 1 ]; then
    warn "UNEXPECTED: audio flows after identity rotation — grant did NOT key to the identity. Record this!"
  else
    log "VERDICT: grant SURVIVED the update (same identity + bundle id) — daemon v$NEWVER, audio flowing, no re-prompt."
  fi
else
  if [ "$ROTATE" = 1 ]; then
    log "EXPECTED: audio stopped after identity rotation — grant is keyed to the signing identity. (Screenshot Settings.)"
    log "Restore the working state: re-run ./04_update_sim.sh (without flags) to re-sign with '$IDENTITY'."
  else
    warn "UNEXPECTED: grant LOST across a same-identity update. Wait ~10s, re-check with ./03_check.sh;"
    warn "if it stays broken this is a NEGATIVE finding for the update story — collect and report."
  fi
fi

echo; echo "Next: ./05_collect.sh"
