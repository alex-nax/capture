#!/bin/bash
# Bootstrap the capture dev environment. v3: a single Rust cargo workspace — no Python venv,
# no Swift helper (the daemon does ScreenCaptureKit + AVFoundation natively). Idempotent.
set -e

cd "$(dirname "$0")"

command -v cargo >/dev/null 2>&1 || {
  echo "cargo not found — install Rust from https://rustup.rs, then re-run ./init.sh" >&2
  exit 1
}

echo "Building the workspace (release-free; gpui's first compile is heavy) ..."
cargo build --workspace

echo "Running the workspace tests ..."
cargo test --workspace >/dev/null && echo "  tests passed."

cat <<'EOF'

Environment ready. The whole app is one cargo workspace under crates/:
  • daemon:  cargo run -p capture-daemon      # the /v1 server (binary `captured`)
  • app:     cargo run -p capture-gui         # GPUI window (auto-spawns the daemon)
  • MCP:     cargo run -p capture-mcp          # stdio server (proxies the daemon)
  • tests:   cargo test --workspace

The dev/eval utilities under tools/ and the eval skills are pure-stdlib Python /v1
clients (any python3, no venv) — they proxy a running `captured`.
EOF
