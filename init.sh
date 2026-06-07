#!/bin/bash
# Bootstrap the capture-mcp dev environment. Idempotent.
set -e

cd "$(dirname "$0")"

# 1. arm64 venv (REQUIRED: system python3 here is x86_64 Rosetta; mlx needs arm64).
if [ ! -d .venv ]; then
  echo "Creating arm64 venv via uv..."
  uv python install 3.12 >/dev/null 2>&1 || true
  uv venv --python 3.12 --python-preference only-managed
fi
# shellcheck disable=SC1091
source .venv/bin/activate

ARCH=$(python -c "import platform; print(platform.machine())")
echo "venv python arch: $ARCH"
[ "$ARCH" = "arm64" ] || echo "WARNING: venv is $ARCH; mlx-whisper requires arm64."

# 2. Install the package + the Apple-Silicon ASR extra.
echo "Installing package (.[mlx]) ..."
uv pip install -e '.[mlx]'

# 3. Build the ScreenCaptureKit per-app audio helper (needs Xcode CLT).
if command -v swiftc >/dev/null 2>&1; then
  bash scripts/build_helper.sh || echo "helper build failed (per-app audio unavailable)"
else
  echo "swiftc not found; skipping helper build (run: xcode-select --install)"
fi

# 4. Smoke test (hermetic; no permissions/GPU required).
echo "Running smoke test ..."
python tests/smoke.py

echo "Environment ready. Run the server with: capture-mcp"
