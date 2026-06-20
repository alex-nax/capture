# Capture skill — invocation friction findings

> **Resolved (#78, 2026-06-20).** Both halves fixed in v3: the signed app **bundles** a working
> daemon-first `capture-mcp` at `Capture.app/Contents/Resources/captured/capture-mcp` (verified: it
> answers `initialize` + `tools/list`), and the skill now **discovers before installing** —
> `scripts/discover_mcp.py` resolves the command (app bundle → PATH → build dirs), verifies the MCP
> handshake, and merges `.mcp.json`. **There is no clone+build onboarding** — app users get the bundled
> binary, a dev's checkout exposes `target/release/capture-mcp`, and discovery finds either (the
> `install.sh`/`install.ps1` source-build scripts were removed, 2026-06-20). The GUI also exposes a
> **Copy MCP command** affordance (Settings → Skills). The recommendations below are kept for context.

**Date:** 2026-06-18
**Context:** `/capture` invoked on a Mac that already had **Capture.app 0.2.6** installed, in a
fresh project (`/Users/alex/vibe-techno`, no `.mcp.json`). Goal was simply: register the `capture`
MCP server and start a capture.
**Scope:** This is **only** about the skill-invocation experience — why the skill didn't land me on
a working setup automatically and made me reverse‑engineer one. (Out of scope: the daemon relaunch
later in the session — that was a manual user action, not a defect.)

---

## TL;DR

The skill's first prescribed action to obtain the `capture-mcp` command is **`install.sh` (git clone
+ venv + mlx/Swift build)** — even when the recommended, "fully contained" macOS app is already
installed. There is **no discovery step** for an already-present `capture-mcp`, and the **app bundle
ships no MCP entry point at all**, so "use the app" yields no `command` for `.mcp.json`. Result: the
skill sent me to do an expensive, unnecessary clone+build; the user had to interrupt ("the app package
is fully contained, you need not clone+venv"); and I then had to reverse‑engineer the bundle to learn
it has no MCP server, before finally falling back to a pre-existing `~/capture/.venv/bin/capture-mcp`
that the skill never mentioned looking for.

**Two fixes:** (1) the skill must *discover an existing/working `capture-mcp` before proposing any
install*, and (2) the app should *ship an MCP entry point* so an app‑only user has a `command` without
building anything.

---

## What the skill told me to do (and what I did)

From `SKILL.md`:

- **Step 1** offers two installs. **A** = the macOS app ("recommended … owns the daemon + permission +
  GUI"). **B** = "from source … clones the repo, makes a venv, installs the package."
- The pivotal sentence:
  > "The app gives you the daemon + grant + GUI. To then drive capture from an MCP client (Claude Code
  > etc.), you still register the `capture` server (Step 2) — **its `capture-mcp` command comes from B**,
  > and being daemon-first it attaches to the app's daemon."
- **Step 2:** `configure_mcp.py --bin <CAPTURE_MCP_BIN>`, where `CAPTURE_MCP_BIN` is printed **by
  `install.sh`**.

So the skill's only documented source of the `.mcp.json` `command` is the from‑source build — **even
for users who have the app**. Following it literally, my assessment was: app present, but no
`capture-mcp` on PATH and no `.mcp.json` → therefore run `install.sh` to produce `CAPTURE_MCP_BIN`.
I started exactly that, and the user had to stop me.

---

## Where the invocation broke

### 1. The "fully contained app" does not provide an MCP command — the skill papers over this with path B
The skill positions the app as the recommended, self-contained install, then quietly requires a
**separate from-source build** to get the one thing an MCP client actually needs: the `capture-mcp`
`command`. That is a contradiction a user reasonably objects to. Confirmed by inspecting the bundle —
the only executables are:

| Path | Role |
|---|---|
| `Contents/MacOS/CaptureBar`, `capture-gui` | menu-bar agent + GUI (Rust) |
| `Contents/Resources/captured/captured` | the **daemon** (PyInstaller); `captured <anything>` only ever tries to start the daemon — no `mcp`/`stdio` subcommand |
| `Contents/Resources/captured/audiocap` | per-app audio helper |

There is **no `capture-mcp` binary in the bundle**, and the daemon's HTTP API is **not** MCP-over-HTTP.
So an app-only machine has no path to a `command` except building from source.

### 2. The skill's first action is the most expensive one
`install.sh` does a network `git clone`, creates a venv, installs `mlx`/whisper, and builds+signs a
Swift helper. The skill frames this as Step 1. It should be the **last resort**, only after cheaper
discovery fails — not the default opening move.

### 3. No discovery of an existing `capture-mcp`
A working entry point already existed at **`~/capture/.venv/bin/capture-mcp`** (and the config the app
itself writes, `~/.capture/config.env`, even points at `CAPTURE_REPO=$HOME/capture`). The skill never
says "check PATH / known locations / the app bundle for an existing `capture-mcp` first." A one-line
probe would have found it instantly and skipped the entire detour. Instead I had to:
- inspect the app bundle,
- probe `captured` for a hidden MCP subcommand (there isn't one),
- test argv[0] basename dispatch (no),
- check the daemon HTTP API for MCP-over-HTTP (no),
- and only then find and verify `~/capture/.venv/bin/capture-mcp`.

### 4. No "does this command actually speak MCP?" verification
Nothing in the skill verifies a chosen `command` before writing it into `.mcp.json`. I had to
hand-write a JSON-RPC `initialize` + `tools/list` handshake to confirm the binary worked (and that it
attaches to the running daemon rather than erroring). A bundled probe would make this safe and obvious.

---

## Recommendations

### A. Skill edits (immediate, low cost)

1. **Add a discovery step before any install.** Resolve the `capture-mcp` `command` in this order, and
   use the first that answers an MCP `initialize`:
   1. **App-bundled MCP entry** — once the app ships one (see B1), e.g.
      `/Applications/Capture.app/Contents/Resources/captured/captured mcp`.
   2. **`command -v capture-mcp`** on PATH.
   3. **Known install locations:** `~/capture/.venv/bin/capture-mcp`,
      `~/.capture-mcp/.venv/bin/capture-mcp`. (Also read `~/.capture/config.env` →
      `$CAPTURE_REPO/.venv/bin/capture-mcp`.)
   4. **Only if all fail:** offer `install.sh`, stating up front it is a network clone + build, and ask
      before running.
2. **Rewrite the misleading sentence.** Drop "the `capture-mcp` command comes from B." Document exactly
   where the MCP `command` lives for each setup (app vs. source) so the agent never has to
   reverse-engineer the bundle.
3. **Demote `install.sh`** from "Step 1" to an explicit last-resort fallback.
4. **Ship a tiny MCP handshake probe** (`initialize` + `tools/list`) and have the skill run it to
   verify the chosen `command` before editing `.mcp.json`.
5. **State the app-only reality** until B1 lands: an app-only user currently has *no* `command`, so the
   skill must either require the app to expose one or fall back to source after discovery fails.

### B. App changes (the root-cause fix the skill depends on)

1. **Ship an MCP stdio entry point in the bundle** so app-only users get a `command` with no build.
   Preferred: add a **`captured mcp` / `captured stdio`** subcommand to the existing daemon binary that
   runs the FastMCP stdio server, daemon-first (bridge to a running daemon, else start one). The binary
   already bundles `click`; today `captured <anything>` ignores args and only starts/refuses the daemon.
   Alternative: freeze a second PyInstaller EXE `capture-mcp` next to `captured`
   (`…/Resources/captured/capture-mcp`).
2. **Let the app write/merge `.mcp.json`.** It already installs this skill into `~/.claude/skills`,
   detects "Claude Code"/"Codex", and manages permissions — MCP registration is the missing half of
   that integration. At minimum, add a GUI "Copy MCP config / command path."
3. **`configure_mcp.py`:** support a `command + args` form (to register `captured mcp`) and an
   `--app`/auto-detect mode that fills in the bundled command path.

---

## One-line takeaway for the changelog

> The skill assumes the MCP `command` only comes from a source build, so on an app-only machine it
> defaults to an expensive clone+build, never checks for an existing `capture-mcp`, and the app ships
> no MCP entry point — fix by (1) discovering an existing/bundled command before installing and (2)
> shipping `captured mcp` in the app.
