#!/usr/bin/env bash
# Remove everything the spike installed (agent, app, work dir, TCC entry, identity).
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

log "Stopping + removing launchd agent..."
agent_bootout
rm -f "$AGENT_PLIST"

log "Removing $APP and $WORK ..."
rm -rf "$APP" "$WORK"

log "Resetting the TCC Screen Recording entry for the spike bundle id..."
tccutil reset ScreenCapture "$BUNDLE_ID_APP" 2>/dev/null \
  || warn "tccutil reset failed (fine on some versions; remove the entry in System Settings by hand)"

read -r -p "Also delete the signing identities from the keychain? [y/N] " yn
if [ "${yn:-n}" = "y" ]; then
  for ident in "$IDENTITY" capture-spike-rotated; do
    security delete-identity -c "$ident" 2>/dev/null && log "deleted '$ident'" || true
  done
fi
log "Done."
