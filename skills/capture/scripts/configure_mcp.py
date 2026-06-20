#!/usr/bin/env python3
"""Create or merge a project's .mcp.json to register the capture-mcp server.

Preserves any existing MCP servers and other keys; only adds/updates the capture entry.
The --bin path is the native Rust capture-mcp binary — the one bundled in the app
(`Capture.app/Contents/Resources/captured/capture-mcp`) or a source build
(`target/release/capture-mcp`); no Python/venv assumptions.

    python configure_mcp.py --bin /path/to/capture-mcp
                            [--args ...] [--project-dir .] [--name capture]

Prefer `discover_mcp.py --configure`, which finds + verifies the command first.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def register(
    bin_path: str,
    project_dir: str = ".",
    name: str = "capture",
    args: list[str] | None = None,
) -> Path:
    """Write/merge `<project_dir>/.mcp.json` so server `name` runs `bin_path` (+ optional `args`).

    Preserves other servers + top-level keys. A command-only entry drops any stale `args`
    (so switching from a `<cmd> sub` form to a standalone binary can't leave a broken argv).
    Returns the .mcp.json path. Raises ValueError on a malformed existing file.
    """
    path = Path(project_dir).expanduser().resolve() / ".mcp.json"

    data: dict = {}
    if path.exists():
        try:
            data = json.loads(path.read_text() or "{}")
        except json.JSONDecodeError as e:
            raise ValueError(f"refusing to overwrite malformed {path}: {e}") from e
        if not isinstance(data, dict):
            raise ValueError(f"refusing to overwrite {path}: top level is not an object")

    servers = data.setdefault("mcpServers", {})
    entry = servers.get(name, {}) if isinstance(servers.get(name), dict) else {}
    entry["command"] = bin_path
    if args:
        entry["args"] = list(args)
    else:
        entry.pop("args", None)  # a standalone binary takes no args; clear any stale ones
    servers[name] = entry

    path.write_text(json.dumps(data, indent=2) + "\n")
    return path


def main() -> int:
    ap = argparse.ArgumentParser(description="Register capture-mcp in .mcp.json")
    ap.add_argument("--bin", required=True, help="path to the capture-mcp executable")
    ap.add_argument("--args", nargs="*", default=None, help="optional args (e.g. a subcommand)")
    ap.add_argument("--project-dir", default=".", help="project root containing .mcp.json")
    ap.add_argument("--name", default="capture", help="MCP server key (default: capture)")
    a = ap.parse_args()

    try:
        path = register(a.bin, a.project_dir, a.name, a.args)
    except ValueError as e:
        print(e)
        return 1

    data = json.loads(path.read_text())
    other = [k for k in data.get("mcpServers", {}) if k != a.name]
    print(f"wrote {path}")
    print(f"  server '{a.name}' -> {a.bin}" + (f" {a.args}" if a.args else ""))
    if other:
        print(f"  preserved other servers: {', '.join(other)}")
    print("Reload/restart MCP in your client so the capture_* tools appear.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
