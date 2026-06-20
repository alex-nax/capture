# Task: Session row states (Dashboard › Sessions list)

Targeted change to the **Sessions** list rows on the Dashboard. Scope is *only* the session row —
do not touch other screens. Tokens referenced are from the Capture v3 palette
(`accent #6366f1`, `live #ff5257`, `panel #242426`, `border #232326`, `text_primary #f2f2f3`,
`text_muted #86868c`, `accent_text #a5a8f7`).

---

## Why

Rows previously had a persistent **"selected"** treatment (accent-subtle fill `#262747` + a 2px
`accent` left bar). That state is wrong: clicking a row navigates straight to **Playback**, so a
row is never left in a selected/highlighted resting state. Remove it, and use the row chrome to
show the states a session *actually* has — **live** and **indexing**.

---

## Before → After

### 1. Default / stopped row — remove the selection treatment
**Before**
```
bg #262747 · 1px border #3d3f6e · 2px accent (#6366f1) left bar
id #f2f2f3 · meta "stopped · 453s · 134 seg" #b9bbf9
actions: folder, copy (both #b9bbf9) · trash #ff7176
```
**After** (plain resting row — same as every other stopped row)
```
bg #242426 · 1px border #232326 · NO left bar
id #f2f2f3 (mono 13/600) · meta "stopped · 453s · 134 seg" #86868c
actions: folder #86868c · copy #86868c · trash #86868c (→ #f2555a on hover)
```

### 2. Live row — trash becomes Stop
A session that is currently recording.
**Before:** n/a (no live affordance in the list).
**After**
```
bg #242426 · 1px border #232326
id #f2f2f3 (mono 13/600)
status: ● red dot (#ff5257, 6px) + "live · 312s · 94 seg"  in #ff5257
actions: folder #86868c · copy #86868c · STOP icon #ff5257   ← replaces the trash icon
```
- Use a filled **stop** glyph (rounded square) in `live #ff5257` where the trash icon would be.
- Clicking Stop ends the capture (it does NOT delete); trash is intentionally absent while live.

### 3. Indexing row — left bar becomes a percent-width progress fill
Indexing is a long post-capture process; the row shows its progress inline.
**Before:** the 2px **vertical** accent bar on the left edge (from the old selected state).
**After:** that accent strip becomes a **horizontal fill along the bottom edge, width = percent**.
```
bg #242426 · 1px border #232326 · position relative, clip overflow
id #f2f2f3 (mono 13/600)
status: spinner/refresh glyph (#a5a8f7) + "indexing · 62%"  in accent_text #a5a8f7
actions: folder #86868c · copy #86868c · trash #86868c
progress fill: absolute, bottom:0, left:0, height 2px, width = {percent}% , bg accent #6366f1
```
- Bind `width` to the live index-build percentage (0–100).
- Keep the standard actions; indexing doesn't block delete.

---

## State selection logic
```
match session.state {
    Live      => live row   (red status + Stop icon, no trash),
    Indexing  => stopped chrome + indexing status + bottom progress fill (width = pct),
    Stopped   => plain row (folder / copy / trash),
}
```
No row ever uses the old `#262747` fill / `accent` left-bar selection styling.

---

## Acceptance
- Stopped rows are plain `panel`/`border`, no accent fill or left bar.
- A live session shows the red dot + "live …" and a red **Stop** in place of trash.
- An indexing session shows "indexing · NN%" and a `accent` bar filling the bottom edge to NN%.
- Clicking any row → Playback (unchanged).
