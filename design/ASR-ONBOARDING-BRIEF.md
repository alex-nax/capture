# Capture — ASR onboarding & runtime-download brief (for Claude design)

A focused companion to `REDESIGN-BRIEF.md`. This one is about a single new flow: **getting
speech-to-text working on a fresh install**, now that the transcription **runtime** and the
**model** are downloaded separately (they're no longer bundled in the app). Match the existing
visual language in `current-design-system.md` — this should feel like the same app, not a new one.

## Context (what's changed)

Capture records a process's screen + audio and transcribes the audio on-device. As of v3 the app
ships **with no ASR engine and no model** — both are downloaded after install:

1. A **runtime** = the speech engine for the user's hardware (e.g. *Whisper (Metal GPU)* on Apple
   Silicon, *MLX* on Apple Silicon, *Whisper (CPU)* elsewhere, or *Remote* = a server endpoint).
   It's a small download (a few MB) hosted as a GitHub release, auto-updating on its own.
2. A **model** = the weights the runtime runs (e.g. `base.en` ~150 MB … `large-v3-turbo` ~1.6 GB).
   The user picks one compatible with the chosen runtime and downloads it.

**Nothing is auto-selected.** Until the user has **both** a runtime and a model, transcription is
off — captures still record audio + screenshots, they just won't produce a transcript. So the app
needs to *invite* the user to set this up, clearly and once, without nagging.

## Hard constraints (must honor)

- **Native GPUI (Rust), no web UI / webview.** These are GPUI views; design within the existing
  component kit (chips, buttons, cards, the progress bar, the modal) in `current-design-system.md`.
- **Match the tokens** in `current-design-system.md`: near-black surfaces (`#141414`/`#1e1e1e`),
  accent blue `#2d4f67` (selected/primary), accent text `#8ab4f8`, progress fill `#4a90d9`, muted
  text `#9aa0a6`, success `#66d9a0`, warning `#ffcc66`. Don't introduce a new palette.
- **The daemon `/v1` API is the only backend.** Every action below maps to a real route (listed per
  state). Don't invent capabilities the daemon doesn't have.
- **Non-blocking.** The dashboard must still be usable (you can start a screenshots-only / audio-only
  capture) while ASR is unconfigured. The CTA invites; it never traps.
- **No silent fallback.** Never imply transcription is happening when no runtime+model are active.

## The states to design

### 1. Dashboard call-to-action — "no runtime AND no model" (the hero of this brief)
The primary new surface. On the **dashboard** (`screens/dashboard.rs`), when neither a runtime pack
nor a model is installed, show a **prominent but dismissible card** (think: a single, calm hero card,
not a wall of options) that:
- States plainly that transcription is off and one quick setup turns it on.
- Has a **primary action** ("Set up transcription" / "Enable voice") that opens the runtime+model
  flow (states 2–3), and a quiet secondary ("Not now" / capture works without it).
- Reassures that captures still record audio + screenshots meanwhile.
Design both **placements**: (a) as a banner/card at the top of the dashboard, and (b) the
*minimised* variant once dismissed (a small inline hint near the Start button), so it's recoverable.

### 2. Runtime picker + download
A card/section listing the **available runtimes for this machine**. Each row/chip shows:
- **Label** (e.g. "Whisper (Metal GPU)"), a one-line **requirement** ("Apple Silicon — runs on the
  Metal GPU"), and an approximate **download size**.
- **State**: not-installed → a **Download** button; downloading → an **inline progress bar**
  (`#4a90d9`) with %; installed → a check + "Installed"; the **active** one is visually selected
  (accent `#2d4f67`), like the existing chips.
- An **unavailable** treatment for runtimes this hardware can't run (e.g. CUDA on a Mac): dimmed +
  a short reason, not a dead button.
Data: `GET /v1/asr/runtimes` → `{active, gpu, runtimes:[{id,label,kind,requires,installed,available,active}]}`.
Actions: `POST /v1/asr/runtimes/install {id}` (downloads the pack; SSE `asr_runtime_install` progress),
`POST /v1/asr/runtime {id}` (make active).

### 3. Model picker + download (shown for a LOCAL runtime)
Once a local runtime is active, the **model catalog** for it. Each row: **name** (`base.en`,
`large-v3-turbo`…), a **size label**, and state (Download / progress / Downloaded / **Active**).
A short note that bigger = more accurate + slower + larger download. For the **Remote** runtime, show
the remote-endpoint config instead (no local model list) — the model list must always match the
selected runtime.
Data: `GET /v1/asr/models` → `{backend_available, active, models:[{repo,name,size_label,downloaded,active,downloading}]}`.
Actions: `POST /v1/asr/models/download {repo}` (SSE progress), `POST /v1/asr/model {repo}` (activate),
`POST /v1/asr/models/delete {repo}`.

### 4. Progress
Downloads can be large (a model is up to ~1.6 GB). Design the **in-flight** state: a labeled progress
bar with %, a sense of size/ETA if available, and a calm "this happens once" tone. A pack download is
quick; a model download is the long one — make the model-download progress feel intentional, not stuck.

### 5. Ready / done
When a runtime + model are both active, the CTA **resolves**: the hero card is replaced by a compact,
positive confirmation (success `#66d9a0`) that transcription is on, naming the active runtime + model,
with a quiet way to change them (→ Settings → Voice). New captures now transcribe.

### 6. Partial states (cover these — they're real)
- **Runtime installed, no model** → guide straight to the model picker ("Almost there — pick a model").
- **Model downloaded, no runtime** → guide to the runtime picker.
- **Download failed / offline** → a recoverable error (retry), warning-toned, never silent.
- **A newer runtime pack is available** (auto-update) → a subtle "Update available" affordance on that
  runtime row (mirrors the app's update chip), not a blocking modal.

## Where these live
- State 1 (CTA) → the **dashboard** (`screens/dashboard.rs`).
- States 2–3 (pickers) → **Settings → Voice** (`screens/settings.rs`) is the existing home for the
  runtime + model UI; the CTA's primary action can deep-link there, or open a focused first-run
  sheet/modal. Design **both** options (inline-in-settings vs a dedicated first-run sheet) and
  recommend one.

## What I'd like back from Claude design
- High-fidelity mocks of states 1–6 in the **Capture visual language** (match `current-design-system.md`
  + the unpacked template referenced in `design/README.md`), dark theme.
- The **dashboard CTA** in both full and minimised placements — this is the most important screen.
- A short rationale for the chosen pattern (inline-in-settings vs first-run sheet) and how the CTA
  recovers after "Not now".
- Component-level notes (reuse the existing chip / button / progress-bar / card so it ports to GPUI
  cleanly) — call out any *new* component you introduce and why.

Keep it calm, one-time, and honest: the goal is a user who lands on the dashboard, understands in one
glance that one quick download turns transcription on, does it, and never thinks about it again.
