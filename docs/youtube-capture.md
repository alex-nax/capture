# Browser capture workflow (YouTube playlist → transcripts)

Application tooling (in `scripts/`) that dogfoods capture-mcp to capture a YouTube playlist
playing in a real browser and produce per-video transcripts + summaries. Windows-oriented (uses
the interactive desktop + WASAPI loopback), but the pieces are OS-neutral above the platform layer.

## Spawning the browser correctly (Windows) — the key insight

The single most important lesson: **do NOT let Selenium launch Chrome.** A Selenium-launched
Chrome carries automation markers and YouTube **throttles its video stream — playback freezes
~42 s into every video** (reproduced with zero capture running; not a capture bug, and not fixable
with flags). Instead, launch a normal Chrome yourself and **attach** to it over the DevTools port:

1. **Launch Chrome normally** with a remote-debug port, in the **interactive desktop** session
   (screenshots + WASAPI loopback are session-isolated — a service/SSH/agent shell is a dead end):
   - Use `scripts/run_interactive.ps1 -NoWait` so it runs in `WinSta0` and stays up.
   - **`--remote-debugging-port=9222`** + a **dedicated `--user-data-dir`** — Chrome 148+ **refuses
     remote debugging on the default profile**, so you must use a separate profile dir. (No sign-in
     is strictly required; attaching to a normal Chrome is what defeats the throttle. A signed-in
     profile is the safest belt-and-suspenders.)
   - **`--disable-gpu`** so `PrintWindow` captures the video (GPU/overlay frames come out black
     otherwise); **`--autoplay-policy=no-user-gesture-required`** for autoplay-with-sound;
     `--disable-background-timer-throttling --disable-backgrounding-occluded-windows
     --disable-renderer-backgrounding` for good measure.
2. **Find the browser pid** = the `chrome.exe` whose command line contains `--remote-debugging-port`
   **and not** `--type=` (child renderer/gpu processes have `--type=`). Pass it as `--target-pid` so
   screenshots target that window via `PrintWindow` (occlusion-proof — you can stack other windows
   on top and keep working).
3. **Attach, don't launch:** the driver connects with Selenium's `debuggerAddress=127.0.0.1:9222`,
   opens its own tab, and drives playback there (leaving your other tabs alone).
4. **No console windows stealing focus:** run the driver itself with **`pythonw.exe`** (no console),
   and the audio helper subprocess is spawned with **`CREATE_NO_WINDOW`**. A stray console window
   becomes the foreground window and ruins whole-screen capture; window-targeting + these flags
   avoid it.

Everything below builds on this.

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
