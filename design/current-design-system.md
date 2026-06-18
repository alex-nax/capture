# Capture — current design system (extracted from the GPUI code)

The actual tokens + components in `gui/src/app.rs` today, so the redesign evolves a real system. All colors
are `0xRRGGBB` (GPUI `rgb()`); the backdrop is `rgba(0x000000cc)`.

## Color palette (in-use, by role)
| Role | Hex | Notes |
|---|---|---|
| App background | `#141414` / `#16181c` / `#0e1216` | near-black; a few slightly different blacks are used inconsistently |
| Panel / surface | `#1e1e1e` / `#1c1c1c` (modal card) / `#23262b` | |
| Chip / control idle bg | `#2a2a2a` | also `#3a3a3a` (progress track) |
| **Accent (blue)** | **`#2d4f67`** | selected chip + primary button fill; `#3d6a87` hover-ish; `#1d2733` subtle |
| Accent text / link | `#8ab4f8` | light blue (e.g. the "downloading update…" text) |
| Progress fill | `#4a90d9` | the new update progress bar |
| Text — primary | `#e0e0e0` / `#f2f2f2` / `#e6e6e6` | |
| Text — secondary/muted | `#9aa0a6` (most common) / `#c8ccd0` / `#aab0b8` | low-contrast gray labels everywhere |
| Text — dim/disabled | `#6a6a6a` / `#666b6f` | |
| Success | `#66d9a0` (+ `#c8e6c8`) | |
| Warning / message | `#ffcc66` / `#e0c063` | the status message line |
| Error | `#e6a0a0` (text) / `#7a2d2d` (destructive btn bg) / `#e6c0c0` | delete/prune |
| Modal backdrop | `rgba(0x000000cc)` | dim overlay behind centered cards |

Observation: the neutrals drift (several near-blacks + several grays), the accent is a *desaturated* blue
(reads muted/flat), and contrast between muted text (`#9aa0a6`) and dark panels is low.

## Typography
GPUI text sizes used: `text_xl` (the "capture" title), `text_lg` (modal titles), `text_sm`, `text_xs`;
default body is small. No explicit weight ladder. ~80 `text_color` overrides carry the hierarchy (color, not
size/weight) — a redesign should introduce a clear **size + weight scale**.

## Spacing / shape
Tailwind-like helpers: padding `px_2/px_3/p_4`, gaps `gap_1/gap_2/gap_3`, `rounded_md`/`rounded_lg`/
`rounded_sm`, fixed widths via `min_w(px(..))` / `w(px(..))` (e.g. label columns `min_w(px(96))`, modal cards
`w(px(340/400))`, progress track `px(160)`). No consistent spacing scale.

## Component kit (current implementations)
- **button** — `px_3 py_1 rounded_md`, bg `#2d4f67`, pointer cursor, text label. (One style; no
  primary/secondary/ghost distinction. Destructive variant uses bg `#7a2d2d` + a trash icon.)
- **chip** (toggle) — `px_2 py_1 rounded_md`; selected: bg `#2d4f67` text `#e0e0e0`; idle: bg `#2a2a2a` text
  `#9aa0a6`. Used for resolution / quality / preset / sample-rate / format toggles. No hover style on most.
- **icon** — inline **SVG** from `icons/<name>.svg`, sized + tinted (e.g. `settings`, `chevron-left`,
  `list-tree`, `trash`, `folder`, `copy`). A redesign can supply a consistent icon set.
- **searchable dropdown** (`language_field`) — a clickable field that expands a filtered list of rows
  (used for the transcription language; the model dropdown is similar). Idle field + expanded list of
  selectable rows with hover tint.
- **selectable list row** — Windows (app group header + a 🎤 mic radio + per-window checkbox rows) and
  Sessions (id · state · duration · segments + folder/copy/delete icon actions). Dense, low chrome.
- **modal/card** — centered `div().absolute().top_0().left_0().size_full().flex().items_center()
  .justify_center().bg(rgba(0x000000cc))` with a `w(px(340..400)) p_4 rounded_lg bg(#1c1c1c)` card
  (title `text_lg` + body `#9aa0a6` + buttons). Used for confirm dialogs + the preset picker.
- **progress bar** (new) — a `px(160)` track `#3a3a3a rounded_sm` with a filled child `#4a90d9` sized to the
  download fraction + a `NN% (DD/TT MB)` label.
- **header** — `text_xl` "capture" + a `daemon v… (api …) · pid …` status line + a hotkey hint line.
- **scrollbar** — a custom overlay thumb on the scroll content.

## Screen structure (toggled by `show_settings` / `playback`)
- **Dashboard** (`dash`): header → Refresh/Start/Import buttons → Mic row → launch field → Import → two
  columns (Windows picker | Sessions list).
- **Settings** (`sett`): capture-quality panel → index-endpoint config → transcription → ASR model manager
  → App update row → skill installer → permissions.
- **Playback**: a session's scrubber + transcript + the built index summary.
- **Overlays**: preset picker + confirm modals (centered cards).

## GPUI implementation notes (what the redesign can rely on)
Available: flex layout (row/col, gap, align/justify, wrap), fixed/min/max sizes in `px`, `rounded_*`,
solid `bg`, **simple gradients**, `border`, `opacity`, `text_color` + the size helpers, `hover(|s| …)`
state styling, `cursor_pointer`, **SVG icons**, scroll containers (`overflow_y_scroll` + a `ScrollHandle`),
absolute overlays. Avoid: arbitrary CSS, heavy multi-layer shadows, blur/backdrop-filter, animations beyond
simple state changes. So: prefer a refined **flat/soft** dark style (clean surfaces, a clear border/elevation
ramp, one confident accent, good type+spacing rhythm) over glassmorphism or heavy effects.
