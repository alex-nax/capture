//! Reusable widget free functions (no `&self`) used across the GUI screens.
//! Relocated verbatim from `app.rs` (#68 refactor).

pub(crate) mod button;
pub(crate) mod chip;
pub(crate) mod icon;

pub(crate) use button::button;
pub(crate) use chip::chip;
pub(crate) use icon::icon;
