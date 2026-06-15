"""Tiny persisted config at ``~/.capture/config.json``.

A flat JSON object of user preferences that outlive a single process — currently
just the active Whisper model (set from the GUI's model manager). Kept dependency-
free and atomic-write; absent/corrupt file reads as ``{}``. The daemon and the
engine both read it, so a model chosen in the GUI applies to new captures started
anywhere (CLI, MCP, GUI).

Resolution precedence for a given setting is the caller's concern (e.g.
``whisper_local`` prefers an explicit arg, then the env var, then this config,
then a hardcoded default) — this module only owns the file.
"""

from __future__ import annotations

import json
import logging
import os
import tempfile
from pathlib import Path

log = logging.getLogger(__name__)


def config_path() -> Path:
    env = os.environ.get("CAPTURE_CONFIG_JSON")
    return Path(env).expanduser() if env else Path.home() / ".capture" / "config.json"


def load() -> dict:
    """The config dict, or ``{}`` if missing/unreadable (never raises)."""
    try:
        data = json.loads(config_path().read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (FileNotFoundError, ValueError, OSError):
        return {}


def get(key: str, default: object = None) -> object:
    return load().get(key, default)


def set_(key: str, value: object) -> None:
    """Merge ``{key: value}`` into the config and write it atomically (0600)."""
    path = config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    data = load()
    data[key] = value
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=".config-", suffix=".json")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)
        os.chmod(tmp, 0o600)
        os.replace(tmp, path)
    except OSError:
        Path(tmp).unlink(missing_ok=True)
        raise
