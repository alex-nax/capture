#!/usr/bin/env bash
# Phase 1: build the spike daemon on THIS Mac (no Xcode needed — audiocap ships prebuilt).
# Installs uv (if missing) -> Python 3.12 venv -> PyInstaller -> onedir `captured`.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

snapshot_env
log "System: $(sw_vers -productVersion) $(uname -m)"

[ -f "$KIT/audiocap" ] || die "prebuilt audiocap missing from the kit (run make_kit.sh on the dev box)"

if ! command -v uv >/dev/null 2>&1; then
  log "Installing uv (standalone, no admin needed)..."
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi
command -v uv >/dev/null 2>&1 || die "uv not on PATH; open a new shell and re-run"

log "Creating venv + PyInstaller..."
uv venv --python 3.12 "$VENV" -q
uv pip install -q --python "$VENV/bin/python" pyinstaller

log "Building CaptureSpike.app (PyInstaller --windowed: codesign-clean bundle layout)..."
rm -rf "$WORK/build"
mkdir -p "$WORK/build"
echo "1.0.0" > "$WORK/build/version.txt"
"$VENV/bin/pyinstaller" --noconfirm --onedir --windowed --name CaptureSpike \
  --osx-bundle-identifier "$BUNDLE_ID_APP" \
  --add-data "$WORK/build/version.txt:." \
  --distpath "$WORK/build/dist" --workpath "$WORK/build/work" \
  --specpath "$WORK/build" "$KIT/captured_spike.py" >"$RESULTS/pyinstaller.log" 2>&1 \
  || die "PyInstaller failed — see $RESULTS/pyinstaller.log"

cp "$KIT/audiocap" "$DIST/Contents/MacOS/audiocap"
chmod +x "$DIST/Contents/MacOS/audiocap"

record build_artifacts ls -la "$DIST/Contents/MacOS"
log "Built: $DIST"
log "Next: ./02_install.sh"
