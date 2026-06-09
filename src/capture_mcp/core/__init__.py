"""capture_mcp.core: the capture engine, independent of any frontend.

Everything in this package is callable without the MCP layer: `CaptureSession`
orchestrates one capture; `SessionRegistry` tracks sessions in-process and
rebuilds finished-session history from disk. The MCP server (`capture_mcp.server`)
is one thin client of this package; future frontends (daemon, CLI, GUI) are
expected to consume the same surface (see docs/specs/product-architecture.md).
"""

from .registry import MAX_SESSIONS, SessionRegistry
from .session import CaptureSession

__all__ = ["CaptureSession", "SessionRegistry", "MAX_SESSIONS"]
