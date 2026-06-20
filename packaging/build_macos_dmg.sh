#!/usr/bin/env bash
# Build Capture.app and wrap it in a .dmg for macOS testing. The bundle is three
# pieces: the native menu-bar **agent** (CaptureBar, the entry point), the GPUI
# **window** (capture-gui), and the native Rust **daemon** (captured) — v3 cutover
# (#67): no Python, no PyInstaller, no embedded runtime.
#
# IMPORTANT: this build is **ad-hoc signed — NOT Developer-ID signed and NOT
# notarized**. macOS Gatekeeper will warn on first launch; testers must bypass it
# (see README → "Installing the macOS app (unsigned test build)").
#
# Self-contained: the .app bundles the native Rust daemon under
# `Contents/Resources/captured/` — the lean `captured` binary, its dlopen'd ASR
# engine `libcapture_asr_whisper.dylib` (whisper.cpp/Metal), and the `capture-mcp`
# stdio server. No `audiocap` helper (the daemon does ScreenCaptureKit + AVFoundation
# natively). Launching the app runs the menu-bar agent, which spawns the daemon and
# opens the window — no venv/`capture daemon start`. The GUI's model manager
# downloads the ASR *weights* on demand (never bundled).
#
# Output: dist/Capture-<version>.dmg  (dist/ is gitignored)
#
# Env knobs:
#   CAPTURE_GUI_VERSION       bundle version (default 0.2.5)
#   CAPTURE_SIGN_IDENTITY     "Developer ID Application: NAME (TEAMID)" — sign for
#                             distribution (hardened runtime + entitlements + shared
#                             Team ID so the daemon inherits the app's TCC grant, #31).
#                             Unset = ad-hoc (testing only; daemon can't share grants).
#   CAPTURE_NOTARIZE_PROFILE  notarytool keychain-profile name → submit + staple the DMG.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Capture"
BUNDLE_ID="com.capturemcp.gui"
VERSION="${CAPTURE_GUI_VERSION:-0.2.6}"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/$APP_NAME-$VERSION.dmg"
TARGET="$ROOT/target/release"

echo "==> Building the Rust workspace binaries (release; gpui's first compile is heavy)…"
# One cargo build for the whole native app: the GUI window, the daemon, the MCP stdio
# server, and the dlopen'd whisper.cpp ASR engine cdylib. All land in the shared target.
cargo build --release \
  -p capture-gui -p capture-daemon -p capture-mcp -p capture-asr-whisper
BIN="$TARGET/capture-gui"
DAEMON="$TARGET/captured"
MCP="$TARGET/capture-mcp"
ASR_DYLIB="$TARGET/libcapture_asr_whisper.dylib"
for f in "$BIN" "$DAEMON" "$MCP" "$ASR_DYLIB"; do
  [ -e "$f" ] || { echo "build failed: $f missing" >&2; exit 1; }
done

echo "==> Building the native menu-bar agent (CaptureBar.swift)…"
command -v swiftc >/dev/null 2>&1 || { echo "swiftc not found (install Xcode CLT)" >&2; exit 1; }
mkdir -p "$ROOT/agent/build"
AGENT="$ROOT/agent/build/CaptureBar"
swiftc -O -o "$AGENT" "$ROOT/agent/macos/CaptureBar.swift"
[ -x "$AGENT" ] || { echo "agent build failed: $AGENT missing" >&2; exit 1; }

echo "==> Assembling $APP …"
mkdir -p "$DIST"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# The native menu-bar agent is the bundle's entry point; the GPUI window is a
# sibling binary it launches on demand. Both live in Contents/MacOS.
cp "$AGENT" "$APP/Contents/MacOS/CaptureBar"
cp "$BIN" "$APP/Contents/MacOS/capture-gui"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>CaptureBar</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Capture records a target app's audio for transcription.</string>
</dict></plist>
EOF

# Bundle the native Rust daemon under Resources/captured. The GUI's bundled_daemon()
# and CaptureBar both spawn Contents/Resources/captured/captured; the daemon dlopens
# its ASR engine from its own dir (engine_dir() = the binary's dir), so the cdylib
# sits right beside it. capture-mcp ships here too for the agent's MCP wiring.
echo "==> Bundling the native daemon + MCP into Resources/captured …"
DAEMON_DIR="$APP/Contents/Resources/captured"
mkdir -p "$DAEMON_DIR"
cp "$DAEMON" "$DAEMON_DIR/captured"
cp "$MCP" "$DAEMON_DIR/capture-mcp"
# #81: the ASR engine ships as a downloadable runtime PACK (GitHub release), not in the bundle.
# CAPTURE_BUNDLE_ENGINE=1 (default, transitional) still bundles whisper-metal so the app transcribes
# out of the box; set =0 to ship engine-less once the pack is published (the onboarding #83 then
# guides the download into ~/.capture/runtimes/whisper-metal/).
if [ "${CAPTURE_BUNDLE_ENGINE:-1}" = "1" ]; then
  echo "   bundling the whisper-metal engine (CAPTURE_BUNDLE_ENGINE=1, transitional)"
  cp "$ASR_DYLIB" "$DAEMON_DIR/libcapture_asr_whisper.dylib"
else
  echo "   engine-less build (CAPTURE_BUNDLE_ENGINE=0) — runtimes download as packs (#81)"
fi

# Bundle the `capture` skill so the GUI's "Install skill →" buttons work from the
# installed .app (it copies this into ~/.claude/skills/capture etc.).
echo "==> Bundling the capture skill into Resources/skill …"
mkdir -p "$APP/Contents/Resources/skill"
rsync -a --exclude '__pycache__' --exclude '*.pyc' \
  "$ROOT/skills/capture/" "$APP/Contents/Resources/skill/"

# --- Sign inside-out -------------------------------------------------------------
# With CAPTURE_SIGN_IDENTITY set to a "Developer ID Application: NAME (TEAMID)"
# identity, sign for DISTRIBUTION: hardened runtime + entitlements + secure
# timestamp. The shared Team ID is what makes the daemon inherit the app's Screen
# Recording grant (and persist across rebuilds) and lets the build be notarized (#31).
# Without it, ad-hoc (testing only — the daemon can't share the grant).
ENT="$ROOT/packaging/entitlements.plist"
if [ -n "${CAPTURE_SIGN_IDENTITY:-}" ]; then
  echo "==> Signing with Developer ID (hardened runtime): $CAPTURE_SIGN_IDENTITY"
  SIGN=(codesign --force --options runtime --timestamp --entitlements "$ENT" --sign "$CAPTURE_SIGN_IDENTITY")
  SEAL=(codesign --force --options runtime --timestamp --entitlements "$ENT" --sign "$CAPTURE_SIGN_IDENTITY")
else
  echo "==> Signing (ad-hoc — testing only; set CAPTURE_SIGN_IDENTITY for a real build)…"
  SIGN=(codesign --force --sign - --timestamp=none)
  SEAL=(codesign --force --sign -)
fi
# Nested Mach-O first (the dlopen'd ASR engine, if bundled), then each binary, then seal the bundle.
[ -f "$DAEMON_DIR/libcapture_asr_whisper.dylib" ] && "${SIGN[@]}" "$DAEMON_DIR/libcapture_asr_whisper.dylib"
"${SIGN[@]}" "$DAEMON_DIR/captured"
"${SIGN[@]}" "$DAEMON_DIR/capture-mcp"
"${SIGN[@]}" "$APP/Contents/MacOS/capture-gui"
"${SIGN[@]}" "$APP/Contents/MacOS/CaptureBar"
"${SEAL[@]}" "$APP"   # seal the bundle last (NO --deep)
codesign --verify --strict "$APP" && echo "   bundle signature verifies"

echo "==> Building $DMG …"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"   # drag-to-install target
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

# --- Notarize + staple (Developer-ID builds only) --------------------------------
# Needs a stored notarytool credential profile: once, run
#   xcrun notarytool store-credentials "$CAPTURE_NOTARIZE_PROFILE" \
#     --apple-id you@example.com --team-id TEAMID --password <app-specific-password>
# Then build with CAPTURE_NOTARIZE_PROFILE set to submit the DMG, wait, and staple.
if [ -n "${CAPTURE_NOTARIZE_PROFILE:-}" ] && [ -n "${CAPTURE_SIGN_IDENTITY:-}" ]; then
  echo "==> Notarizing $DMG (profile: $CAPTURE_NOTARIZE_PROFILE)…"
  xcrun notarytool submit "$DMG" --keychain-profile "$CAPTURE_NOTARIZE_PROFILE" --wait
  echo "==> Stapling the ticket…"
  xcrun stapler staple "$DMG"
  # Staple the .app too so a drag-installed copy is also notarized-offline.
  xcrun stapler staple "$APP" || true
  xcrun stapler validate "$DMG" && echo "   notarization stapled + validated"
fi

echo "==> Done: $DMG ($(du -h "$DMG" | cut -f1))"
if [ -n "${CAPTURE_SIGN_IDENTITY:-}" ]; then
  echo "   Developer-ID signed${CAPTURE_NOTARIZE_PROFILE:+ + notarized} — no Gatekeeper bypass needed."
else
  echo "   Testers must bypass Gatekeeper — README → 'Installing the macOS app (unsigned test build)'."
fi
echo "   Launch runs the menu-bar agent (CaptureBar) → it spawns the daemon + opens the window."
