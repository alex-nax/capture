# TCC attribution spike (feature #30)

**The question this answers:** when a signed, bundled, **launchd-spawned** daemon
(`CaptureSpike.app` → PyInstaller `captured` → `audiocap --system`) needs macOS
Screen Recording, does TCC attribute the grant to the **daemon/app** (one grant
covers every client — the bet the whole daemon-peers architecture rests on,
`docs/specs/product-architecture.md`), or to something else (terminal, helper)?
And does that grant **survive an app update** re-signed with the same identity?

A NEGATIVE answer here is a success for the spike — it redirects #31/#32 before
they're built.

## What you need

- The spare Mac (macOS 13+; **14 or 15 preferred** — 15.x also answers the
  periodic re-approval question). Clean-ish is best: no prior capture-mcp grants.
- Network on that Mac (the setup downloads `uv` + Python + PyInstaller, ~2 min).
- No Xcode, no Apple Developer account, no admin rights needed
  (`audiocap` ships prebuilt + universal; signing uses a self-signed identity —
  same TCC mechanics as Developer ID, minus Gatekeeper which launchd ignores).

## Run it

On the **dev box** (this repo):

```bash
bash spike/tcc-attribution/make_kit.sh     # -> dist/capture-tcc-spike.tar.gz
```

Copy the tarball to the spare Mac (AirDrop/USB/scp). There:

```bash
tar -xzf capture-tcc-spike.tar.gz && cd capture-tcc-spike
./01_setup.sh         # uv -> Python 3.12 -> PyInstaller -> build `captured`
./02_install.sh       # identity -> CaptureSpike.app -> sign -> launchd agent
./03_check.sh         # THE TEST: grant in System Settings, verify audio flows
./04_update_sim.sh    # update simulation: rebuild + re-sign SAME identity
./04_update_sim.sh --rotate-identity   # optional negative control
./05_collect.sh       # bundle evidence -> ~/Desktop/tcc-spike-results-*.tar.gz
./uninstall.sh        # remove everything afterwards
```

Each step prints what it found; `~/CaptureSpike/status.json` is the live truth
(`"verdict"` field). The daemon runs under launchd, **never under your
terminal** — do NOT grant Screen Recording to Terminal on this machine.

## What to record (the spike report)

1. **Attribution name** — screenshot the Screen Recording pane when the entry
   appears: is it *CaptureSpike* (bundle attribution — ideal), *captured*
   (binary name — Info.plist work needed), or something else? This is finding #1.
2. **Audio flows after grant** — `03_check.sh` verdict / `status_after_grant.json`.
3. **Grant survives same-identity update** — `04_update_sim.sh` verdict /
   `status_after_update.json`. Optional: rotation control loses the grant.
4. **macOS 15 only:** the periodic re-approval dialog — when it appears (days /
   reboots later), its wording, and what it attributes to. Screenshot it. This
   one takes calendar time; leave the agent installed if possible and check back.

Bring `tcc-spike-results-*.tar.gz` back; the findings go into
`docs/specs/product-architecture.md` (the #30 gate) and decide whether #31/#32
proceed on the daemon-peers design or fall back.

## Known deliberate simplifications

- Self-signed identity instead of Developer ID: TCC keys on the designated
  requirement / signing identity either way; Developer ID + notarization is a
  distribution (Gatekeeper) concern for #31, not an attribution one.
- No hardened runtime on the PyInstaller bundle: one variable at a time;
  hardened-runtime entitlements are #31 packaging work.
- The spike daemon is a stub (no /v1 API) — only the TCC-relevant process tree
  is faithful: launchd → signed bundled PyInstaller binary → audiocap child.
