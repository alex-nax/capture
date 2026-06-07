#!/usr/bin/env bash
# Build the ScreenCaptureKit per-app audio helper.
# Output: helper/audiocap (a native arm64/x86_64 binary).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
src="$here/helper/audiocap.swift"
out="$here/helper/audiocap"

if ! command -v swiftc >/dev/null 2>&1; then
  echo "error: swiftc not found. Install Xcode or the Command Line Tools:" >&2
  echo "  xcode-select --install" >&2
  exit 1
fi

echo "Building $out ..."
swiftc -O -o "$out" "$src" \
  -framework ScreenCaptureKit \
  -framework AVFoundation \
  -framework CoreMedia

# Sign with a stable identifier. Ad-hoc (-s -) is the baseline; for a TCC grant
# that PERSISTS across rebuilds, set CODESIGN_IDENTITY to a real signing identity
# (e.g. a self-signed cert or Developer ID): CODESIGN_IDENTITY="My Cert" build_helper.sh
IDENTITY="${CODESIGN_IDENTITY:--}"
codesign --force --sign "$IDENTITY" --identifier com.local.audiocap "$out" \
  && echo "Signed $out (identity: $IDENTITY)" \
  || echo "warning: codesign failed; the helper may be blocked by Gatekeeper/TCC"

echo "Built $out"
echo
echo "IMPORTANT: ScreenCaptureKit needs the Screen Recording permission for the"
echo "process that launches this helper (System Settings > Privacy & Security >"
echo "Screen Recording). On first capture macOS shows an approval prompt; until"
echo "it is granted, startCapture fails with SCStreamError -3805. An ad-hoc"
echo "signature changes on every rebuild, so the grant must be re-approved — use"
echo "CODESIGN_IDENTITY with a stable cert to avoid re-prompting."
