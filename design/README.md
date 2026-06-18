# Capture — design package (upload this folder to Claude)

Everything Claude needs to redesign the look of **Capture** (the native GPUI capture app), for the v3 work.

## What's here
- **`REDESIGN-BRIEF.md`** — start here. What the app is, the hard constraints (native GPUI, dark,
  cross-platform mac+win), the screens, what's wrong today, and exactly what to deliver back (a refreshed
  dark design system + component specs + screen mockups).
- **`current-design-system.md`** — the *actual* tokens + components in the code today (color palette by
  role, type/spacing, the button/chip/dropdown/modal/list-row/progress specs, the screen map, and what GPUI
  can/can't do). Evolve this — keep the dark identity, fix the drift/low-contrast.
- **`screenshots/1-settings.png`** — the live **Settings** screen (the densest one — shows the chips,
  dropdowns, provider config, preset chips, and the current muted dark style).
- **`icons/`** — the current SVG icon set (19 line icons). Keep or refresh; the app renders SVGs tinted by
  a single color.

## Missing screenshots (easy to add)
I could only auto-capture the screen the app was on (Settings). For full coverage, capture and drop in:
- `screenshots/2-dashboard.png` — click **Back** to the dashboard (Start capture / Windows + Sessions lists).
- `screenshots/3-playback.png` — open a finished session (the scrubber + transcript + index).
- `screenshots/4-preset-picker.png` — click **Start capture** to show the preset modal.
(Or just describe them from `current-design-system.md` — the screen/component map covers all of them.)

## The ask in one line
Refresh Capture into a polished, modern **dark** product UI — better hierarchy, spacing, a confident accent,
clean component states — **implementable in GPUI** (flex + text + SVG + rounded fills + hover; no web
effects). Return tokens (hex), component specs, and mockups of Dashboard / Settings / Playback / preset
picker.
