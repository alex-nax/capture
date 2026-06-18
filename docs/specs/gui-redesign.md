# Spec: GUI v3 redesign + `app.rs` decomposition

_Status: **PLANNED** (v3). Visual source of truth = `design/CAPTURE-HANDOFF.md` + `design/Capture Screens
(standalone).html` (the Claude-design export). This spec covers HOW we land it: the module decomposition,
the rules, and the feature breakdown. Update it in the same change as the code._

## Purpose
Two things at once, both on the `gui/` crate:
1. **Decompose the monster** — `gui/src/app.rs` is **4,116 lines**: one ~1,100-line `render()` with the
   dashboard + settings inline, plus `render_playback`, all the action methods, and the `icon`/`button`/
   `chip` free fns. Split it **by screens, components, and domains** so each file is small and single-purpose.
2. **Apply the redesign** — the locked dark system from the handoff (indigo `#6366f1` accent, pure-gray
   neutral ramp on `#1b1b1d`, tight 4–6px radii, full semantic colors, Inter + JetBrains Mono, a 4px spacing
   scale) across new component primitives and restyled/restructured screens.

The decomposition lands **first** (behavior-preserving), so the redesign work happens in small, focused
files instead of a 4k-line monolith.

## Module decomposition (the target `gui/src/` layout)
```
gui/src/
├── main.rs               entry (unchanged)
├── app.rs                SLIM — the CaptureApp struct, new(), the poll loop, and impl Render::render that
│                         DISPATCHES to the screen modules (dash / settings / playback) + overlays. ~300 lines.
├── state.rs              the state types pulled out of app.rs: LiveState, ConfirmKind, the settings-nav enum, etc.
├── theme.rs              design tokens as named constants — color (rgb), font size, weight, spacing, radius
│                         (handoff §3). The ONE source of truth; no raw hex/size literals in component/screen code.
├── components/           reusable widgets (FREE functions, parameterized by state — handoff §4)
│   ├── mod.rs            re-exports
│   ├── button.rs         button(variant: Primary|Secondary|Ghost|Destructive, …)
│   ├── chip.rs           chip(state: Idle|Hover|Selected|Disabled, …)
│   ├── list_row.rs       selectable list row (idle/hover/selected + 2px accent left-bar; checkbox; action icons)
│   ├── dropdown.rs       dropdown field + menu (generalizes today's language_field / model dropdown)
│   ├── modal.rs          centered card + dimmed backdrop
│   ├── progress.rs       progress bar (accent fill, success when complete)
│   ├── section.rs        section header / eyebrow / status pill
│   └── icon.rs           icon(name, sz, color) (SVG, single-tint)
├── screens/              one module per screen/state (handoff §5) — render methods as `impl CaptureApp` blocks
│   ├── mod.rs
│   ├── dashboard.rs      dashboard + the permission-denied state (banner + Windows blocked empty state)
│   ├── settings/
│   │   ├── mod.rs        the Settings SHELL: left nav (brand/daemon/hotkey/status + nav items) + content dispatch
│   │   ├── capture_quality.rs
│   │   ├── transcription.rs
│   │   ├── voice.rs      Voice recognition: runtime selector → runtime-dependent model manager
│   │   ├── index_endpoint.rs
│   │   ├── skills.rs
│   │   ├── permissions.rs    granted + not-granted rows
│   │   └── updates.rs    version + auto-update progress
│   ├── playback.rs       saved + live playback (shared layout, branch on session state)
│   └── preset_picker.rs  the Start preset modal
├── domain/               app logic (non-render) as `impl CaptureApp` blocks, grouped by domain
│   ├── mod.rs
│   ├── capture.rs        start_capture / stop / mic switch / preset apply
│   ├── indexing.rs       index_session / fetch_index_models / live-index status
│   ├── asr.rs            model manager (download/use/remove), language, chunk, runtime select
│   └── sessions.rs       delete / prune / retranscribe / import / open-folder / copy
├── daemon.rs             the /v1 client (data layer — unchanged)
├── update.rs  skill.rs  tray.rs  hotkey.rs  assets.rs   (unchanged)
```

### Decomposition rules
- **`CaptureApp` stays the single GPUI model**; its fields become **`pub(crate)`** so the `impl CaptureApp`
  blocks in `screens/` and `domain/` (same crate) can read/mutate state. (The orphan rule allows `impl` in any
  module of the defining crate.)
- **Components are free functions** taking explicit params + a listener — no `&self`. **Screens are
  `impl CaptureApp` methods** (they need broad state). **Domain actions are `impl CaptureApp` methods** grouped
  by concern.
- **No raw literals** outside `theme.rs` — colors/sizes/spacing/radii come from named tokens. *Acceptance for
  the decomposition+theme features:* `grep -nE "rgb\(0x|px\([0-9]" gui/src/screens gui/src/components` returns
  only `theme::` references.
- The decomposition feature is **behavior-preserving** — same screens, same actions, just relocated +
  `cargo build` clean; the visual change comes in the later features.

## Design system + components + screens
The tokens (§3), component state matrices (§4), screen & state inventory (§5 — Settings/left-nav,
Dashboard + permission-denied, Playback saved + live, preset picker, the runtime→model-list rule), and the
**GPUI translation notes** (§7 — no glow/blur/multi-layer shadows; elevation = neutral ramp + 1px border;
native window chrome; real frame image in playback; runtime-driven model list) are authoritative in
`design/CAPTURE-HANDOFF.md`. Implement to that + the HTML export; this spec does not duplicate them.

## Notable behavior changes (beyond styling)
- **Dashboard**: per-app **Start capture** in each Windows group header (enabled **iff** a window of that app
  is selected) replaces the per-app mic pill; **Refresh** → Windows header; **Import** → Sessions header.
- **Settings**: long scroll → **left-nav pane + content panel**; brand/daemon/hotkey/status move into the nav.
- **Voice recognition**: a runtime selector (CPU·whisper.cpp / Core ML / CUDA / Remote) drives a
  **runtime-dependent** model manager (each runtime lists the models it actually supports; CUDA = installable
  pack path / unavailable; Remote = endpoint config). Wires the existing ASR-model + runtime-pack (#58) APIs.
- **Permissions**: explicit granted / not-granted rows; the **permission-denied dashboard** (blocking banner,
  Windows blocked empty state, mic/launch disabled, Sessions still usable) is a real state to wire to the
  `/v1/permissions` checks.
- **Live playback**: REC badge + streaming transcript + Stop/elapsed/level/segments + live mic & language.

## Implementation features (order = dependency order)
`#68` decompose → `#69` theme → `#70` components+icons → then the screens (`#71` settings shell, `#72`
settings sections, `#73` dashboard, `#74` dashboard permission states, `#75` playback saved+live, `#76` preset/
confirm modals). See `features.json`.

## Tests / acceptance
- Each feature: `cargo build --release` clean; matches the handoff/HTML for that screen/component.
- Decomposition + theme: the no-raw-literals grep above; no behavior change vs the pre-refactor app.
- No new web-only effects (blur/backdrop-filter/box-shadow) — GPUI primitives only (handoff §7).
