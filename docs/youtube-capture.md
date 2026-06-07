# Browser capture workflow (YouTube playlist → transcripts)

Application tooling (in `scripts/`) that dogfoods capture-mcp to capture a YouTube playlist
playing in a real browser and produce per-video transcripts + summaries. Windows-oriented (uses
the interactive desktop + WASAPI loopback), but the pieces are OS-neutral above the platform layer.

## Pieces
- **`scripts/run_interactive.ps1`** — runs a command in the logged-on user's interactive desktop
  session (`WinSta0`) via a transient Interactive-logon scheduled task. Needed because a service /
  SSH / agent shell runs in a non-interactive window station where there are no user windows and no
  real audio. `-NoWait` starts it fire-and-forget (for long-running jobs like the capture or the
  debug browser); otherwise it waits and unregisters the task.
- **`scripts/capture_youtube_playlist.py`** — the driver. Enumerates the playlist with `yt-dlp`,
  **attaches** (Selenium) to a Chrome started with `--remote-debugging-port` (attaching to a
  normally-launched, signed-in Chrome avoids the automation throttle that cut a fresh automated
  Chrome off after ~42 s), plays each video, **mutes/skips ads** (so ad audio is silence in the
  transcript), and runs ONE continuous `CaptureSession` (screenshots + WASAPI loopback → ASR). Key
  flags: `--attach host:port`, `--target-pid <chrome-browser-pid>` (window-targeted, occlusion-proof
  screenshots), `--model large-v3`, `--out <dir>`, `--screenshot-interval`.
- **`scripts/transcribe_audio.py`** — offline (re)transcribe a saved `audio.s16le` with
  faster-whisper. Authoritative: live capture timestamps can drift if the loopback lags wall-clock,
  so the offline pass gives a clean, gap-free transcript indexed by audio offset.
- **`scripts/playlist_deliverables.py`** — split the continuous transcript into per-video
  `transcript.txt` (+ screenshots) by per-video windows from `manifest.json`/`status.json`.

## Run it (Windows)
```powershell
# 1. Start a debug-enabled Chrome on a dedicated profile in the interactive session.
#    (Chrome 148 blocks remote-debugging on the default profile, so use a separate --user-data-dir.)
./scripts/run_interactive.ps1 -NoWait -TaskName capmcp_chrome -Exe "C:\Program Files\Google\Chrome\Application\chrome.exe" `
  -Arguments "--remote-debugging-port=9222 --user-data-dir=C:\...\yt_profile --autoplay-policy=no-user-gesture-required --disable-gpu --disable-background-timer-throttling https://www.youtube.com"
# (sign in once in that window if you want the non-throttled signed-in session)

# 2. Find the Chrome *browser* pid (the chrome.exe with --remote-debugging-port and no --type=).
# 3. Run the capture in the interactive session (pythonw = no console window stealing focus):
./scripts/run_interactive.ps1 -NoWait -TaskName capmcp_playlist -Exe "C:\...\.venv\Scripts\pythonw.exe" `
  -Arguments "scripts\capture_youtube_playlist.py --playlist <url> --attach 127.0.0.1:9222 --target-pid <pid> --model large-v3 --out C:\...\run"

# 4. When done: deliverables + (if needed) an authoritative offline re-transcribe.
python scripts/playlist_deliverables.py --run C:\...\run
python scripts/transcribe_audio.py C:\...\run\capture-*\audio.s16le --model large-v3
```

## Gotchas (learned the hard way)
- **Throttling:** a *fresh Selenium-launched* Chrome gets cut off by YouTube after ~42 s. Attaching
  to a *normally-launched* Chrome (`debuggerAddress`) avoids it. No sign-in strictly required, but a
  signed-in profile is safest.
- **Interactive session required:** screenshots and loopback audio are session-isolated — capture
  must run in `WinSta0` (hence `run_interactive.ps1`), not the agent/service shell.
- **No console windows:** run the driver with `pythonw` and launch the audio helper with
  `CREATE_NO_WINDOW`, so nothing steals the foreground / pollutes whole-screen captures. Target the
  Chrome window (`--target-pid`) for occlusion-proof screenshots regardless.
- **System audio, not per-app:** the loopback captures the whole output mix — mute other audio for a
  clean transcript. Ads are muted by the driver (silence) so they don't reach the transcript.
- **Audio can lag wall-clock** over long runs → prefer `transcribe_audio.py` (offline) for the final
  transcript, and split per video by content rather than wall-time.
- **Non-narrated videos** (music/montage/demo) yield no transcript by design — verify against the
  source audio (`yt-dlp -f bestaudio` + transcribe) before assuming a capture bug.
