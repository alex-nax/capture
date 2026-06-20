#!/usr/bin/env bash
# Build an ASR runtime PACK (#81) — the engine cdylib, named as the GitHub-release asset the daemon
# downloads. The app ships engine-less; `POST /v1/asr/runtimes/install` fetches the newest
# `pack-<id>-v<semver>` release's asset matching this OS/arch into ~/.capture/runtimes/<id>/.
#
#   scripts/build_asr_pack.sh [whisper-metal]        # default: whisper-metal on macOS
#
# Output: dist/packs/<id>-<os>-<arch>.dylib  (asset to attach to a release tagged pack-<id>-v<X.Y.Z>).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ID="${1:-whisper-metal}"

case "$ID" in
  whisper-metal|whisper-cpu) PKG="capture-asr-whisper"; STEM="whisper" ;;
  mlx) echo "mlx pack: the engine cdylib isn't implemented yet (#82)." >&2; exit 2 ;;
  *) echo "unknown runtime id '$ID' (expected whisper-metal / whisper-cpu)." >&2; exit 2 ;;
esac

OS="$(uname -s)"; ARCH="$(uname -m)"
case "$OS" in Darwin) OSTAG="macos"; EXT="dylib"; LIBPREFIX="lib" ;;
              Linux)  OSTAG="linux"; EXT="so";    LIBPREFIX="lib" ;;
              *)      OSTAG="windows"; EXT="dll";  LIBPREFIX="" ;; esac
case "$ARCH" in arm64|aarch64) ARCHTAG="arm64" ;; x86_64|amd64) ARCHTAG="x86_64" ;; *) ARCHTAG="$ARCH" ;; esac

echo "==> Building $PKG (release; the whisper.cpp engine links in statically)…"
cargo build --release -p "$PKG" --manifest-path "$ROOT/Cargo.toml"
DYLIB="$ROOT/target/release/${LIBPREFIX}capture_asr_${STEM}.${EXT}"
[ -f "$DYLIB" ] || { echo "build failed: $DYLIB missing" >&2; exit 1; }

OUT_DIR="$ROOT/dist/packs"; mkdir -p "$OUT_DIR"
ASSET="$OUT_DIR/${ID}-${OSTAG}-${ARCHTAG}.${EXT}"
cp "$DYLIB" "$ASSET"

echo "==> Done: $ASSET ($(du -h "$ASSET" | cut -f1))"
echo
echo "Publish it as a runtime-pack release (its own version line, auto-updated by the daemon):"
echo "  gh release create pack-${ID}-vX.Y.Z \"$ASSET\" --repo alex-nax/capture \\"
echo "    --title \"ASR pack: ${ID} X.Y.Z\" --notes \"whisper.cpp engine for ${OSTAG}-${ARCHTAG}\""
echo
echo "Then a fresh, engine-less app (build_macos_dmg.sh with CAPTURE_BUNDLE_ENGINE=0) installs it via"
echo "the onboarding: GET /v1/asr/runtimes -> POST /v1/asr/runtimes/install {id:\"${ID}\"}."
