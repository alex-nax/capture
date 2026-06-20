//! Design tokens (design/CAPTURE-HANDOFF.md §3). One source of truth — no raw hex/size
//! literals elsewhere. Colors are `0xRRGGBB` for GPUI `rgb(theme::TOKEN)` (or `0xRRGGBBAA`
//! for `rgba(theme::TOKEN)`). Sizes are `f32` px. Some tokens are defined ahead of use
//! (the type scale + several color/radius/spacing values land in #70+), so allow dead_code.
#![allow(dead_code)]

// ── Neutral ramp (surfaces & borders) ───────────────────────────────────────
pub(crate) const BG: u32 = 0x1b1b1d; // App background / canvas. The single base.
pub(crate) const PANEL: u32 = 0x242426; // Cards, grouped sections, list containers.
pub(crate) const ELEVATED: u32 = 0x2d2d30; // Dropdowns, modal card, hovered rows/inputs.
pub(crate) const BORDER: u32 = 0x3a3a3d; // 1px dividers, default control outlines.
pub(crate) const BORDER_STRONG: u32 = 0x4a4a4e; // Input borders, hovered/focused outlines.
pub(crate) const NAV_BG: u32 = 0x1c1c1e; // Settings left-nav pane background.
pub(crate) const CARD_BORDER: u32 = 0x232326; // Card outline — darker than BORDER (structural, not a control).
pub(crate) const HAIRLINE: u32 = 0x2a2a2d; // Nav edge / section divider hairline.
pub(crate) const CHIP_IDLE: u32 = 0x2a2a2c; // Idle chip/control fill (sits on BG).
pub(crate) const CHIP_DISABLED: u32 = 0x1f1f22; // Disabled chip fill (§4 chip matrix).

// ── Overlays (rgba) — use via rgba(theme::TOKEN) ─────────────────────────────
pub(crate) const GHOST_HOVER: u32 = 0xffffff0d; // Ghost-button / menu-item hover: white @ ~5%.
pub(crate) const TRANSPARENT: u32 = 0x00000000; // Fully transparent (e.g. unselected list-row left bar — keeps layout stable).

// ── Accent — indigo ─────────────────────────────────────────────────────────
pub(crate) const ACCENT: u32 = 0x6366f1; // Primary fill, active selection bar, focus, progress.
pub(crate) const ACCENT_HOVER: u32 = 0x7c7ef5; // Hover on primary / selected.
pub(crate) const ACCENT_ACTIVE: u32 = 0x5457e0; // Pressed.
pub(crate) const ACCENT_SUBTLE: u32 = 0x262747; // Selected chip fill, "on" toggle bg, selected row bg.
pub(crate) const ACTIVE_ROW: u32 = 0x1f2033; // Active model-row tint (Voice section, §5).
pub(crate) const ACCENT_BORDER: u32 = 0x3d3f6e; // Border on selected chips & focus rings.
pub(crate) const ACCENT_TEXT: u32 = 0xa5a8f7; // Links, hotkey hints, accent text on dark.
pub(crate) const ACCENT_TEXT_STRONG: u32 = 0xb9bbf9; // Text inside a selected (ACCENT_SUBTLE) surface.
pub(crate) const ON_ACCENT: u32 = 0xffffff; // Text/icons on an accent fill.

// ── Text ─────────────────────────────────────────────────────────────────────
pub(crate) const TEXT_PRIMARY: u32 = 0xf2f2f3; // Headings, ids, values, active labels.
pub(crate) const TEXT_SECONDARY: u32 = 0xb6b6bb; // Body copy, row labels.
pub(crate) const TEXT_MUTED: u32 = 0x86868c; // Metadata, hints, section eyebrows, placeholder.
pub(crate) const TEXT_DISABLED: u32 = 0x5a5a5f; // Disabled text & inactive icons.

// ── Semantic ─────────────────────────────────────────────────────────────────
pub(crate) const SUCCESS: u32 = 0x3ecf8e; // Reachable, complete, saved, downloaded, granted.
pub(crate) const SUCCESS_SUBTLE: u32 = 0x16291f; // Success pill bg.
pub(crate) const WARNING: u32 = 0xf5b544; // Advisory messages, "update available".
pub(crate) const WARNING_SUBTLE: u32 = 0x2e2410; // Warning pill bg.
pub(crate) const ERROR: u32 = 0xf2555a; // Errors, destructive text, "not granted".
pub(crate) const ERROR_SUBTLE: u32 = 0x311a1c; // Destructive button fill / blocking banner bg.
pub(crate) const ERROR_BORDER: u32 = 0x5e2a2d; // Destructive button / banner border.
pub(crate) const LIVE: u32 = 0xff5257; // Recording / live dot. Red — never the accent.
pub(crate) const INFO: u32 = 0x4c9aff; // Neutral informational accents.

// ── Modal backdrop (rgba) — use via rgba(theme::BACKDROP); #000 @ 66% ────────
pub(crate) const BACKDROP: u32 = 0x000000a8;

// ── Radius (tight) — px ──────────────────────────────────────────────────────
pub(crate) const RADIUS_SM: f32 = 5.0; // chips, buttons, inputs
pub(crate) const RADIUS_MD: f32 = 6.0; // cards, list rows, dropdown fields
pub(crate) const RADIUS_LG: f32 = 8.0; // panels, modal card

// ── Spacing scale — 4px base (px) ────────────────────────────────────────────
pub(crate) const SP_1: f32 = 4.0;
pub(crate) const SP_2: f32 = 8.0;
pub(crate) const SP_3: f32 = 12.0;
pub(crate) const SP_4: f32 = 16.0;
pub(crate) const SP_5: f32 = 20.0;
pub(crate) const SP_6: f32 = 24.0;
pub(crate) const SP_8: f32 = 32.0;

// ── Type scale (Inter; mono = JetBrains Mono for ids/sizes/paths) ────────────
// Sizes/weights in px; applied in #70+ (this pass does not change .text_*/weight usages).
pub(crate) const TS_TITLE: f32 = 20.0; // Window title "Capture". weight 600.
pub(crate) const TS_SECTION: f32 = 18.0; // Settings content panel title. weight 600.
pub(crate) const TS_HEADING: f32 = 15.0; // Column headers, card titles. weight 600.
pub(crate) const TS_BODY: f32 = 13.0; // Body / list rows. strong = weight 500, body = 400.
pub(crate) const TS_SMALL: f32 = 12.0; // Metadata, hints, status pills. weight 400.
pub(crate) const TS_EYEBROW: f32 = 11.0; // Section eyebrows. weight 600, +0.06em, UPPERCASE.

pub(crate) const FW_REGULAR: u16 = 400;
pub(crate) const FW_MEDIUM: u16 = 500;
pub(crate) const FW_SEMIBOLD: u16 = 600;
