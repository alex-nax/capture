#!/usr/bin/env python3
"""Create or merge a project's .mcp.json to register the capture-mcp server.

Preserves any existing MCP servers and other keys; only adds/updates the capture entry.

    python configure_mcp.py --bin /path/to/.venv/bin/capture-mcp [--model <hf_repo>]
                            [--project-dir .] [--name capture]
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser(description="Register capture-mcp in .mcp.json")
    ap.add_argument("--bin", required=True, help="path to the capture-mcp executable")
    ap.add_argument("--model", default=None, help="CAPTURE_WHISPER_MODEL to set in env")
    ap.add_argument("--project-dir", default=".", help="project root containing .mcp.json")
    ap.add_argument("--name", default="capture", help="MCP server key (default: capture)")
    a = ap.parse_args()

    path = Path(a.project_dir).expanduser().resolve() / ".mcp.json"

    data: dict = {}
    if path.exists():
        try:
            data = json.loads(path.read_text() or "{}")
        except json.JSONDecodeError as e:
            print(f"refusing to overwrite malformed {path}: {e}")
            return 1
        if not isinstance(data, dict):
            print(f"refusing to overwrite {path}: top level is not an object")
            return 1

    servers = data.setdefault("mcpServers", {})
    entry = servers.get(a.name, {}) if isinstance(servers.get(a.name), dict) else {}
    entry["command"] = a.bin
    env = entry.get("env", {}) if isinstance(entry.get("env"), dict) else {}
    if a.model:
        env["CAPTURE_WHISPER_MODEL"] = a.model
    if env:
        entry["env"] = env
    servers[a.name] = entry

    path.write_text(json.dumps(data, indent=2) + "\n")
    other = [k for k in servers if k != a.name]
    print(f"wrote {path}")
    print(f"  server '{a.name}' -> {a.bin}")
    if a.model:
        print(f"  env CAPTURE_WHISPER_MODEL={a.model}")
    if other:
        print(f"  preserved other servers: {', '.join(other)}")
    print("Reload/restart MCP in your client so the capture_* tools appear.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
