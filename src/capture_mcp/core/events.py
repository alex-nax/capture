"""In-process event bus + per-session ``events.jsonl`` writer (M0b, feature #26).

Components push events (``screenshot_taken``, ``transcript_segment``,
``log_line``, ``audio_status``) and ``CaptureSession`` pushes ``state``
transitions, alongside the polled counters that continue to work unchanged.
Future frontends (daemon WS fan-out, GUI) subscribe to the bus; today the one
built-in subscriber is :class:`EventsFileWriter`, which gives any client a
single live-tailable file per session.

Events are plain dicts ``{"t": <iso>, "type": <str>, **payload}`` — JSON-ready,
no schema class. Publishing never raises and never blocks: slow subscribers
drop events (counted on the subscription), because a capture must never stall
on an observer.
"""

from __future__ import annotations

import json
import logging
import queue
import threading
from pathlib import Path
from typing import Callable

from .util import iso, now

log = logging.getLogger(__name__)

#: Per-subscriber buffer; beyond this, new events are dropped for that
#: subscriber (and counted), never blocking the publishing capture loop.
SUBSCRIBER_QUEUE_MAX = 1000


class Subscription:
    """One subscriber's bounded event queue. Obtain via :meth:`EventBus.subscribe`."""

    def __init__(self, bus: "EventBus") -> None:
        self._bus = bus
        self._q: "queue.Queue[dict]" = queue.Queue(maxsize=SUBSCRIBER_QUEUE_MAX)
        self.dropped = 0  # events lost to a full queue (observability, not an error)

    def get(self, timeout: float | None = None) -> dict:
        """Next event; raises ``queue.Empty`` on timeout."""
        return self._q.get(timeout=timeout)

    def close(self) -> None:
        self._bus._unsubscribe(self)


class EventBus:
    """Fan-out of capture events to in-process subscribers.

    ``publish`` is safe to call from any capture thread: it never raises and
    never blocks (see module docstring).
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._subs: list[Subscription] = []

    def subscribe(self) -> Subscription:
        sub = Subscription(self)
        with self._lock:
            self._subs.append(sub)
        return sub

    def _unsubscribe(self, sub: Subscription) -> None:
        with self._lock:
            if sub in self._subs:
                self._subs.remove(sub)

    def publish(self, type_: str, **data) -> None:
        event = {"t": iso(now()), "type": type_, **data}
        with self._lock:
            subs = list(self._subs)
        for sub in subs:
            try:
                sub._q.put_nowait(event)
            except queue.Full:
                sub.dropped += 1
            except Exception:  # publishing must never break a capture loop
                log.exception("event publish failed")


class EventsFileWriter:
    """Tails a session's bus into ``<session_dir>/events.jsonl``.

    The file records the session **lifecycle**: every ``state`` event, a
    counter ``snapshot`` line every ``interval`` seconds, and one final
    snapshot on close. High-volume bus events (``log_line``,
    ``screenshot_taken``, ``transcript_segment``) are deliberately NOT
    persisted here — they already live in ``output.log`` / ``screenshots/`` /
    ``transcript.jsonl``; this file is the cheap thing a client tails to follow
    progress without fs-watching the artifact dirs.

    Snapshot lines are ``{"t", "type": "snapshot", "summary": <summary_fn()>}``.
    """

    PERSISTED_TYPES = ("state",)

    def __init__(
        self,
        path: Path,
        bus: EventBus,
        summary_fn: Callable[[], dict],
        interval: float = 5.0,
    ) -> None:
        self.path = path
        self.interval = max(0.5, float(interval))
        self._summary_fn = summary_fn
        self._sub = bus.subscribe()
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._file = None  # type: ignore[assignment]

    def start(self) -> None:
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self._file = open(self.path, "w", buffering=1)
        except Exception:
            # A broken events file must not break the capture; run file-less.
            log.exception("could not open %s; events file disabled", self.path)
            self._sub.close()
            return
        self._thread = threading.Thread(target=self._run, name="events-writer", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        """Drain pending state events, write a final snapshot, close the file."""
        self._stop.set()
        if self._thread:
            self._thread.join(timeout=5.0)
        self._sub.close()
        if self._file:
            self._write_snapshot()  # final counters, always the last line
            try:
                self._file.flush()
                self._file.close()
            except Exception:
                pass
            self._file = None

    # -- internals --------------------------------------------------------------

    def _write(self, event: dict) -> None:
        try:
            self._file.write(json.dumps(event, ensure_ascii=False) + "\n")
        except Exception:
            log.exception("events.jsonl write failed")

    def _write_snapshot(self) -> None:
        try:
            summary = self._summary_fn()
        except Exception:
            log.exception("snapshot summary failed")
            return
        self._write({"t": iso(now()), "type": "snapshot", "summary": summary})

    def _run(self) -> None:
        next_snap = now() + self.interval
        while not self._stop.is_set():
            try:
                ev = self._sub.get(timeout=max(0.05, next_snap - now()))
            except queue.Empty:
                ev = None
            if ev is not None and ev["type"] in self.PERSISTED_TYPES:
                self._write(ev)
            if now() >= next_snap:
                self._write_snapshot()
                next_snap = now() + self.interval
        # Stop was signalled: drain whatever is already queued so the terminal
        # state events (stopping/stopped/error) always reach the file.
        while True:
            try:
                ev = self._sub.get(timeout=0.05)
            except queue.Empty:
                break
            if ev["type"] in self.PERSISTED_TYPES:
                self._write(ev)
