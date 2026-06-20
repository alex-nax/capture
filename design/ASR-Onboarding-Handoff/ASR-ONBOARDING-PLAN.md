# Capture — ASR Onboarding: Feature Plan

A scoped implementation plan for **one feature**: getting on-device speech-to-text working on a
fresh install, now that the ASR **runtime** (speech engine) and the **model** are downloaded
separately rather than bundled. Nothing outside this flow is covered here.

> **Visual source of truth:** the ASR frames in `Capture Screens.dc.html` (frame labels prefixed
> `ASR ·`). **Token / component source:** `CAPTURE-HANDOFF.md` §3–4. **Original requirements:**
> `uploads/ASR-ONBOARDING-BRIEF.md`. Stack is native **GPUI (Rust)** — these are GPUI views built
> from the existing component kit, not web UI.

---

## 1. What we're building

On a fresh install Capture ships with **no engine and no model**, so transcription is off (captures
still record audio + screenshots — they just produce no transcript). The feature:

1. **Invites** the user — once, calmly — to turn transcription on, from the dashboard.
2. Lets them **pick + download a runtime** and a **model** inline in Settings → Voice.
3. **Confirms** when both are active, then gets out of the way.
4. **Recovers** gracefully from "Not now", partial setup, and failed/offline downloads.

**Placement decision (locked):** the pickers live **inline in Settings → Voice**; the dashboard CTA
**deep-links** there. We are *not* building a separate first-run modal — the runtime + model UI
already belongs in Settings, so one home keeps a single source of truth. The dashboard surface is
only the invite + the resolved confirmation.

---

## 2. States → frames (the mocks to build against)

| # | State | Frame in `Capture Screens.dc.html` |
|---|---|---|
| 1 | Dashboard CTA — no engine + no model (the hero) | **ASR · 1 — Dashboard CTA (hero)** |
| 1b | CTA minimised after "Not now" (sits by Launch/Start) | **ASR · CTA recovery** (top card) |
| 2 | Runtime picker + download (not-installed / downloading / installed / active / unavailable / update-available) | **ASR · 2 — Settings → Voice · speech engine** |
| 3 | Model picker for a local runtime | **ASR · 3 + 4 — Settings → Voice · model & download** |
| 3R | Remote runtime → endpoint config, **no local model list** | existing **Settings · Voice recognition** Remote-runtime path (already in app) |
| 4 | Download in-flight (the long model download — size, %, ETA) | **ASR · 3 + 4** (the `large-v3-turbo` row) |
| 5 | Ready / done — CTA resolves to a compact confirmation | **ASR · 5 — Dashboard ready** |
| 6 | Partials & errors: engine-ready-no-model · offline/failed · update-available | **ASR · CTA recovery** + **ASR · 2** (update chip) + **ASR · 3 + 4** (failed row) |

**Recovery after "Not now":** the hero collapses to a quiet inline pill —
`🔊 Transcription is off · Set it up` — next to the Launch field. Never lost, never nagging. (See
**ASR · CTA recovery**.)

---

## 3. Daemon API — every action maps to a real route

```
GET  /v1/asr/runtimes              → { active, gpu, runtimes:[{id,label,kind,requires,installed,available,active}] }
POST /v1/asr/runtimes/install {id} → downloads the pack;  SSE event: asr_runtime_install (progress)
POST /v1/asr/runtime {id}          → make a runtime active

GET  /v1/asr/models                → { backend_available, active, models:[{repo,name,size_label,downloaded,active,downloading}] }
POST /v1/asr/models/download {repo}→ downloads a model;    SSE progress
POST /v1/asr/model {repo}          → activate a model
POST /v1/asr/models/delete {repo}  → remove a model
```

Rules: **nothing auto-selects**; transcription is on **iff** a runtime *and* a model are both active.
The **model list must always match the selected runtime** (a local runtime → its model catalog; the
Remote runtime → endpoint config, no local list). Engine packs are small/quick; a model is the long
download (up to ~1.6 GB) — make its progress feel intentional. **No silent fallback:** never imply a
transcript is being produced when no runtime+model are active.

### State → call

- Dashboard CTA shown when `GET /v1/asr/runtimes`.active is null **and** `GET /v1/asr/models`.active is null.
- "Almost there — pick a model" when a runtime is active but no model.
- "Pick an engine" when a model is downloaded but no runtime active.
- Runtime row buttons: Download → `runtimes/install`; Use → `runtime {id}`; Update → `runtimes/install` on the newer pack.
- Model row buttons: Download → `models/download`; Use → `model {repo}`; Remove → `models/delete`; Retry → re-issue `models/download`.
- Ready confirmation reads `runtimes.active.label` + `models.active.name`.

---

## 4. Reuse the existing kit (port cleanly to GPUI)

All from `CAPTURE-HANDOFF.md` §4 — **no new components** except one icon.

- **Card** (`panel` `#242426`, 1px `border` `#3a3a3d`, radius 8) — the hero CTA, the confirmation, each picker section.
- **Selectable list row** (idle `panel` · selected `accent_subtle` `#262747` + 1px `accent_border` `#3d3f6e` + 2px `accent` left bar) — runtime rows & the active model row.
- **Button** — Primary (`accent` `#6366f1`) for Set up / Download / Pick a model; Secondary for Use / Retry; Ghost for Not now / Cancel / Update; Destructive for Remove.
- **Chip** — the existing status/eyebrow treatments; the `Update available` chip mirrors the app's update affordance (`warning` `#f5b544` on `warning_subtle` `#2e2410`).
- **Progress bar** — track `elevated` `#2d2d30`, fill `accent` `#6366f1`; mono label (size / % / ETA). Inline thin bar on a runtime row; full-width prominent bar for the model download.
- **Status text** — `success` `#3ecf8e` (active / downloaded / "Transcription is on"), `error` `#f2555a` (failed / offline), `text_muted` `#86868c` (sizes, requirements, "Unavailable").

**New glyph:** a download icon (tray + down-arrow), sized 14. *(No others — the "Remote" row uses no
leading icon, matching the other engine rows.)*

**Treatments to honor**
- *Unavailable* runtime (e.g. CUDA on a Mac): dimmed row + one-line reason, **not** a dead button.
- *Update available*: subtle inline chip on that runtime row, never a blocking modal.
- *Downloading*: inline % for an engine pack; full-width bar + `656 MB / 1.6 GB · 41% · ~2 min left` for a model.
- *Failed / offline*: warning/error-toned, **recoverable** (Retry), never silent.

---

## 5. Build order

1. **Data + visibility logic.** Wire `GET /v1/asr/runtimes` + `GET /v1/asr/models`. Derive the four
   top-level states: *none · engine-only · model-only · ready*. This drives both the dashboard CTA and
   the Settings step hints.
2. **Dashboard CTA (state 1).** Hero card (title + one-line body + reassurance, Primary deep-links to
   Settings → Voice, Ghost = "Not now"). Persist dismissal → render the minimised pill by Launch.
   *Acceptance:* dashboard stays fully usable (screenshots-only / audio-only capture works) while ASR is unconfigured.
3. **Settings → Voice · runtime picker (states 2, 6).** Rows from `/v1/asr/runtimes` with all row
   states; install via `runtimes/install` (subscribe `asr_runtime_install`); activate via `runtime`.
   Unavailable + update-available treatments.
4. **Settings → Voice · model picker (states 3, 4, 6).** Catalog from `/v1/asr/models` **for the active
   runtime**; download (SSE progress), activate, remove, retry. Remote runtime shows endpoint config
   instead of a model list.
5. **Step hints.** "Step 1 of 2 — pick an engine" → collapses to an engine summary with **Change**
   once a runtime is active; "Step 2 — pick a model".
6. **Ready confirmation (state 5).** When both active, replace the dashboard hero with the compact
   `Transcription is on · <runtime> · <model>` strip + quiet "Settings → Voice".

---

## 6. Acceptance / tone

- A user lands on the dashboard, understands **in one glance** that one quick download turns
  transcription on, does it, and never thinks about it again.
- Calm, one-time, honest. The CTA **invites**; it never traps. Nothing nags after "Not now".
- Transcription is reported on **only** when a runtime + model are both active.
