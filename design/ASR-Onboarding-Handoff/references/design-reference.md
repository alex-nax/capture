# Design reference — tokens & components used by ASR onboarding

Scoped subset of `CAPTURE-HANDOFF.md` §3–4 — only what this feature touches. All colors are
`#RRGGBB` for GPUI `rgb()`; reference by token name, no raw hex in component code.

## Colors

| Token | Hex | Used for (in this feature) |
|---|---|---|
| `bg` | `#1b1b1d` | App canvas; idle row fill |
| `panel` | `#242426` | Hero CTA card, confirmation strip, picker cards |
| `elevated` | `#2d2d30` | Progress-bar track; row borders |
| `border` | `#3a3a3d` | Card / row outlines |
| `accent` | `#6366f1` | Primary buttons, selection bar, **progress fill** |
| `accent_subtle` | `#262747` | Selected runtime/model row, active-model tint, CTA icon chip |
| `accent_border` | `#3d3f6e` | Selected-row / focus outline |
| `accent_text` | `#a5a8f7` | "Set it up" link, hotkey hint |
| `accent_text_strong` | `#b9bbf9` | Text on a selected surface |
| `text_primary` | `#f2f2f3` | Titles, labels, model names |
| `text_secondary` | `#b6b6bb` | Body copy |
| `text_muted` | `#86868c` | Sizes, requirements, "this happens once" |
| `text_disabled` | `#5a5a5f` | "Unavailable" |
| `success` | `#3ecf8e` | active / downloaded / "Transcription is on" |
| `success_subtle` | `#16291f` | Ready-confirmation icon chip |
| `warning` / `warning_subtle` | `#f5b544` / `#2e2410` | "Update available" chip |
| `error` | `#f2555a` | failed / offline |
| `error_subtle` / `error_border` | `#311a1c` / `#5e2a2d` | Offline CTA card |

> **Do not** use the older blue palette (`#2d4f67` / `#8ab4f8` / `#4a90d9`) from the original brief —
> the app standardized on indigo `#6366f1` (incl. progress fill). Red `#ff5257` stays reserved for
> live-record and is never reused here.

## Type — Inter; mono = JetBrains Mono (ids / sizes / repos / paths)

| Role | Size / Weight |
|---|---|
| Card title | 14–15 / 600 |
| Body | 12–13 / 400 |
| Row label | 13 / 500–600 |
| Eyebrow | 11 / 600, +0.06em, UPPERCASE |
| Size / progress label | 11–12 / mono |

## Components reused (GPUI specs in `CAPTURE-HANDOFF.md` §4)

- **Card** — `panel` + 1px `border`, radius 8, padding 16–20.
- **Selectable list row** — idle `panel`/1px `border`; **selected** `accent_subtle`/1px `accent_border` + 2px `accent` left bar. Runtime rows + active model row.
- **Button** — Primary `accent` · Secondary `elevated`+border · Ghost transparent · Destructive `error_subtle`/`error_border`. Height 32, radius 5.
- **Progress bar** — track `elevated`, fill `accent`; mono label. Thin inline on an engine row; full-width + size/%/ETA for a model.
- **Chip** — status + the `Update available` chip (`warning` on `warning_subtle`).

## New component

- **Download icon** — tray + down-arrow, 14px, single-tint stroke. The only new glyph. (No icon on the "Remote" engine row — it matches the other engine rows.)

## Radius & spacing

Radius: 5 (chips/buttons/inputs) · 6 (rows/fields) · 8 (cards). Spacing on a 4px base
(`4·8·12·16·20·24`); card padding 16–20, row gaps 12–14.
