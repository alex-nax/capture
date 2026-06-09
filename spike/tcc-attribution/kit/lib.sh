# Shared paths/helpers for the TCC attribution spike. Sourced by the numbered scripts.
set -euo pipefail

KIT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$HOME/CaptureSpike"                 # build + results live here
RESULTS="$WORK/results"
VENV="$WORK/venv"
DIST="$WORK/build/dist/CaptureSpike.app"  # PyInstaller .app output
APP="$HOME/Applications/CaptureSpike.app"
AGENT_LABEL="com.capturemcp.spike"
AGENT_PLIST="$HOME/Library/LaunchAgents/$AGENT_LABEL.plist"
BUNDLE_ID_APP="com.capturemcp.spike.app"
IDENTITY="${CAPTURE_SPIKE_IDENTITY:-capture-spike-codesign}"
STATUS_JSON="$WORK/status.json"

mkdir -p "$RESULTS"

log()  { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n'  "$*"; }
die()  { printf '\033[1;31mXX\033[0m %s\n'  "$*" >&2; exit 1; }

record() {  # record <name> <command...>  -> tee output into results/
  local name="$1"; shift
  { echo "\$ $*"; "$@" 2>&1; } | tee "$RESULTS/$name.txt"
}

snapshot_env() {
  { sw_vers; echo; uname -m; echo; date -u +"%Y-%m-%dT%H:%M:%SZ"; } > "$RESULTS/sysinfo.txt"
}

agent_bootout() {
  launchctl bootout "gui/$(id -u)/$AGENT_LABEL" 2>/dev/null || true
}

agent_bootstrap() {
  launchctl bootstrap "gui/$(id -u)" "$AGENT_PLIST"
}

agent_kickstart() {
  launchctl kickstart -k "gui/$(id -u)/$AGENT_LABEL"
}

show_status() {  # pretty-print the daemon's status.json (waits up to $1 seconds for it)
  local wait="${1:-10}" i=0
  while [ ! -f "$STATUS_JSON" ] && [ "$i" -lt "$wait" ]; do sleep 1; i=$((i+1)); done
  [ -f "$STATUS_JSON" ] || { warn "no status.json yet at $STATUS_JSON"; return 1; }
  python3 -c "import json,sys;print(json.dumps(json.load(open('$STATUS_JSON')),indent=2))" 2>/dev/null \
    || cat "$STATUS_JSON"
}
