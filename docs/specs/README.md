# Specs

Per-scope specifications for capture-mcp. **Specs are mandatory** — they are the
source of *intent*; the code is the source of *reality*; the two must agree.

## The rule (also in [`AGENTS.md`](../../AGENTS.md))

**When you implement or change behavior, update the matching spec in the SAME change.**
A change where the code moved but the spec didn't is incomplete. Adding a new scope/module
means adding a new `docs/specs/<scope>.md` and linking it in the table below. This is what
keeps the harness stable across sessions: a new agent reads the spec for intent, the code
for reality, and verifies they match.

Workflow:
1. **Before coding** — read the relevant spec to load the contract.
2. **While coding** — keep *Public contract*, *Behavior*, *Invariants*, *Failure modes* in sync.
3. **After coding** — confirm the spec matches the code; commit code + spec + `claude-progress.md` together.

## Index

| Scope | Spec | Covers |
|-------|------|--------|
| MCP server | [mcp-server.md](mcp-server.md) | FastMCP stdio entrypoint; async `capture_start`/`capture_stop`/`capture_status`/`list_windows`; exactly-one-target validation; bounded session registry |
| Session | [session.md](session.md) | `CaptureSession` lifecycle (created→starting→running→stopping→stopped/error); start/stop rollback; session dir layout; `summary()` + `session.json` |
| Events | [events.md](events.md) | `EventBus` (publish never raises/blocks; bounded per-subscriber queues) + `EventsFileWriter` (`events.jsonl`: state transitions + counter snapshots, `CAPTURE_EVENTS_SNAPSHOT_SECONDS`); component `emit` hooks |
| Daemon + CLI | [daemon.md](daemon.md) | `captured` local HTTP `/v1` API (127.0.0.1 + bearer token, `~/.capture/daemon.json`) over the shared engine; `capture` CLI client; stdlib-only. UDS/WebSocket/MCP-daemon-first are planned |
| Session registry | [session-registry.md](session-registry.md) | `SessionRegistry` (core/registry.py): bounded live tracking; append-only `sessions.jsonl` index (`CAPTURE_SESSION_INDEX`); history rebuild on restart (stopped/error/interrupted/unknown) |
| Screenshots | [screenshots.md](screenshots.md) | Grid-scheduled `screencapture`; window targeting + `_last_wid` cross-Space cache; whole-screen fallback; `parse_resolution`; sips resize/convert; jpeg quality; rc=0-but-no-file quirk |
| Process logs | [process-logs.md](process-logs.md) | Launch-mode `stdout.log`/`stderr.log`/merged `output.log`; pump threads; teardown ordering; attach-mode stdio limitation |
| Audio | [audio.md](audio.md) | Source selection (app helper / mic ffmpeg); 16 kHz mono s16le contract; chunking + offsets; first-byte epoch anchoring; transcripts; reader/stop ordering; failure-status surfacing |
| ASR | [asr.md](asr.md) | `ASRBackend` interface; `create()` factory + auto-fallback; local Whisper (mlx/faster); remote OpenAI-compatible endpoint (stdlib-only, `CAPTURE_OPENAI_ASR_URL`); remote Riva/Nemotron |
| Windows | [windows.md](windows.md) | Quartz `CGWindowList` discovery (pid/app → `CGWindowID`); layer-0/largest-first; on-screen→all-windows cross-Space fallback |
| Helper contract (frozen) | [helper-contract.md](helper-contract.md) | Process-boundary protocol for ALL audio helpers: argv; PCM-only stdout (16 kHz mono s16le); `READY` scanned (not line 1) on stderr; exit codes; `-3801`/`-3803` fatal vs `-3805` reconnect |
| ScreenCaptureKit helper | [screencapturekit-helper.md](screencapturekit-helper.md) | `audiocap` CLI; stdout PCM + stderr status contract; `AVAudioConverter`; `-3805` auto-reconnect; shutdown guard; signals |
| Permissions & signing | [permissions-and-signing.md](permissions-and-signing.md) | Screen Recording (TCC); stable self-signed identity; `build_helper` signing; `-3805` (transient) vs `-3801`/`-3803` (permission) |
| Platform abstraction | [platform-abstraction.md](platform-abstraction.md) | `WindowFinder`/`ScreenGrabber`/`AudioSource` interfaces + `current()` factory; macOS (screencapture/Quartz/helper) and Windows (GDI+/`EnumWindows`/ctypes) backends; macOS+Windows support |
| GUI app | [gui.md](gui.md) | `capture-gui` — native **GPUI (Rust)** daemon client (gui/, crates.io gpui 0.2.2, macOS slice 1): health + window picker + start/stop + live-polled sessions; thin client of `/v1`, no web UI |
| Product architecture | [product-architecture.md](product-architecture.md) | Decision record + plan: daemon-peers architecture (`captured` + GPUI app + MCP + CLI as thin peers); native GPUI GUI (no web UI); packaging/signing/TCC strategy; roadmap M0–M5 (features #25–#35) |

See [`../architecture.md`](../architecture.md) for the cross-cutting module map and hard constraints.

## Spec template (for a new scope)

Every spec uses these `##` sections, in order:

```markdown
# Spec: <Scope name>
_Status: current as of <date>. Source of truth = the code; update this spec in the same change as the code._

## Purpose
## Files
## Public contract
## Behavior
## Invariants & constraints
## Failure modes & handling
## Outputs / artifacts
## Configuration
## Known limitations / open items
## Tests
```

> Each spec's **Known limitations / open items** section is the live backlog for that scope;
> when you close one, remove it there and (if it warrants a tracked task) reflect it in
> `features.json`.
