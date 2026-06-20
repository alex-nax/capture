//! Screen render methods on `CaptureApp`, one `impl CaptureApp` block per file.
//! Relocated verbatim from `app.rs` (#68 refactor). `render()` in `app.rs` builds the
//! shell and dispatches to these.

pub(crate) mod dashboard;
pub(crate) mod modals;
pub(crate) mod playback;
pub(crate) mod settings;
