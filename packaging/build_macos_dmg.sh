#!/usr/bin/env bash
# Build Capture.app (the GPUI GUI + the bundled `captured` daemon) and wrap it in
# a .dmg for macOS testing.
#
# IMPORTANT: this build is **ad-hoc signed — NOT Developer-ID signed and NOT
# notarized**. macOS Gatekeeper will warn on first launch; testers must bypass it
# (see README → "Installing the macOS app (unsigned test build)").
#
# Self-contained: the .app bundles a PyInstaller-frozen daemon under
# `Contents/Resources/captured/` (with the signed `audiocap` helper beside it), so
# the GUI auto-spawns its own daemon — no separate venv/`capture daemon start`.
# The frozen daemon does capture + raw audio; transcription still needs a
# configured ASR backend (mlx is excluded to keep the bundle small).
#
# Output: dist/Capture-<version>.dmg  (dist/ is gitignored)
#
# Env knobs:
#   CAPTURE_GUI_VERSION   bundle version (default 0.1.0)
#   CAPTURE_SKIP_FREEZE=1 reuse an existing freeze (fast GUI-only iteration)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Capture"
BUNDLE_ID="com.capturemcp.gui"
VERSION="${CAPTURE_GUI_VERSION:-0.1.0}"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/$APP_NAME-$VERSION.dmg"
VENV_PY="$ROOT/.venv/bin/python"
FREEZE_DIR="$ROOT/packaging/build/dist/captured"

echo "==> Building the GUI (release; gpui's first compile is heavy)…"
cargo build --release --manifest-path "$ROOT/gui/Cargo.toml"
BIN="$ROOT/gui/target/release/capture-gui"
[ -x "$BIN" ] || { echo "build failed: $BIN missing" >&2; exit 1; }

# --- Freeze the Python daemon (PyInstaller onedir) -------------------------------
# Bundles the on-device ASR runtime (mlx-whisper) so the installed app transcribes
# locally — the GUI's model manager downloads the *weights* on demand (never
# bundled). torch/faster-whisper/riva (CUDA/cross-platform) are excluded.
# NOTE: captured_main.py calls multiprocessing.freeze_support() — numba (a
# mlx_whisper dep) uses multiprocessing, and without it a frozen child re-runs the
# entry and starts a rogue second daemon.
if [ "${CAPTURE_SKIP_FREEZE:-0}" = "1" ] && [ -x "$FREEZE_DIR/captured" ]; then
  echo "==> CAPTURE_SKIP_FREEZE=1 — reusing existing freeze at $FREEZE_DIR"
else
  [ -x "$VENV_PY" ] || { echo "missing venv python: $VENV_PY (run ./init.sh)" >&2; exit 1; }
  "$VENV_PY" -c "import PyInstaller" 2>/dev/null || {
    echo "==> Installing PyInstaller into the venv…"
    uv pip install --python "$VENV_PY" pyinstaller >/dev/null
  }
  echo "==> Freezing the daemon + mlx ASR runtime (PyInstaller onedir; ~390 MB)…"
  "$VENV_PY" -m PyInstaller --noconfirm --onedir --name captured \
    --distpath "$ROOT/packaging/build/dist" \
    --workpath "$ROOT/packaging/build/work" \
    --specpath "$ROOT/packaging/build" \
    --hidden-import capture_mcp.core.platform.macos \
    --hidden-import capture_mcp.core.asr.whisper_local \
    --hidden-import capture_mcp.core.asr.openai_compat \
    --collect-all Quartz \
    --collect-all mlx --collect-all mlx_whisper --collect-all huggingface_hub \
    --collect-all tiktoken --collect-all numba \
    --exclude-module torch --exclude-module faster_whisper --exclude-module riva \
    "$ROOT/packaging/captured_main.py"
fi
[ -x "$FREEZE_DIR/captured" ] || { echo "freeze failed: $FREEZE_DIR/captured missing" >&2; exit 1; }

# Best-effort: prove the frozen mlx runtime works (Metal kernel + a whisper-tiny
# transcription). Non-fatal — a cold cache needs network for the tiny model.
# CAPTURE_ASR_SELFTEST=0 skips it.
if [ "${CAPTURE_ASR_SELFTEST:-1}" = "1" ]; then
  echo "==> Verifying the frozen ASR runtime (Metal + whisper-tiny; best-effort)…"
  if "$FREEZE_DIR/captured" --asr-selftest; then
    echo "   ASR self-test passed."
  else
    echo "   ⚠ ASR self-test did not pass (offline / first-run download?) — bundle still built." >&2
  fi
fi

echo "==> Assembling $APP …"
mkdir -p "$DIST"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/capture-gui"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>capture-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF

# Bundle the frozen daemon under Resources/captured (the GUI's bundled_daemon()
# looks for Contents/Resources/captured/captured relative to the GUI binary).
echo "==> Bundling the frozen daemon into Resources/captured …"
rsync -a "$FREEZE_DIR/" "$APP/Contents/Resources/captured/"

# Place the signed ScreenCaptureKit helper beside the frozen daemon — the engine's
# helper_path() resolves `audiocap` next to sys.executable (the frozen binary).
# cp preserves its embedded `com.local.audiocap` signature (stable TCC identity).
if [ -x "$ROOT/helper/audiocap" ]; then
  echo "==> Placing the signed audiocap helper beside the daemon …"
  cp "$ROOT/helper/audiocap" "$APP/Contents/Resources/captured/audiocap"
else
  echo "   (no helper/audiocap — per-app audio will fall back to ffmpeg/mic)"
fi

# Bundle the `capture` skill so the GUI's "Install skill →" buttons work from the
# installed .app (it copies this into ~/.claude/skills/capture etc.).
echo "==> Bundling the capture skill into Resources/skill …"
mkdir -p "$APP/Contents/Resources/skill"
rsync -a --exclude '__pycache__' --exclude '*.pyc' \
  "$ROOT/skills/capture/" "$APP/Contents/Resources/skill/"

# --- Sign inside-out (ad-hoc), preserving the helper's stable identity -----------
# Apple discourages `--deep`; we sign nested Mach-Os first, then the bundle. The
# `audiocap` helper KEEPS its `com.local.audiocap` (capture-mcp-codesign) signature
# from the cp above, so its Screen-Recording/audio TCC grant persists across builds.
echo "==> Signing (ad-hoc; helper keeps its stable identity)…"
# Every dylib/.so the frozen daemon loads + the frozen binary itself.
find "$APP/Contents/Resources/captured" \
  -type f \( -name '*.so' -o -name '*.dylib' \) \
  -exec codesign --force --sign - --timestamp=none {} +
codesign --force --sign - --timestamp=none "$APP/Contents/Resources/captured/captured"
codesign --force --sign - --timestamp=none "$APP/Contents/MacOS/capture-gui"
# Seal the bundle last (NO --deep → the helper's signature is left intact).
codesign --force --sign - "$APP"
codesign --verify --strict "$APP" && echo "   bundle signature verifies (ad-hoc)"

echo "==> Building $DMG …"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"   # drag-to-install target
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

echo "==> Done: $DMG ($(du -h "$DMG" | cut -f1))"
echo "   Testers must bypass Gatekeeper — README → 'Installing the macOS app (unsigned test build)'."
echo "   Self-contained: the GUI auto-spawns the bundled daemon (no \`capture daemon start\`)."
