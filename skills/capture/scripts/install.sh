#!/usr/bin/env bash
# Install (or update) capture-mcp into a self-contained location, ready to register
# in an MCP client. Idempotent. Prints CAPTURE_MCP_BIN and CAPTURE_MCP_PY on success.
#
#   bash install.sh
#
# Env overrides:
#   CAPTURE_HOME     install dir            (default: ~/.capture-mcp)
#   CAPTURE_REPO_URL git repo to clone from (default: the public capture repo)
set -euo pipefail

CAPTURE_HOME="${CAPTURE_HOME:-$HOME/.capture-mcp}"
REPO_URL="${CAPTURE_REPO_URL:-https://github.com/alex-nax/capture.git}"
OS="$(uname -s)"; ARCH="$(uname -m)"

command -v git >/dev/null 2>&1 || { echo "error: git is required" >&2; exit 1; }

# 1. Clone or update.
if [ -d "$CAPTURE_HOME/.git" ]; then
  echo "Updating capture-mcp in $CAPTURE_HOME ..."
  git -C "$CAPTURE_HOME" pull --ff-only || echo "  (pull skipped; using existing checkout)"
else
  echo "Cloning capture-mcp into $CAPTURE_HOME ..."
  git clone --depth 1 "$REPO_URL" "$CAPTURE_HOME"
fi
cd "$CAPTURE_HOME"

# 2. Pick the ASR extra: mlx on Apple Silicon, faster-whisper elsewhere.
if [ "$OS" = "Darwin" ] && [ "$ARCH" = "arm64" ]; then EXTRA="mlx"; else EXTRA="whisper"; fi

# 3. Create the venv + install. Prefer uv (and a managed arm64 interpreter on macOS,
#    since a system python may be x86_64 under Rosetta and can't load mlx).
if command -v uv >/dev/null 2>&1; then
  if [ ! -d .venv ]; then
    if [ "$OS" = "Darwin" ] && [ "$ARCH" = "arm64" ]; then
      uv python install 3.12 >/dev/null 2>&1 || true
      uv venv --python 3.12 --python-preference only-managed
    else
      uv venv --python 3.12
    fi
  fi
  echo "Installing capture-mcp[.$EXTRA] (uv) ..."
  VIRTUAL_ENV="$CAPTURE_HOME/.venv" uv pip install -e ".[$EXTRA]"
  PY="$CAPTURE_HOME/.venv/bin/python"
else
  echo "uv not found; falling back to python venv + pip"
  [ -d .venv ] || python3 -m venv .venv
  PY="$CAPTURE_HOME/.venv/bin/python"
  "$PY" -m pip install --upgrade pip >/dev/null
  echo "Installing capture-mcp[.$EXTRA] (pip) ..."
  "$PY" -m pip install -e ".[$EXTRA]"
fi

BIN="$CAPTURE_HOME/.venv/bin/capture-mcp"
ARCH_OK="$("$PY" -c 'import platform; print(platform.machine())' 2>/dev/null || echo '?')"
[ "$OS" = "Darwin" ] && [ "$ARCH" = "arm64" ] && [ "$ARCH_OK" != "arm64" ] && \
  echo "WARNING: venv python is $ARCH_OK, not arm64 — mlx-whisper may not load. Install uv and re-run." >&2

# 4. Build + STABLY SIGN the per-app audio helper (macOS only; optional).
#    Use setup_codesign.sh, not a plain ad-hoc build: an ad-hoc signature changes on
#    every rebuild, so macOS treats the helper as a new binary and re-prompts for the
#    Screen Recording (TCC) grant each time — the "audio failed with permissions on
#    every run" symptom. A stable self-signed identity means you approve it ONCE and
#    the grant sticks. setup_codesign.sh builds the helper first if it's missing.
SIGNED_STABLE=0
if [ "$OS" = "Darwin" ]; then
  if command -v swiftc >/dev/null 2>&1; then
    echo "Building + signing ScreenCaptureKit audio helper (stable identity) ..."
    if bash scripts/setup_codesign.sh; then
      SIGNED_STABLE=1
    else
      echo "  stable signing unavailable; falling back to an ad-hoc build" >&2
      bash scripts/build_helper.sh >/dev/null 2>&1 \
        && echo "  helper built (ad-hoc — the Screen Recording grant will NOT persist across rebuilds)." \
        || echo "  helper build failed (per-app audio unavailable; screenshots/logs still work)."
    fi
  else
    echo "  swiftc not found — skipping audio helper (run: xcode-select --install). Mic fallback still works."
  fi
fi

echo
echo "✓ capture-mcp ready."
echo "CAPTURE_MCP_BIN=$BIN"
echo "CAPTURE_MCP_PY=$PY"
if [ "$OS" = "Darwin" ] && [ "$SIGNED_STABLE" = "1" ]; then
  echo
  echo "macOS one-time permission (per-app audio): approve Screen Recording ONCE — the helper"
  echo "is stably signed, so the grant then persists across rebuilds. To trigger the prompt now:"
  echo "  \"$CAPTURE_HOME/helper/audiocap\" --system   # then enable 'audiocap' + your terminal in"
  echo "  System Settings > Privacy & Security > Screen Recording, and reopen the terminal."
fi
echo
echo "Next: register it in your project's .mcp.json:"
echo "  python $(dirname "$0")/configure_mcp.py --bin \"$BIN\""
