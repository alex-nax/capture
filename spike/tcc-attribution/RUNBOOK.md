# TCC Attribution Spike — Agent Runbook

**You (Claude Code) are running on a spare Mac. Goal: run this spike end-to-end and report the verdict.**
This validates capture-mcp's V2 architecture decision (feature #30): *does a signed, bundled,
**launchd-spawned** daemon become the macOS Screen-Recording (TCC) responsible process — so one
grant covers every client — and does that grant survive an app update?* A negative result is a
**successful** spike (it redirects the design before we build #31/#32).

## The one thing you cannot do yourself
Granting Screen Recording requires a **human clicking a toggle in System Settings**. You cannot
click it. So: run the scripts (they no longer block on `read` when
`CAPTURE_SPIKE_NONINTERACTIVE=1` is set — they poll instead), and when you reach step 3 below,
**pause and tell the human exactly what to click**, then let the script's poll detect the grant.

## Prerequisites (target Mac)
- macOS **13+** (14 or 15 preferred; 15 also answers the periodic re-approval question).
- Network access (the build step downloads `uv` + Python + PyInstaller, ~2–3 min).
- **No Xcode, no Apple Developer account, no admin** — a prebuilt **universal** `audiocap`
  ships in this branch, and signing uses a throwaway self-signed identity.
- Ideally a clean-ish Mac with no prior capture-mcp Screen Recording grants.

## Step 0 — get the kit
Clone this branch (private repo — use the human's GitHub auth: SSH key or `gh auth login`):

```bash
git clone --branch tcc-spike git@github.com:alex-nax/capture.git ~/capture-spike
cd ~/capture-spike/spike/tcc-attribution/kit
export CAPTURE_SPIKE_NONINTERACTIVE=1     # so scripts poll instead of blocking on Enter
```

(If SSH fails, try `gh repo clone alex-nax/capture ~/capture-spike -- --branch tcc-spike`.)

## Step 1 — build (`./01_setup.sh`)
Builds `CaptureSpike.app` (PyInstaller). Uses the **committed prebuilt universal `audiocap`** in
`kit/audiocap` — no compiler needed. Expect it to end with `Built: .../CaptureSpike.app`.

## Step 2 — install + sign + launch (`./02_install.sh`)
Creates the stable signing identity, deep-signs the app (verifies `--deep --strict`), installs a
**launchd user agent**, and starts the daemon. The daemon spawns `audiocap` and writes
`~/CaptureSpike/status.json` every 2 s. Right now it will show a **permission error** — expected,
the grant isn't given yet.

## Step 3 — THE TEST (`./03_check.sh`)  ⟵ pause for the human here
The script opens the Screen Recording settings pane and polls for up to 4 minutes. **Tell the human:**
> 1. In **System Settings → Privacy & Security → Screen Recording**, look for the spike's entry.
>    **Screenshot it and tell me the exact NAME shown** (e.g. *CaptureSpike*, *captured*, *audiocap*,
>    or a terminal). **That name is the attribution answer — finding #1.** Save the screenshot to the Desktop.
> 2. **Enable that entry's toggle.** If macOS offers "Quit & Reopen," accept.
> 3. **Do NOT enable Terminal/iTerm** — the grant must work without any terminal involved.

When the human grants it, `status.json` flips to `"audio_flowing": true` and the script prints the
verdict. Record `status_before_grant.json` / `status_after_grant.json` (saved automatically).

## Step 4 — update persistence (`./04_update_sim.sh`)
Rebuilds the daemon (new binary/cdhash), re-signs with the **same** identity + bundle id, swaps the
bundle, restarts. Expectation: **audio keeps flowing with no new prompt** (grant persists across an
update). Optional negative control: `./04_update_sim.sh --rotate-identity` should LOSE the grant
(proves it's keyed to the signing identity) — only run this if asked.

## Step 5 — collect (`./05_collect.sh`)
Bundles everything (status snapshots, daemon logs, launchctl dumps, today's Desktop screenshots)
into `~/Desktop/tcc-spike-results-*.tar.gz`.

## What to report back
1. **Attribution name** from step 3 (the screenshot + the literal name).
2. **`audio_flowing` verdict** after the grant (step 3).
3. **Did the grant survive the same-identity update?** (step 4 verdict).
4. **macOS 15 only:** if a periodic "still wants to record your screen" prompt appears over the next
   days, note its wording + what it attributes to (leave the agent installed to catch it).
5. The path to `tcc-spike-results-*.tar.gz` — the human carries it back to the dev repo.

## Cleanup (when done)
`./uninstall.sh` removes the launchd agent, the app, `~/CaptureSpike`, the TCC entry, and (on
confirmation) the signing identities. On macOS 15, consider leaving it installed a few days first.

## If something fails
- `01` fails → read `~/CaptureSpike/results/pyinstaller.log`.
- `02` "signature does not verify" → report it; do not proceed.
- `03` never flows audio after the grant → that's a candidate **negative result**: capture the
  Screen Recording screenshot + `status_after_grant.json` and report — this is exactly what the
  spike exists to catch. Run `05` anyway.
- See `README.md` (next to this file) for the design rationale and deliberate simplifications.
