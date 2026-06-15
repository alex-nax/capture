"""capture_mcp.daemon: a local HTTP API that exposes the capture engine.

The `captured` daemon is one of the thin peers in the V2 daemon-peers
architecture (see docs/specs/product-architecture.md): it owns a shared
`SessionRegistry`, so an agent/CLI/GUI all talk to the same live sessions, and
(when packaged + signed) it is the macOS Screen Recording TCC-responsible
process so one grant covers every client (validated by spike #30).

This slice is a stdlib-only HTTP/1.1 `/v1` API bound to 127.0.0.1 with a bearer
token (no new deps; CI-friendly). The unix-domain-socket + WebSocket event
stream evolution is planned (docs/specs/daemon.md). See `server.py` for the
API surface and `client.py` for the client used by the CLI (and, later, the
MCP daemon-first mode).
"""

from __future__ import annotations

from .server import API_VERSION, CaptureDaemon, daemon_json_path, run_daemon

__all__ = ["CaptureDaemon", "run_daemon", "daemon_json_path", "API_VERSION"]
