#!/usr/bin/env bash
# DEV-BOX SCRIPT: build the spike kit tarball for the spare Mac.
# Compiles a UNIVERSAL (arm64 + x86_64) audiocap so the target needs no Xcode CLT,
# then tars the kit. Output: dist/capture-tcc-spike.tar.gz
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="$ROOT/dist"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

echo "==> Building universal audiocap (arm64 + x86_64, min macOS 13)..."
for arch in arm64 x86_64; do
  swiftc -O -target "$arch-apple-macos13.0" -o "$TMP/audiocap.$arch" "$ROOT/helper/audiocap.swift" \
    -framework ScreenCaptureKit -framework AVFoundation -framework CoreMedia
done
lipo -create "$TMP/audiocap.arm64" "$TMP/audiocap.x86_64" -output "$TMP/audiocap"
lipo -info "$TMP/audiocap"

echo "==> Staging kit..."
STAGE="$TMP/capture-tcc-spike"
mkdir -p "$STAGE"
cp "$HERE/README.md" "$STAGE/"
cp "$HERE"/kit/*.sh "$HERE"/kit/captured_spike.py "$STAGE/"
cp "$TMP/audiocap" "$STAGE/audiocap"
chmod +x "$STAGE"/*.sh "$STAGE/audiocap"

mkdir -p "$OUT"
tar -czf "$OUT/capture-tcc-spike.tar.gz" -C "$TMP" capture-tcc-spike
echo "==> Kit: $OUT/capture-tcc-spike.tar.gz ($(du -h "$OUT/capture-tcc-spike.tar.gz" | cut -f1))"
echo "Copy it to the spare Mac (AirDrop/USB/scp), then there:"
echo "  tar -xzf capture-tcc-spike.tar.gz && cd capture-tcc-spike && ./01_setup.sh"
