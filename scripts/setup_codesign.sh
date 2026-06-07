#!/usr/bin/env bash
# Set up a STABLE code-signing identity for the ScreenCaptureKit helper.
#
# Why: an ad-hoc signature changes on every rebuild, so macOS treats the helper as
# a new binary and its Screen Recording (TCC) grant has to be re-approved each time
# (and stream starts can fail with SCStreamError -3805). Signing with a stable,
# self-signed certificate gives the helper a fixed identity, so you approve it once
# and the grant sticks across rebuilds.
#
# Idempotent. Safe to run on a fresh machine. Run once, then approve Screen
# Recording for the helper.
#
#   bash scripts/setup_codesign.sh
#
# Override the certificate common name with CAPTURE_CODESIGN_CN.
set -euo pipefail

CERT_NAME="${CAPTURE_CODESIGN_CN:-capture-mcp-codesign}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$ROOT/helper/audiocap"
KEYCHAIN="$(security default-keychain | tr -d ' "')"
[ -n "$KEYCHAIN" ] || KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

have_identity() { security find-identity -v -p codesigning 2>/dev/null | grep -q "$CERT_NAME"; }

if have_identity; then
  echo "✓ Code-signing identity '$CERT_NAME' already exists."
else
  echo "Creating self-signed code-signing certificate '$CERT_NAME' in $KEYCHAIN ..."
  TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
  cat > "$TMP/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions    = v3
prompt             = no
[dn]
CN = $CERT_NAME
[v3]
basicConstraints   = critical,CA:false
keyUsage           = critical,digitalSignature
extendedKeyUsage   = critical,codeSigning
EOF
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" -config "$TMP/cert.cnf" >/dev/null 2>&1
  openssl pkcs12 -export -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -name "$CERT_NAME" -out "$TMP/id.p12" -passout pass: >/dev/null 2>&1
  # Import the identity and authorise codesign to use the private key.
  security import "$TMP/id.p12" -k "$KEYCHAIN" -P "" -T /usr/bin/codesign >/dev/null
  # Avoid codesign's "wants to use key" prompt (needs your login-keychain password).
  echo "  (you may be asked for your login keychain password)"
  security set-key-partition-list -S apple-tool:,apple: -s -k "" "$KEYCHAIN" >/dev/null 2>&1 \
    || echo "  note: could not set partition list non-interactively; codesign may prompt once (click 'Always Allow')."
  if have_identity; then echo "✓ Created '$CERT_NAME'."; else echo "✗ Failed to create identity." >&2; exit 1; fi
fi

if [ ! -x "$HELPER" ]; then
  echo "Helper not built yet — building it first..."
  bash "$ROOT/scripts/build_helper.sh"
fi

echo "Signing $HELPER with '$CERT_NAME' ..."
codesign --force --options runtime --sign "$CERT_NAME" --identifier com.local.audiocap "$HELPER"
codesign -dvvv "$HELPER" 2>&1 | grep -E "Authority|Identifier|TeamIdentifier" | sed 's/^/  /' || true

cat <<EOF

✓ Helper signed with a stable identity.

One-time, on this machine:
  1. Trigger the permission prompt:   ./helper/audiocap --system
  2. System Settings → Privacy & Security → Screen Recording → enable 'audiocap'
     (and ensure your terminal app is enabled), then quit & reopen the terminal.

From now on, rebuild with the SAME identity so the grant persists:
  CODESIGN_IDENTITY="$CERT_NAME" bash scripts/build_helper.sh

The helper also auto-reconnects through transient SCStreamError -3805 interruptions,
so background capture survives Space/window switches even before you grant permission.
EOF
