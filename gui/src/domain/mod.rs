//! Action methods on `CaptureApp`, grouped by domain. Relocated verbatim from `app.rs`
//! (#68 refactor — pure code relocation). Each submodule is an `impl CaptureApp` block.

pub(crate) mod asr;
pub(crate) mod capture;
pub(crate) mod indexing;
pub(crate) mod sessions;
