#!/usr/bin/env bash
# Phase 5: bundle all evidence into one tarball to bring back to the dev box.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

snapshot_env
cp -f "$STATUS_JSON" "$RESULTS/status_final.json" 2>/dev/null || true
cp -f "$WORK/daemon.err.log" "$WORK/daemon.out.log" "$RESULTS/" 2>/dev/null || true
record launchctl_final launchctl print "gui/$(id -u)/$AGENT_LABEL" || true

# Today's screenshots from the Desktop (the System Settings evidence).
find "$HOME/Desktop" -maxdepth 1 -name "*.png" -newerct "$(date '+%Y-%m-%d')" \
  -exec cp {} "$RESULTS/" \; 2>/dev/null || true

OUT="$HOME/Desktop/tcc-spike-results-$(date +%Y%m%d-%H%M).tar.gz"
tar -czf "$OUT" -C "$WORK" results
log "Evidence bundle: $OUT"
log "Copy it back to the dev repo (AirDrop/USB/scp) and drop it next to spike/tcc-attribution/."
echo
echo "Optional cleanup afterwards: ./uninstall.sh"
