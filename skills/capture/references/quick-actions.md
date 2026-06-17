# capture-mcp quick actions

Recipes to run AFTER `capture` is registered in `.mcp.json` and the `capture_start` /
`capture_stop` / `capture_status` MCP tools are available in the client.

## Tool cheat-sheet (`capture_start`)
Target — pass exactly ONE: `command` (launch; also captures stdout/stderr), `pid`, or `app_name`.
Common options: `output_dir` (required), `window_id` (pin screenshots to one window — from `list_windows`),
`screenshot_interval` (s, default 1.0), `screenshot_format` (png/jpg/...), `screenshot_resolution`
("1280x720" or "1280x720/jpg", aspect-preserved), `screenshot_jpeg_quality`, `capture_screenshots`
(false = audio-only), `capture_audio`, `audio_source` (`auto`|`app`|`mic`), `mic_device` (also record this
input device as a separate mic track), `audio_chunk_seconds`, `asr_backend` (`auto`|`local`|`nemotron`),
`bundle_id`, `cwd`. Returns a `session_id`. Stop with `capture_stop(session_id)`. Full tool set: §7.

`preset` (#54) records the capture intent + the index preset a later `capture_index` defaults to:
`meeting` (mic on; participants/active-speaker/task-assignments), `coding` (verbatim code at high-res),
`lecture` (slides/explainer), `auto` (classify per frame), `general`, or `custom`. **As a frontier model
you can pass `preset="custom"`** to mark the session for your own tailored indexing, then call
`capture_index(session_id, leaf_prompt=…, leaf_schema=…)` with a prompt/schema you craft for that content
(the cheap local model executes it; raise `max_px` for small-font code). Good prompts are saved to
`<session>/index_prompts.json` so they can be folded into the defaults.

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
  Summaries carry **capability flags** `has_screenshots`/`has_audio`/`has_mic`/`can_retranscribe`/`can_index`
  and the active `mic_device`.
- `capture_stop(session_id)` stops one; with one session running, `capture_stop()` stops it.

## 7. The full tool set (beyond start/stop/status)
- `list_windows(app_name?, pid?)` — the picker: `window_id`/`pid`/`app_name`/`title`/size, largest first.
  Pass a `window_id` to `capture_start` to pin screenshots to ONE window (two Chrome windows share a pid).
- `list_audio_devices()` — input devices `{id, name, default}` for `mic_device`.
- `capture_set_mic(session_id, device)` — **switch the mic on a LIVE capture** (no restart): a device id /
  `"default"` = on/switch, `null`/`""` = off. The mic is a separate track (`mic.s16le`/`mic_transcript.*`).
- `capture_prune(session_id, parts)` — free disk on a finished capture: `parts` ⊆ `screenshots` (delete),
  `screenshots_halve`, `audio` (removing audio disables re-transcribe). Returns freed bytes + refreshed flags.
- `capture_retranscribe(session_id, asr_backend?, model?, language?, chunk_seconds?)` — re-run ASR over the
  saved audio (needs `can_retranscribe`). **Fixes a wrong-language or hallucinated transcript** — pass
  `language="ru"` (etc.) and/or a larger `chunk_seconds`. Background; watch `capture_status`.
- `transcription_settings(language?, chunk_seconds?)` — get/set the persisted transcription settings shared by
  all captures. **`language`** (ISO code; `""`/`auto` = auto-detect) stops Whisper hallucinating "Thank you."
  on short non-English chunks — and applies **on the fly** to a running capture. **`chunk_seconds`** (1–120,
  default 30) is the window length; ≥24 s avoids short-chunk hallucination. No args = read current.
- `capture_import(path, output_dir?, asr_backend?)` — turn an existing audio/video file into a session
  (extracts audio + frames, runs ASR). Audio-only files → audio-only sessions; silent video → frames only.
- `capture_index(session_id, endpoint?, model?, sample_rate?)` — build a hierarchical multimodal index of a
  session's screenshots with a remote vision LLM (LM Studio): leaf captions → a whole-session root summary
  (`GET /v1/sessions/{id}/index`). Needs `can_index` + a configured, reachable endpoint (`CAPTURE_INDEX_URL`).

## 8. Fix a wrong / hallucinated transcript
**Symptom:** the transcript is the same phrase over and over ("Thank you.", "Thanks for watching",
"Обригаду", "Gracias"), is in the wrong language, or is empty/garbled even though you could hear speech.

**Why:** these are Whisper *hallucinations*, not lost audio — the audio is usually fine. Whisper is trained
on 30-second windows, so it confabulates a stock phrase when a chunk is **too short** (old captures used 8 s),
**mostly silence** (a muted/distant mic loops junk), or **a non-English language it mis-detected as English**.
Confirm the audio is real first: `capture_status(session_id)` should show `audio_status: running`/`stopped`
(not `*-audio-failed`) and a non-trivial `transcript_segments` count — if so, it's a recovery job, not a
re-capture.

**Fix — set the language + a longer chunk, then re-transcribe (the saved audio is reused, nothing re-recorded):**
1. `transcription_settings(language="ru", chunk_seconds=30)` — pin the spoken language (ISO code: `ru`, `en`,
   `de`, …; `"auto"` to clear) and use a 30 s window. This persists for new captures too, and on a *running*
   capture it takes effect on the next chunk (so you can correct a live transcript without restarting).
2. `capture_retranscribe(session_id, language="ru", chunk_seconds=30)` — re-runs ASR over the stored
   `audio.s16le`, replacing `transcript.jsonl`/`.txt` (the old one is kept as `transcript.prev.*`). Runs in
   the background; watch `capture_status` `transcript_segments`. Needs `can_retranscribe` (audio still present
   — `capture_prune ... "audio"` removes it).
3. Re-read `transcript.txt`. Still off? Try a stronger model — `capture_retranscribe(session_id,
   model="mlx-community/whisper-large-v3-turbo", language=…)`.

A silent mic that only ever produced "Thank you." was genuinely empty — gate/ignore it (newer builds default
to 30 s chunks + a silence gate, so fresh captures rarely need this).

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
