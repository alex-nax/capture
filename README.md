# capture-mcp

An MCP server that captures everything a target process produces, **on demand**:

- **Window video** — timestamped PNG screenshots of the process's window every
  *N* seconds (default 1 s, configurable).
- **Logs** — the process's `stdout`/`stderr`, both raw and merged with per-line
  ISO timestamps.
- **Audio → speech** — the process's audio is captured per-app (macOS
  ScreenCaptureKit) and transcribed by a **pluggable ASR backend**, writing each
  recognized segment with the absolute time it was spoken.

An agent calls `capture_start` to begin saving to a chosen location and
`capture_stop` to end it and stop using disk. `capture_status` reports progress.

> Platform: macOS (tested on 15.x, Apple Silicon). Screenshots and per-app audio
> use macOS-only APIs.

## Tools

| Tool | Purpose |
|------|---------|
| `capture_start` | Begin a capture; returns a `session_id` and summary. |
| `capture_stop`  | Stop a capture (or the only running one) and flush to disk. |
| `capture_status`| List sessions / show one session's counters. |

### `capture_start` targets

Pick **one**:

- `command` — a command line to **launch**. This is the only mode that captures
  `stdout`/`stderr`. Its window and audio are tracked once it appears.
- `pid` — attach to a running process by PID.
- `app_name` — attach by app-name substring (e.g. `"Safari"`).

Key options: `output_dir` (required), `screenshot_interval` (s, default 1.0),
`capture_screenshots`, `capture_audio`, `audio_source` (`auto`|`app`|`mic`),
`audio_chunk_seconds`, `asr_backend` (`auto`|`local`|`nemotron`), `bundle_id`.

## Session output layout

```
<output_dir>/capture-<id>/
├── session.json        # config + live/final summary
├── stdout.log          # raw stdout              (launch mode)
├── stderr.log          # raw stderr              (launch mode)
├── output.log          # merged, ISO-timestamped (launch mode)
├── screenshots/
│   └── 2026-06-07T09-47-01.250Z.png ...
├── audio.s16le         # raw captured audio (16 kHz mono s16le)
├── transcript.jsonl    # {start,end,start_offset,end_offset,text} per segment
└── transcript.txt      # [ISO timestamp] recognized text
```

## Install

```bash
cd /Users/alex/capture
uv venv && source .venv/bin/activate
uv pip install -e .                 # core server
uv pip install -e '.[mlx]'          # + Apple-Silicon Whisper (recommended ASR)
# or: uv pip install -e '.[whisper]'  # cross-platform faster-whisper
# or: uv pip install -e '.[riva]'     # remote Nemotron-3.5 via NVIDIA Riva

# Build the per-app audio helper (needs Xcode / command line tools):
bash scripts/build_helper.sh
```

Grant **Screen Recording** permission to the program that runs the server
(Terminal / your MCP client) under *System Settings ▸ Privacy & Security ▸ Screen
Recording* — required for both screenshots and per-app audio.

## ASR backends

The ASR layer is pluggable (`src/capture_mcp/asr/`):

- **Local (default).** `mlx-whisper` (Apple-Silicon-native) or `faster-whisper`.
  Runs entirely on this Mac; downloads model weights on first use.
- **Nemotron-3.5 ASR (`nemotron`/`riva`).** The 600M NeMo model needs an NVIDIA
  GPU, so it can't run on this Mac directly. The adapter talks to a **Riva**
  server hosting it (self-hosted or NVIDIA-hosted). Configure with env vars:
  `CAPTURE_RIVA_SERVER`, `CAPTURE_RIVA_API_KEY`, `CAPTURE_RIVA_LANG`,
  `CAPTURE_RIVA_MODEL`.

To add a backend, implement `ASRBackend.transcribe(pcm, sample_rate)` in a new
module and register it in `asr/__init__.py:create`.

## Register with an MCP client

Example `claude_desktop_config.json` / `.mcp.json` entry:

```json
{
  "mcpServers": {
    "capture": {
      "command": "/Users/alex/capture/.venv/bin/capture-mcp"
    }
  }
}
```

## How per-app audio works

`helper/audiocap.swift` uses ScreenCaptureKit to capture audio from a single
application (matched by PID or bundle id), converts it to 16 kHz mono PCM, and
streams it on stdout. The Python side buffers it into windows, runs ASR, and
timestamps each segment. If the helper isn't built (or `audio_source="mic"`),
it falls back to capturing the default microphone via `ffmpeg`. The helper also
supports `--system` for whole-display audio.

### Troubleshooting per-app audio (SCStreamError -3805)

If `capture_status` shows `audio_status: app-audio-failed ... code=-3805`
(`failedApplicationConnectionInterrupted`), ScreenCaptureKit enumerated content
fine but `startCapture` was interrupted. This is a **permission** problem, not a
bug:

- Grant **Screen Recording** to the launching process (Terminal / your MCP
  client) and approve the prompt that appears on first capture.
- An **ad-hoc signature changes on every rebuild**, so macOS treats the helper
  as a new binary and drops the grant. Build once with a stable signing identity
  (`CODESIGN_IDENTITY="<cert>" bash scripts/build_helper.sh`) so the approval
  sticks, then approve it once.
- As an immediate workaround, use `audio_source="mic"` (ffmpeg microphone), which
  needs only the Microphone permission.

The capture session degrades gracefully: if the helper can't start, screenshots
and stdout/stderr logging continue and the audio failure is reported in
`audio_status`.

## Caveats

- Attaching by `pid`/`app_name` **cannot** capture pre-existing `stdout`/`stderr`
  (the kernel gives no handle to them); use `command` launch mode for logs.
- Per-app audio requires macOS 13+ and the Screen Recording permission.
- The first transcription downloads model weights (needs network), unless using
  a remote Riva endpoint.
- **ASR runs on fixed offline windows, not true streaming.** Audio is buffered
  into `audio_chunk_seconds` windows and each window is transcribed
  independently, so a word landing on a window boundary can be split across two
  transcript lines. The Nemotron/Riva backend's cache-aware *streaming* mode is
  not yet wired up (the adapter uses `offline_recognize`); switching it to
  `streaming_recognize` is the path to gapless, lower-latency transcription.
- **Speech timestamps are estimates.** The audio timeline is anchored to the
  wall-clock arrival of the first PCM bytes (correcting for capture-startup
  latency), then offsets accrue from sample counts; large silence gaps inserted
  by the source can introduce drift.
