#!/usr/bin/env bash
# Build Capture.app (the GPUI GUI) and wrap it in a .dmg for macOS testing.
#
# IMPORTANT: this build is **ad-hoc signed — NOT Developer-ID signed and NOT
# notarized**. macOS Gatekeeper will warn on first launch; testers must bypass it
# (see README → "Installing the macOS app (unsigned test build)"). The GUI is a
# thin daemon client: a running `captured` daemon is still required
# (`capture daemon start`); the .app does not bundle the Python engine.
#
# Output: dist/Capture-<version>.dmg  (dist/ is gitignored)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Capture"
BUNDLE_ID="com.capturemcp.gui"
VERSION="${CAPTURE_GUI_VERSION:-0.1.0}"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/$APP_NAME-$VERSION.dmg"

echo "==> Building the GUI (release; gpui's first compile is heavy)…"
cargo build --release --manifest-path "$ROOT/gui/Cargo.toml"
BIN="$ROOT/gui/target/release/capture-gui"
[ -x "$BIN" ] || { echo "build failed: $BIN missing" >&2; exit 1; }

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

# Bundle the `capture` skill so the GUI's "Install skill →" buttons work from the
# installed .app (it copies this into ~/.claude/skills/capture etc.).
echo "==> Bundling the capture skill into Resources/skill …"
mkdir -p "$APP/Contents/Resources/skill"
rsync -a --exclude '__pycache__' --exclude '*.pyc' \
  "$ROOT/skills/capture/" "$APP/Contents/Resources/skill/"

# Ad-hoc signature (identity "-"): runnable on Apple Silicon, but Gatekeeper
# still treats it as an unidentified developer. Developer ID + notarization is #31.
echo "==> Ad-hoc signing (NOT Developer-ID / NOT notarized)…"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP" && echo "   signature verifies (ad-hoc)"

echo "==> Building $DMG …"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"   # drag-to-install target
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

echo "==> Done: $DMG ($(du -h "$DMG" | cut -f1))"
echo "   Testers must bypass Gatekeeper — README → 'Installing the macOS app (unsigned test build)'."
echo "   The GUI needs a running daemon: \`capture daemon start\`."
