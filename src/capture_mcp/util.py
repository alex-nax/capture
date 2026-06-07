"""Small shared helpers: timestamps and filesystem-safe names."""

from __future__ import annotations

import time
from datetime import datetime, timezone


def now() -> float:
    """Monotonic-ish wall clock as a unix epoch float (seconds)."""
    return time.time()


def iso(ts: float | None = None) -> str:
    """ISO-8601 UTC timestamp, millisecond precision, e.g. ``2026-06-07T09:47:01.250Z``."""
    dt = datetime.fromtimestamp(ts if ts is not None else now(), tz=timezone.utc)
    return dt.strftime("%Y-%m-%dT%H:%M:%S.") + f"{dt.microsecond // 1000:03d}Z"


def fs_stamp(ts: float | None = None) -> str:
    """Filesystem-safe timestamp for filenames, e.g. ``2026-06-07T09-47-01.250Z``."""
    return iso(ts).replace(":", "-")
