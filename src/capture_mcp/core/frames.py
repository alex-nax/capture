"""Screenshot enumeration + leaf selection for the multimodal index (#44).

Lists a session's ``screenshots/<fs_stamp>.png`` in time order, maps each to its offset
on the session timeline (so frames line up with the transcript), and picks the **leaf
frames** to caption by a tunable sampling rate (decimation) with a hard cap — the cost
knob for the index (see docs/specs/indexing.md, D1).
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from . import retranscribe  # reuse _recover_epoch (transcript/started_at anchoring)

log = logging.getLogger(__name__)

#: Screenshot extensions the indexer accepts (capture format is png/jpg-configurable).
_IMAGE_EXTS = (".png", ".jpg", ".jpeg")


@dataclass
class Frame:
    path: Path
    stamp: float   # unix epoch seconds (parsed from the filename)
    offset: float  # seconds since the session epoch (aligns with transcript offsets)
    iso: str       # ISO-8601 stamp for display


def _parse_fs_stamp(stem: str) -> float | None:
    """Parse a ``2026-06-16T22-01-13.146Z`` screenshot stem back to a unix timestamp."""
    try:
        dt = datetime.strptime(stem, "%Y-%m-%dT%H-%M-%S.%fZ").replace(tzinfo=timezone.utc)
        return dt.timestamp()
    except Exception:
        return None


def list_frames(session_dir: "str | Path") -> list[Frame]:
    """All screenshots in ``session_dir``, oldest first, with timeline offsets. Frames
    whose name doesn't parse are skipped. Empty if there are no screenshots."""
    d = Path(session_dir)
    shots = d / "screenshots"
    if not shots.is_dir():
        return []
    epoch = retranscribe._recover_epoch(d)
    out: list[Frame] = []
    # Screenshots may be png OR jpg/jpeg (the capture format is configurable); the stem is
    # the fs_stamp regardless of extension.
    for f in shots.iterdir():
        if not f.is_file() or f.suffix.lower() not in _IMAGE_EXTS:
            continue
        ts = _parse_fs_stamp(f.stem)
        if ts is None:
            continue
        out.append(Frame(path=f, stamp=ts, offset=round(ts - epoch, 3), iso=_display_iso(f.stem)))
    out.sort(key=lambda fr: fr.stamp)  # fs_stamp already sorts chronologically; be explicit
    return out


def _display_iso(stem: str) -> str:
    """``2026-06-16T22-01-13.146Z`` -> ``2026-06-16T22:01:13.146Z`` (only the time part)."""
    if "T" in stem:
        date, _, time = stem.partition("T")
        return f"{date}T{time.replace('-', ':')}"
    return stem


def select_leaves(frames: list[Frame], sample_rate: float, max_leaves: int) -> list[Frame]:
    """Pick the leaf frames to caption: keep every ``round(1/rate)``-th frame (rate in
    ``(0,1]``; 1.0 = all, 0.5 = every other), then uniformly decimate to ``max_leaves`` if
    still over. Always keeps at least the first frame (and the last, when >1 survives)."""
    if not frames:
        return []
    rate = min(1.0, max(1e-3, float(sample_rate)))
    step = max(1, round(1.0 / rate))
    kept = frames[::step]
    if kept and kept[-1] is not frames[-1] and len(frames) > 1:
        kept.append(frames[-1])  # always anchor the end of the timeline
    if max_leaves and len(kept) > max_leaves:
        # Uniformly sample max_leaves indices across the kept list (endpoints included).
        n = len(kept)
        idx = sorted({round(i * (n - 1) / (max_leaves - 1)) for i in range(max_leaves)})
        kept = [kept[i] for i in idx]
    return kept
