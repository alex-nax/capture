#!/usr/bin/env bash
# Phase 2: create the stable signing identity, assemble + sign CaptureSpike.app,
# install the launchd agent, and start the daemon.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

[ -d "$DIST" ] || die "app not built — run ./01_setup.sh first"

# -- 1. stable self-signed identity (same mechanism as the repo's setup_codesign.sh;
#       the -legacy/-passphrase handling covers both LibreSSL and OpenSSL 3) -------
have_identity() { security find-identity -p codesigning 2>/dev/null | grep -q "$IDENTITY"; }
if have_identity; then
  log "Identity '$IDENTITY' already exists."
else
  log "Creating self-signed code-signing identity '$IDENTITY'..."
  KEYCHAIN="$(security default-keychain | tr -d ' "')"
  [ -n "$KEYCHAIN" ] || KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
  TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
  cat > "$TMP/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions    = v3
prompt             = no
[dn]
CN = $IDENTITY
[v3]
basicConstraints   = critical,CA:false
keyUsage           = critical,digitalSignature
extendedKeyUsage   = critical,codeSigning
EOF
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" -config "$TMP/cert.cnf" >/dev/null 2>&1
  P12_LEGACY=""; openssl pkcs12 -help 2>&1 | grep -q -- -legacy && P12_LEGACY="-legacy"
  openssl pkcs12 -export $P12_LEGACY -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -name "$IDENTITY" -out "$TMP/id.p12" -passout "pass:capture-spike" >/dev/null 2>&1
  security import "$TMP/id.p12" -k "$KEYCHAIN" -P "capture-spike" -T /usr/bin/codesign >/dev/null
  echo "  (you may be asked for your login keychain password — click 'Always Allow')"
  security set-key-partition-list -S apple-tool:,apple: -s -k "" "$KEYCHAIN" >/dev/null 2>&1 \
    || warn "could not set partition list non-interactively; codesign may prompt once"
  have_identity || die "failed to create identity"
fi

# -- 2. install + sign CaptureSpike.app -------------------------------------------
# PyInstaller already produced a codesign-clean .app layout; we deep-sign it with
# the stable identity. (No hardened runtime in the spike: one variable at a time —
# hardened-runtime + entitlements are #31 packaging work, not attribution.)
log "Installing $APP ..."
agent_bootout
mkdir -p "$(dirname "$APP")"
rm -rf "$APP"
cp -R "$DIST" "$APP"

log "Signing with '$IDENTITY' (this can take a minute)..."
codesign --force --deep --sign "$IDENTITY" --identifier "$BUNDLE_ID_APP" "$APP" \
  2>>"$RESULTS/sign_warnings.log"
codesign --verify --deep --strict "$APP" || die "bundle signature does not verify"
record codesign_app codesign -dvvv "$APP"

# -- 4. launchd agent ------------------------------------------------------------
log "Installing launchd agent $AGENT_LABEL ..."
mkdir -p "$(dirname "$AGENT_PLIST")"
cat > "$AGENT_PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$AGENT_LABEL</string>
  <key>ProgramArguments</key>
  <array><string>$APP/Contents/MacOS/CaptureSpike</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
  <key>StandardOutPath</key><string>$WORK/daemon.out.log</string>
  <key>StandardErrorPath</key><string>$WORK/daemon.err.log</string>
</dict></plist>
EOF
agent_bootstrap
record launchctl_print launchctl print "gui/$(id -u)/$AGENT_LABEL" || true

log "Daemon starting. Waiting for first status..."
sleep 4
show_status 15 || true

cat <<'DONE'

Installed. The daemon is running under launchd (NOT under this terminal) and is
respawning the audiocap helper, which will fail with a permission error until
the grant is given. Next: ./03_check.sh
DONE
