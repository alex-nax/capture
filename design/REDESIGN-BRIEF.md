# Capture — visual redesign brief (for Claude design)

Upload this folder (`design/`) to Claude to redesign the look of **Capture**. It contains this brief, the
current design tokens (`current-design-system.md`), and screenshots of the live app (`screenshots/`).

## What Capture is
A native desktop app (menu-bar/tray agent + main window) that **records any app or window** — timestamped
screenshots + that app's audio — and turns the audio into **on-device Whisper transcripts** and an
**AI multimodal index** (a navigable, summarized timeline of the recording). For developers / power users
capturing meetings, coding sessions, tutorials, and browser video. Cross-platform: **macOS + Windows**.

## Hard constraints (the redesign MUST honor these)
- **Native GPUI (Rust), NOT a web app.** The UI is built from GPUI primitives: flexbox `div`s, text (a few
  sizes + colors + weight), **SVG icons**, rounded rectangles, solid (and simple gradient) fills, `hover`
  states, opacity, and scroll containers. **No** arbitrary CSS, heavy blurs, or complex shadows — keep
  effects to what GPUI supports (rounded corners, solid/subtle-gradient fills, a light border, maybe a soft
  shadow). Deliverables must be **implementable in GPUI** (give concrete colors/sizes/spacing, not CSS).
- **Dark theme**, fast, lightweight. **Cross-platform** (must look right on macOS and Windows — no
  Mac-only chrome assumptions).
- The **data shapes are fixed** (sessions, windows, transcript segments, the index tree) — redesign the
  presentation, not the information.

## The screens (see screenshots/ + current-design-system.md for the component map)
1. **Dashboard** (default) — Refresh-windows / **Start capture** / Import buttons; a **Mic** selector row;
   a launch-command field; and two columns: **Windows** (the capture-target picker — app → window rows,
   each with a 🎤 mic radio) and **Sessions** (each row: short id · state · duration · segment count, plus
   open-folder / copy / delete actions). Live captures show status here.
2. **Settings** — **Capture quality** (screenshots on/off, format PNG/JPEG, resolution chips, JPEG quality);
   **Index endpoint** (provider selector, host/port/key, a model dropdown, sample-rate + preset chips);
   **Transcription** (a searchable language dropdown, chunk length); an **ASR model manager** (download /
   select Whisper models; on Windows, installable runtime packs); the **App** auto-update row (now with a
   download **progress bar**); a skill installer; permissions.
3. **Playback** (review a finished session) — a timeline/scrubber over the screenshots + the **time-aligned
   transcript**, and the built **index** (root summary + node count) with a Build-index action.
4. **Modals** — a **preset picker** shown on Start (Auto / Meeting / Coding / Lecture / General / Custom,
   each a label + one-line hint), and confirm dialogs (delete / prune / update) — a centered card over a
   dimmed backdrop.
5. **Menu-bar / tray** — a small native status item (separate agent) — secondary, not the focus.

## What's wrong today (the redesign goals)
The app is **functional but utilitarian and dense** — flat near-black panels, low-contrast gray labels,
chips/rows packed tightly, weak visual hierarchy, inconsistent emphasis, and a lot of information competing
for attention (especially the dashboard's two list columns). It reads like a dev tool, not a polished
product. Goals:
- A **refined dark visual system** — a more intentional palette (a cleaner neutral ramp + a confident
  single accent + clear semantic colors), with real **contrast/hierarchy** (headings vs body vs muted).
- **Breathing room + rhythm** — consistent spacing scale, grouped cards/sections with clear separation,
  calmer density on the dashboard.
- **Clear component states** — selected/hover/disabled for chips, buttons, list rows; a tidy selectable
  list-row pattern (Windows + Sessions); a clean dropdown; a polished modal.
- **Stronger primary actions** (Start capture, Build index) and a clear live-capture state.
- **Consistent iconography** + a small, legible type scale.
- Keep it unmistakably **native + fast + dark** — refine, don't web-ify.

## What I'd like back from Claude design
1. A **refreshed dark design system**: a neutral surface ramp (bg / panel / elevated / border), an accent
   (default + hover + subtle/“on” fill), text colors (primary / secondary / muted / on-accent), and
   semantic (success / warning / error / info) — as **hex tokens** with usage notes, plus a small **type
   scale** (sizes + weights) and a **spacing scale**. (Evolve the current tokens in
   `current-design-system.md` — keep the dark identity.)
2. **Component specs**: button (primary/secondary/ghost), chip/toggle (idle/hover/selected), selectable
   **list row**, **dropdown**, **modal/card**, **progress bar**, section header — each with the tokens +
   sizes/radii/padding so it's directly implementable in GPUI.
3. **Mockups of the key screens** redesigned: **Dashboard**, **Settings**, **Playback**, and the **preset
   picker** modal — dark, native, with the new system applied.

Annotate anything that would be hard in GPUI so we can adjust. Thanks!
