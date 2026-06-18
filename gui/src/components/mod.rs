//! Reusable widget free functions (no `&self`) used across the GUI screens.
//! Relocated verbatim from `app.rs` (#68 refactor); primitives extended in #70.

pub(crate) mod button;
pub(crate) mod card;
pub(crate) mod chip;
pub(crate) mod dropdown;
pub(crate) mod icon;
pub(crate) mod list_row;
pub(crate) mod modal;
pub(crate) mod progress;
pub(crate) mod section;

pub(crate) use button::{button, button_id, button_sm_id, ButtonVariant};
pub(crate) use chip::chip;
#[allow(unused_imports)]
pub(crate) use card::card;
pub(crate) use icon::icon;

// Primitives built in #70 but not wired into a screen until #71–#76. Re-exported
// for those features; silence the "unused import" until a call site lands.
#[allow(unused_imports)]
pub(crate) use button::{button_disabled, button_sm, icon_button};
#[allow(unused_imports)]
pub(crate) use chip::chip_disabled;
#[allow(unused_imports)]
pub(crate) use dropdown::{dropdown_field, dropdown_menu, dropdown_menu_item};
#[allow(unused_imports)]
pub(crate) use list_row::{checkbox, list_row};
#[allow(unused_imports)]
pub(crate) use modal::modal;
#[allow(unused_imports)]
pub(crate) use progress::progress_bar;
#[allow(unused_imports)]
pub(crate) use section::{column_header, eyebrow, status_pill};
