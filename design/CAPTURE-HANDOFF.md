# Capture — Redesign Handoff for Claude Code

Implementation brief for the **Capture** v3 visual redesign. Pair this document with the
HTML export (`Capture Screens (standalone).html`) — the HTML is the **visual source of truth**;
this file explains the system, the decisions behind it, and how to land it in the real app.

> **Stack:** native **GPUI (Rust)**, not web. Everything here is expressed in GPUI primitives:
> flex layout, text (size/weight/color), SVG icons, rounded solid / simple-gradient fills, 1px
> borders, hover states, opacity, scroll. **No** glassmorphism, blur/backdrop-filter, multi-layer
> shadows, or animation beyond simple state changes. The HTML uses a few web conveniences purely
> for preview — they are called out under "GPUI translation notes" and must NOT be ported literally.

---

## 1. Goal

Turn Capture from a dense, utilitarian dev tool into a polished, modern **dark** product:
stronger hierarchy, breathing room, a confident accent, and clean component states — without
changing the information architecture or the fixed data shapes (sessions, windows, transcript
segments, the index tree).

---

## 2. Decisions locked in (the calls you made)

| Decision | Choice | Rationale |
|---|---|---|
| **Accent** | **Indigo `#6366f1`** | Saturated, modern; a deliberate upgrade from the flat `#2d4f67`. Chosen to be **clearly distinct from the red live-record signal** so "selected" (indigo) and "recording" (red) never read as the same thing. |
| **Neutral ramp** | **Pure gray, no cast** | Fixes the old drift between several near-blacks/grays. |
| **Base background** | **Lighter charcoal `#1b1b1d`** | Softer than the old near-black; one canonical bg. |
| **Accent usage** | **Moderate** | Accent on primary actions + selection, plus full semantic colors throughout. |
| **Live-capture signal** | **Red dot `#ff5257`** (classic record) | Accent stays indigo everywhere else. |
| **Corner radius** | **Tight (4–6px)** | Crisp, native, professional. |
| **Dashboard actions** | Per-app **Start capture** in each Windows group header (replaces the per-app mic pill), **enabled only when that app has a window selected**; global **Refresh** moved into the Windows section header; **Import** moved into the Sessions section header. | Declutters the top of the dashboard; ties the primary action to a concrete capture target. |
| **Settings layout** | **Left navigation pane** + single content panel. | Replaces the long dense scroll. |
| **Permissions** | Both **granted** and **not-granted** states. | Not-granted uses error red + an accent "Grant access" action. |
| **Added states** | **Dashboard – permission denied** and **Playback – live**. | Cover the real runtime states the screenshots showed. |

---

## 3. Design tokens

All colors are `0xRRGGBB` for GPUI `rgb()`. Define these once as a palette module and reference by name.

### Neutral ramp (surfaces & borders)
| Token | Hex | Usage |
|---|---|---|
| `bg` | `#1b1b1d` | App background / canvas. The single base. |
| `panel` | `#242426` | Cards, grouped sections, list containers. |
| `elevated` | `#2d2d30` | Dropdown menus, modal card, hovered list rows & inputs. |
| `border` | `#3a3a3d` | 1px dividers, default control outlines. |
| `border_strong` | `#4a4a4e` | Input borders, hovered/focused outlines. |
| `nav_bg` | `#1c1c1e` | Settings left-nav pane background. |
| `chip_idle` | `#2a2a2c` | Idle chip/control fill (sits on `bg`). |

### Accent — indigo
| Token | Hex | Usage |
|---|---|---|
| `accent` | `#6366f1` | Primary button fill, active selection bar, focus, progress fill. |
| `accent_hover` | `#7c7ef5` | Hover on primary / selected. |
| `accent_active` | `#5457e0` | Pressed. |
| `accent_subtle` | `#262747` | Selected chip fill, "on" toggle bg, selected row bg. |
| `accent_border` | `#3d3f6e` | Border on selected chips & focus rings. |
| `accent_text` | `#a5a8f7` | Links, hotkey hints, accent text on dark. |
| `accent_text_strong` | `#b9bbf9` | Text inside a selected (`accent_subtle`) surface. |
| `on_accent` | `#ffffff` | Text/icons on an accent fill. |

### Text
| Token | Hex | Usage |
|---|---|---|
| `text_primary` | `#f2f2f3` | Headings, ids, values, active labels. |
| `text_secondary` | `#b6b6bb` | Body copy, row labels. |
| `text_muted` | `#86868c` | Metadata, hints, section eyebrows, placeholder. |
| `text_disabled` | `#5a5a5f` | Disabled text & inactive icons. |

### Semantic
| Token | Hex | Usage |
|---|---|---|
| `success` | `#3ecf8e` | Reachable, complete, saved, "downloaded", "granted". |
| `success_subtle` | `#16291f` | Success pill bg. |
| `warning` | `#f5b544` | Advisory messages, "update available". |
| `warning_subtle` | `#2e2410` | Warning pill bg. |
| `error` | `#f2555a` | Errors, destructive text, "not granted". |
| `error_subtle` | `#311a1c` | Destructive button fill / blocking banner bg. |
| `error_border` | `#5e2a2d` | Destructive button / banner border. |
| `live` | `#ff5257` | Recording / live dot. **Red — never the accent.** |
| `info` | `#4c9aff` | Neutral informational accents. |

### Type scale (Inter; mono = JetBrains Mono for ids/sizes/paths)
| Role | Size / Weight | Notes |
|---|---|---|
| Title | 20 / 600 | Window title "Capture". |
| Section heading | 18 / 600 | Settings content panel title. |
| Heading | 15 / 600 | Column headers ("Windows", "Sessions"), card titles. |
| Body strong | 13 / 500 | Row labels, button labels. |
| Body | 13 / 400 | Default body, list rows. |
| Small | 12 / 400 | Metadata, hints, status pills. |
| Eyebrow | 11 / 600, +0.06em, UPPERCASE | Section eyebrows. |

### Spacing scale — 4px base
`4 · 8 · 12 · 16 · 20 · 24 · 32`. Card padding 16–20; content panel padding 22–26; row gaps 12–14.

### Radius (tight)
`sm 5px` (chips, buttons, inputs) · `md 6px` (cards, list rows, dropdown fields) · `lg 8px` (panels, modal card).

---

## 4. Component specs

> Full interactive state matrix is in the **Components** reference (`Capture Components.dc.html`).
> Condensed here:

- **Button** — height 32, radius 5, icon gap 7–8.
  - *Primary:* fill `accent` → `accent_hover` hover → `accent_active` pressed, text `on_accent`, 13/600.
  - *Secondary:* fill `elevated`, 1px `border` → border `border_strong` hover, text `text_primary`, 13/500.
  - *Ghost:* transparent → white@5% hover, text `text_secondary`, 13/500.
  - *Destructive:* fill `error_subtle`, 1px `error_border`, text `error`, 13/500.
  - *Disabled:* fill `elevated`, text `text_disabled`, no hover.
- **Chip / toggle** — height 29, padding 6×12, radius 5, 13/500.
  - idle `chip_idle`/`text_muted` · hover `elevated`/`text_secondary` · selected `accent_subtle`/`accent_text_strong` + 1px `accent_border` · disabled `#1f1f22`/`text_disabled`.
- **Selectable list row** — padding 10×12, radius 6, gap 12.
  - idle `panel`/1px `border` · hover `elevated`/1px `border` · **selected** `accent_subtle`/1px `accent_border` **+ 2px `accent` left bar**.
  - id = mono 13/600 `text_primary`; meta = 12/400 `text_muted`; action icons 15px `text_muted` → `text_secondary` hover; delete-icon hover → `error`. Checkbox 14px; checked = `accent` fill + white check.
- **Dropdown** — field padding 8×12, radius 6, `panel`/1px `border` (hover border `border_strong`, open border `accent_border`). Menu `elevated`/1px `border`, pad 4; rows pad 7×10, radius 4, hover white@5%, selected `accent_subtle`/`accent_text`. Caret 15px `text_muted`.
- **Modal / card** — width 380, `panel`/1px `border`, radius 8, padding 20; backdrop `#000` @ 66%. Title 15/600; body 13/400 `text_secondary` (1.5); actions right-aligned, gap 10 (ghost Cancel + primary/destructive).
- **Progress bar** — track height 6, radius 3, `elevated`; fill `accent` (or `success` when complete). Label = mono 12 `text_secondary`, gap 14.
- **Section header** — *eyebrow* 11/600 +0.06em `text_muted` (+ optional 1px rule); *column header* = title 15/600 `text_primary` + count 12 `text_muted` + optional status pill (12 `success` on `success_subtle`, pad 3×9, radius 5).

---

## 5. Screen & state inventory (frames in the HTML)

1. **Settings** — left nav (Capture quality · Transcription · Voice recognition · Index endpoint · Skills · Permissions · App & updates) + content panel. Brand + daemon line + hotkey hint + status pill live in the nav.
2. **Dashboard** — header (Settings button) → Import (in Sessions header) / Refresh (in Windows header) → Mic chips → Launch field → two columns: **Windows** (app group headers each with a per-app **Start capture**, enabled iff a window of that app is checked) and **Sessions** (selectable rows + folder/copy/delete actions).
3. **Playback (saved)** — frame preview → time-aligned transcript caption → scrubber + transport (skip/rewind/play/ff/skip + volume + time) → **Manage** (screenshots/audio toggles, language, Re-transcribe / Halve frames / Delete frames / Remove audio / Build index) → **Index summary** (node count + root summary).
4. **Preset picker (modal)** — Auto / Meeting / Coding / Lecture / General / Custom, each label + one-line hint; one selected (`accent_subtle`); footer Cancel (ghost) + Start (primary).
5. **All settings — reference** — every Settings section expanded in one scroll (static reference; not a shipping screen).
6. **Dashboard — permission denied** — red blocking banner (Screen Recording required) + Open System Settings / Re-check; mic chips disabled + "Microphone denied"; Launch disabled; **Windows column = blocked empty state** (alert + CTA); **Sessions remain usable**.
7. **Playback — live** — `REC` badge on the frame; streaming live transcript (settled line muted, current line bright + caret); **Stop capture** (red) + elapsed timer + level meter + segment count; live mic + language selectors. No scrubber / no post-session actions.

### Voice recognition (key new behavior)
A **runtime selector** drives the model list:
- **CPU · whisper.cpp** / **Core ML** → a local **Whisper model manager**: rows of `name · size` with state →
  - *downloaded* → `success` "downloaded" + **Use** (secondary) + **Remove** (destructive);
  - *active* → `success` dot + "active" + **Remove**, row tinted `#1f2033`;
  - *available* → **Download** (primary).
  - Core ML shows a **different model set** than CPU (this is the "models depend on runtime" rule — wire each runtime to whatever models it actually supports).
- **CUDA** → unavailable state ("no NVIDIA GPU detected") with a disabled install action (on Windows this is the installable-runtime-pack path).
- **Remote endpoint** → endpoint + model config, no local downloads.

---

## 6. Task decomposition for Claude Code

Implement against `gui/src/app.rs`. Suggested order:

1. **Palette + type + spacing module.** Add the tokens from §3 as named constants (color, font size, weight, spacing, radius). Replace ad-hoc literals app-wide. *Acceptance:* no raw hex left in component code; one source of truth.
2. **Primitive widgets.** Build `button(variant)`, `chip(state)`, `list_row(state)`, `dropdown`, `modal`, `progress_bar`, `section_header`, `status_pill` per §4, each with hover via `hover(|s| …)`. *Acceptance:* matches the Components reference states.
3. **Icon set.** Standardize on the existing line icons (stroke = single tint color), sized 12–18. Add the few new glyphs used: shield (permissions), waveform/volume (voice), alert-triangle (banner), stop (live), check, caret-down, x.
4. **Settings shell.** Left nav pane (`nav_bg`, 1px right border) with active item = `accent_subtle`/`accent_text_strong`; content panel switches on selected section. Move brand/daemon/hotkey/status into the nav.
5. **Settings sections.** Capture quality, Transcription, Index endpoint (existing content, restyled) + **Voice recognition** (runtime selector → runtime-dependent model manager, §5), **Skills** (install/update rows), **Permissions** (granted/not-granted rows + Restart daemon), **App & updates** (version + progress bar).
6. **Dashboard.** Header; Refresh in Windows header; Import in Sessions header; per-app Start capture (enabled iff a window of that app is selected); restyled Windows picker + Sessions list with the new selectable-row pattern.
7. **Dashboard permission states.** Wire real permission checks → blocking banner + Windows blocked empty state when Screen Recording is denied; disable mic chips + Launch when mic denied; keep Sessions available.
8. **Playback (saved + live).** Saved: scrubber + transport + Manage + Index summary. Live: REC badge, streaming transcript, Stop capture, elapsed/level/segment count, live mic + language. Share layout; branch on session state.
9. **Preset picker + confirm modals.** Centered card on dimmed backdrop per §4.

---

## 7. GPUI translation notes (do NOT port literally)

- **Brand-mark & spacing-bar gradients** in the HTML are decorative; a flat fill or the existing app icon is fine. Simple linear gradients are allowed but optional.
- **Playback frame preview** is a striped CSS placeholder standing in for the real captured screenshot — render the actual frame image there.
- **Window title bar** uses generic minimize/maximize/close glyphs for a cross-platform mock. Use the **native** window chrome per OS (macOS traffic lights / Windows caption buttons); don't hardcode the mock glyphs.
- **Streaming caret** on the live transcript is a static bar in the mock; a simple blink is acceptable (single-property state change) but optional.
- **Focus rings / selection** = a 1px `accent_border` outline + `accent_subtle` fill. **No glow / no box-shadow.**
- **Elevation** = the neutral ramp + a 1px border, never a soft/multi-layer shadow.
- The **runtime → model-list** dependency is real app logic: render model rows from what the selected runtime reports it supports, not a fixed list.

---

## 8. Original task framing (for context)

The redesign was delivered in three reviewed stages: **(1)** a refreshed dark design system
(tokens + type + spacing), **(2)** component specs with exact tokens/sizes, **(3)** screen mockups
with the system applied. Subsequent iterations added the dashboard action restructure, the Settings
left-nav with Voice recognition / Skills / Permissions, granted & not-granted permission states,
and the permission-denied dashboard + live playback states. The HTML export reflects the final state.
