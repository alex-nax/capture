#!/usr/bin/env python3
"""Discover a working `capture-mcp` command and optionally register it in a project's `.mcp.json`.

This is the FIRST thing the capture skill should run. It resolves the MCP server command in
priority order and **verifies each candidate with a real MCP `initialize` + `tools/list`
handshake** before accepting it, so a dead path never lands in `.mcp.json`:

  1. The Capture.app bundle  — `…/Capture.app/Contents/Resources/captured/capture-mcp`
     (the daemon-first binary the signed app ships).
  2. `capture-mcp` on PATH.
  3. Known local install locations — `target/release/capture-mcp` under ~/.capture-mcp, ~/capture,
     a CAPTURE_REPO from ~/.capture/config.env (+ a legacy ~/…/.venv/bin entry for old installs).

If none of these answer the handshake, the skill points the user to install Capture.app — which
ships capture-mcp — and then re-runs this.

    python discover_mcp.py                 # discover + verify; print CAPTURE_MCP_BIN
    python discover_mcp.py --configure     # also create/merge ./.mcp.json (capture server)
    python discover_mcp.py --json          # machine-readable result

Exit 0 = a working command was found; 3 = none found (the skill then offers an install).
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

# A minimal MCP stdio handshake: initialize, the initialized notification, then tools/list.
# The capture-mcp binary answers both over stdout and exits on stdin EOF; it does NOT need a
# running daemon for these (only tools/call proxies the daemon), so discovery works any time.
_HANDSHAKE = (
    '{"jsonrpc":"2.0","id":1,"method":"initialize",'
    '"params":{"protocolVersion":"2024-11-05","capabilities":{},'
    '"clientInfo":{"name":"capture-skill-probe","version":"1"}}}\n'
    '{"jsonrpc":"2.0","method":"notifications/initialized"}\n'
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n'
)


def _candidates() -> list[tuple[str, Path]]:
    """(source-label, path) in priority order; existence is checked later."""
    home = Path.home()
    out: list[tuple[str, Path]] = []

    # 1. App bundle (macOS) — the contained, always-present entry for app users.
    for app in (Path("/Applications/Capture.app"), home / "Applications" / "Capture.app"):
        out.append(("app bundle", app / "Contents" / "Resources" / "captured" / "capture-mcp"))

    # 2. On PATH.
    on_path = shutil.which("capture-mcp")
    if on_path:
        out.append(("PATH", Path(on_path)))

    # 3. Known build/install locations (cargo workspace release binary).
    build_roots: list[Path] = []
    # ~/.capture/config.env may point CAPTURE_REPO at a checkout — prefer it.
    env = home / ".capture" / "config.env"
    if env.is_file():
        for line in env.read_text(errors="ignore").splitlines():
            line = line.strip()
            if line.startswith("CAPTURE_REPO="):
                repo = line.split("=", 1)[1].strip().strip('"').strip("'")
                if repo:
                    build_roots.append(Path(os.path.expanduser(repo)))
    build_roots += [home / ".capture-mcp", home / "capture", home / ".capture"]
    for root in build_roots:
        out.append(("install dir", root / "target" / "release" / "capture-mcp"))
        # Legacy pre-Rust (v2) Python venv entry — back-compat with old installs.
        out.append(("venv (legacy)", root / ".venv" / "bin" / "capture-mcp"))

    return out


def verify(path: Path) -> dict | None:
    """Run the MCP handshake against `path`. Returns {server, version, tools:[...]} if it
    speaks MCP and exposes the capture tools, else None."""
    try:
        proc = subprocess.run(
            [str(path)], input=_HANDSHAKE, capture_output=True, text=True, timeout=20
        )
    except (OSError, subprocess.SubprocessError):
        return None

    init = tools = None
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue  # tolerate stray non-JSON (a well-behaved server emits none)
        if msg.get("id") == 1 and isinstance(msg.get("result"), dict):
            init = msg["result"]
        elif msg.get("id") == 2 and isinstance(msg.get("result"), dict):
            tools = msg["result"].get("tools")

    if not init or not isinstance(tools, list) or not tools:
        return None
    names = [t.get("name") for t in tools if isinstance(t, dict)]
    if "capture_start" not in names:  # it answered MCP but isn't capture — reject
        return None
    info = init.get("serverInfo", {}) if isinstance(init.get("serverInfo"), dict) else {}
    return {"server": info.get("name"), "version": info.get("version"), "tools": names}


def discover() -> tuple[str, Path, dict] | None:
    """First candidate that exists, is executable, and passes the handshake."""
    seen: set[Path] = set()
    for source, path in _candidates():
        resolved = path.resolve() if path.exists() else path
        if resolved in seen:
            continue
        seen.add(resolved)
        if not (path.is_file() and os.access(path, os.X_OK)):
            continue
        info = verify(path)
        if info:
            return source, path, info
    return None


_INSTALL_HINT = (
    "capture-mcp isn't available yet. Install Capture.app — it ships capture-mcp — then re-run this:\n"
    "  Download https://github.com/alex-nax/capture/releases/latest (Capture-<ver>.dmg), drag\n"
    "  Capture.app to Applications, launch it, and grant Screen Recording on first launch."
)


def main() -> int:
    ap = argparse.ArgumentParser(description="Discover + verify a capture-mcp command")
    ap.add_argument("--configure", action="store_true", help="also create/merge ./.mcp.json")
    ap.add_argument("--project-dir", default=".", help="project root for --configure")
    ap.add_argument("--name", default="capture", help="MCP server key (default: capture)")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    a = ap.parse_args()

    found = discover()
    if not found:
        if a.json:
            print(json.dumps({"found": False, "hint": _INSTALL_HINT}))
        else:
            print(_INSTALL_HINT, file=sys.stderr)
        return 3

    source, path, info = found
    binp = str(path)

    configured_path = None
    if a.configure:
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import configure_mcp  # local sibling

        configured_path = str(configure_mcp.register(binp, a.project_dir, a.name))

    if a.json:
        print(json.dumps({"found": True, "bin": binp, "source": source,
                          "configured": configured_path, **info}))
    else:
        print(f"CAPTURE_MCP_BIN={binp}")
        print(f"  source: {source} | {info['server']} {info['version']} | "
              f"{len(info['tools'])} tools verified via MCP handshake")
        if configured_path:
            print(f"  registered '{a.name}' in {configured_path} — reload MCP in your client.")
        else:
            here = Path(__file__).with_name("configure_mcp.py")
            print(f"Register it:  python {here} --bin \"{binp}\"")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
