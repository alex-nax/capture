# Spec: Events

_Status: current as of 2026-06-10. Source of truth = the code; update this spec in the same change as the code._

## Purpose

The event surface added in M0b (feature #26): an in-process `EventBus` that
components publish capture events to, and an `EventsFileWriter` that tails the
bus into a per-session `events.jsonl` (state transitions + periodic counter
snapshots). Polling (`capture_status` / `summary()`) is unchanged; events are
push *alongside* poll. Built for the daemon/GUI clients planned in
[product-architecture.md](product-architecture.md) — the daemon's WS fan-out
subscribes to the same bus; today's only built-in subscriber is the file writer.

## Files

- `src/capture_mcp/core/events.py` — the entire scope (`EventBus`,
  `Subscription`, `EventsFileWriter`, `SUBSCRIBER_QUEUE_MAX`).
- Emitters: `core/session.py` (state events, owns the bus + writer),
  `core/screenshots.py`, `core/proc.py`, `core/audio.py` (optional `emit`
  hook each).

## Public contract

- Events are plain dicts: `{"t": <iso>, "type": <str>, **payload}`. No schema
  class; JSON-ready by construction.
- `EventBus.subscribe() -> Subscription`; `Subscription.get(timeout)` (raises
  `queue.Empty`), `.close()`, `.dropped` (count of events lost to a full queue).
- `EventBus.publish(type_, **data)` — **never raises, never blocks**; fans out
  to each subscriber's bounded queue (`SUBSCRIBER_QUEUE_MAX = 1000`), dropping
  (and counting) on overflow.
- Components accept `emit=None` (an `EventBus.publish`-shaped callable);
  `CaptureSession` passes its bus's `publish`. With `emit=None` components are
  silent (zero overhead) — components stay frontend-ignorant.
- `CaptureSession.events` is the public bus; subscribe **before** `start()` to
  observe the full lifecycle.

### Event types

| type | emitter | payload |
|------|---------|---------|
| `state` | session | `state` (starting/running/stopping/stopped/error), `session_id` |
| `screenshot_taken` | screenshots | `path`, `count` |
| `screenshot_error` | screenshots | `errors` |
| `log_line` | proc | `stream` ("out"/"err"), `line` (no trailing newline) |
| `transcript_segment` | audio | the transcript.jsonl record (`start`,`end`,`start_offset`,`end_offset`,`text`) + `count` |
| `audio_status` | audio | `status`, `mode` (on start, on no-data failure, on stop) |
| `snapshot` | EventsFileWriter (file only, not on the bus) | `summary` = `session.summary()` |

## Behavior

- **Bus**: subscriber list under a lock; publish snapshots the list, then
  `put_nowait` per subscriber — `queue.Full` increments `sub.dropped`, any other
  exception is logged and swallowed (a capture loop must never die in an
  observer).
- **events.jsonl writer**: started by `CaptureSession.start()` under the lock,
  right before the `"starting"` state event, so the file records the full
  lifecycle. Persists **only** `state` events plus `snapshot` lines — the
  high-volume types already live in `output.log` / `screenshots/` /
  `transcript.jsonl`; this file is the cheap tail target. Snapshot cadence:
  every `CAPTURE_EVENTS_SNAPSHOT_SECONDS` (default 5.0, min 0.5) while live,
  plus one final snapshot written by `stop()` — always the file's last line.
  On stop, the writer drains already-queued events after the stop flag so the
  terminal state lines (`stopping`/`stopped`/`error`) always reach the file.
- **Session publish points**: `starting` (under lock, after writer start),
  `running`, `stopping`, `stopped`, `error` — each right after the
  corresponding `session.json` write; `stopped`/`error` are followed by
  `_close_events()` (writer stop + final snapshot).

## Invariants & constraints

- **Publish never raises / never blocks** — capture threads call it inline.
- **The events file must not break a capture**: open failure disables the
  writer (logged) and the session continues; write failures are logged and
  swallowed (mirrors `session.json` discipline).
- **events.jsonl never duplicates bulk artifacts** (log lines, screenshots,
  transcript segments) — lifecycle + snapshots only.
- **Subscribers are in-process only.** Cross-process delivery is the daemon's
  job (M2); nothing here touches sockets.
- Components keep working with `emit=None` (all existing call sites outside
  `CaptureSession` are unaffected).

## Failure modes & handling

- Slow/stuck subscriber → its queue fills, events drop, `sub.dropped` counts
  them; other subscribers and the capture are unaffected.
- Writer thread wedged at stop → `join(timeout=5)` then the final snapshot is
  still written and the file closed (single-writer: the thread is dead or the
  join timed out — accepted small race, same class as audio's wedged-reader
  handling).
- `summary_fn` raising inside a snapshot → logged, snapshot skipped.

## Outputs / artifacts

- `<session_dir>/events.jsonl` — one JSON object per line, in order: `state`
  lines and `snapshot` lines (`{"t","type":"snapshot","summary":{...}}`);
  final line is always a snapshot with the final counters.

## Configuration

- `CAPTURE_EVENTS_SNAPSHOT_SECONDS` — snapshot cadence (default 5.0, floor
  0.5). Read at `CaptureSession.start()` time.
- `SUBSCRIBER_QUEUE_MAX = 1000` — module constant, not configurable.

## Known limitations / open items

- Snapshots write unconditionally on the timer (no change detection): ~720
  lines/hour at the default cadence. Fine for files; revisit if cadence drops.
- No replay for late subscribers — subscribe before `start()` or read
  `events.jsonl`. The daemon (M2) will need a small ring buffer for WS
  late-joiners; design then.
- `audio_status` is emitted at three fixed points, not on every internal
  status mutation (e.g. per-chunk `asr-errors` count changes are visible in
  snapshots, not as events).
- Drop accounting is per-subscription only; nothing surfaces `dropped` in
  `summary()` yet.

## Tests

`tests/smoke.py` (35 checks, hermetic; suite sets
`CAPTURE_EVENTS_SNAPSHOT_SECONDS=0.5`): `test_launch_mode` asserts
`events.jsonl` has the state sequence starting→running→stopping→stopped,
periodic + final snapshots with the final snapshot last and matching the final
summary counters; `test_event_bus` subscribes before `start()` and asserts
live delivery of `state`, exactly 6 `log_line` events with both stream tags,
`screenshot_taken` events, and zero drops.
