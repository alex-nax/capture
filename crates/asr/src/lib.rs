//! capture-asr — the v3 speech-to-text layer (port of `core/asr/`), engine-agnostic.
//!
//! One swappable [`AsrBackend`] trait turns mono float32 PCM chunks into timestamped [`Segment`]s.
//! Local engines are **dynamically-loaded cdylibs** (whisper.cpp via `capture-asr-whisper` now;
//! mlx-rs and Windows engines later) that the lean daemon `dlopen`s through [`DynamicEngine`] — so the
//! daemon embeds no engine and a runtime is an optional, shippable component. Remote engines (OpenAI-
//! compatible, Riva) are in-process HTTP (later #64 pieces). The runtime MANAGER that registers,
//! selects, and loads these engines is the next #64 piece.
//!
//! Port order (#64): [1] trait + whisper engine (validated) → **[1b] make the engine a dlopen'd cdylib
//! (this change)** → [2] the runtime manager (registry/select/availability) + GGML model manager → [3]
//! remote backends → [4] wire the daemon's `/v1/asr/*` routes + the capture loop (with #65).

pub mod backend;
pub mod config;
pub mod dynamic;
pub mod models;
pub mod runtime;

pub use backend::{
    is_silent, resample_linear, silence_rms16, AsrBackend, Segment, TARGET_SAMPLE_RATE,
};
pub use dynamic::{engine_dylib_filename, DynamicEngine, ENGINE_ABI_VERSION};
pub use models::{CatalogEntry, CHUNK_BOUNDS, DEFAULT_CHUNK_SECONDS, DEFAULT_MODEL};
pub use runtime::{AsrRuntimeInfo, AsrRuntimeManager};

#[cfg(test)]
pub(crate) mod testlock {
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());

    /// Serialize every test that mutates process-global env (`CAPTURE_CONFIG`,
    /// `CAPTURE_GGML_BASE_URL`, `CAPTURE_OPENAI_ASR_URL`, …) across *all* modules in this crate — a
    /// per-module lock doesn't, so they'd race on the shared config path. Poison-tolerant: one
    /// panicking test releases the lock cleanly instead of cascading `PoisonError`s into the rest.
    pub(crate) fn guard() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}
