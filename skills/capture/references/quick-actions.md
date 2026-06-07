# capture-mcp quick actions

Recipes to run AFTER `capture` is registered in `.mcp.json` and the `capture_start` /
`capture_stop` / `capture_status` MCP tools are available in the client.

## Tool cheat-sheet (`capture_start`)
Target — pass exactly ONE: `command` (launch; also captures stdout/stderr), `pid`, or `app_name`.
Common options: `output_dir` (required), `screenshot_interval` (s, default 1.0),
`screenshot_format` (png/jpg/...), `screenshot_resolution` ("1280x720" or "1280x720/jpg",
aspect-preserved), `screenshot_jpeg_quality`, `capture_screenshots`, `capture_audio`,
`audio_source` (`auto`|`app`|`mic`), `audio_chunk_seconds`, `asr_backend` (`auto`|`local`|`nemotron`),
`bundle_id`, `cwd`. Returns a `session_id`. Stop with `capture_stop(session_id)`.

Outputs land in `<output_dir>/capture-<id>/`: `screenshots/<iso>.png|jpg`, `stdout.log`/
`stderr.log`/`output.log` (launch mode), `audio.s16le`, `transcript.jsonl` + `transcript.txt`,
`session.json`.

## 1. Capture a browser video
macOS (Chrome shown; adapt the app name):
1. Open the page so it's the active tab: `open -a "Google Chrome" "<URL>"` and give it a few seconds.
2. `capture_start(output_dir="<dir>", app_name="Google Chrome", audio_source="app",
   screenshot_interval=2.0, screenshot_resolution="1280x720/jpg")`.
   (Targeting works even if the player goes fullscreen onto another Space, and the user can keep
   working in other windows — capture runs in the background.)
3. Let it run for the clip; `capture_stop(session_id)`.
4. Read `transcript.txt` for narration and sample `screenshots/` for key frames.
> The user must be able to multitask during capture — never require them to keep the window focused.

## 2. Launch & capture a process or app
- A command you start (gets stdout/stderr too):
  `capture_start(output_dir="<dir>", command="<cmd ...>", cwd="<dir>")`.
- A GUI app you start: `open -a "<App>"`, then attach by `app_name="<App>"` or `pid`.

## 3. Attach to an already-running app
`capture_start(output_dir="<dir>", pid=<PID>)` or `app_name="<substring>"`.
(Attach mode cannot capture pre-existing stdout/stderr — use `command` launch mode for logs.)

## 4. Change the default ASR model (and download it)
```bash
python scripts/set_model.py --model mlx-community/whisper-large-v3-turbo \
       --prefetch --python "<CAPTURE_MCP_PY>"
```
Or per-capture: pass `asr_backend="local"` (default) / `"nemotron"` (needs a Riva endpoint via
`CAPTURE_RIVA_*` env). Valid models: `mlx-community/whisper-tiny`,
`mlx-community/whisper-large-v3-turbo`. **`mlx-community/whisper-base` is NOT a real repo.**

## 5. Per-project / per-capture config
- **Server-level env** (applies to every capture in the project): edit the `capture` entry's
  `env` in `.mcp.json` (e.g. `CAPTURE_WHISPER_MODEL`, `CAPTURE_RIVA_SERVER`,
  `CAPTURE_RIVA_API_KEY`, `CAPTURE_RIVA_FUNCTION_ID`, `CAPTURE_RIVA_LANG`) — or re-run
  `configure_mcp.py --model ...`. Reload MCP after editing.
- **Per-capture**: pass parameters to `capture_start` (interval, format/resolution, quality,
  `capture_audio`/`capture_screenshots`, `audio_source`, `audio_chunk_seconds`).

## 6. Status / stop
- `capture_status()` lists sessions; `capture_status(session_id)` shows one (counts, `audio_status`).
- `capture_stop(session_id)` stops one; with one session running, `capture_stop()` stops it.

## Troubleshooting
- **`audio_status: app-audio-failed ... -3805`** — `failedApplicationConnectionInterrupted`, a
  *transient* connection blip (NOT a permission denial, which is `-3801`). The helper auto-reconnects;
  if it persists, ensure audio is actually playing and that Screen Recording is granted. Run
  `bash "<CAPTURE_HOME>/scripts/setup_codesign.sh"` once so the grant persists across rebuilds.
- **No transcript / `asr-unavailable`** — install an ASR backend (the installer does this) and use a
  valid model name. First use downloads weights (prefetch to avoid the stall).
- **Empty/black screenshots** — grant Screen Recording to the app that launches the MCP server.
- **Windows/Linux** — per-app audio + screenshots are macOS-only today; see the project's
  `docs/specs/platform-abstraction.md`.

### Still stuck? Report it upstream
If you can't resolve it, file a tracked bug (preview first, then post with consent):
```bash
python scripts/report_issue.py --summary "<what went wrong>" --session-dir "<output_dir>"   # preview
python scripts/report_issue.py --summary "<...>" --session-dir "<output_dir>" --create        # post via gh / URL
```
It auto-collects safe diagnostics (version, OS, the session's `audio_status`/errors) and omits
secrets. Posting publishes to `github.com/alex-nax/capture` — confirm with the user first.
