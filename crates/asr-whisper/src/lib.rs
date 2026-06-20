//! The whisper.cpp ASR engine, packaged as a **dynamically-loaded cdylib** behind the generic
//! `capture_asr_engine_*` C ABI. The lean `capture-daemon` links no engine; it `dlopen`s this dylib
//! (via `capture_asr::DynamicEngine`) when the whisper runtime is selected. whisper.cpp is linked
//! statically *into this dylib* (so it's a self-contained, shippable, optional component), reusing
//! `whisper-rs` rather than hand-binding whisper.cpp's `whisper_full_params` ABI.
//!
//! A future `capture-asr-mlx` cdylib (mlx-rs) implements this same ABI and loads through the same seam.
//!
//! ## The engine C ABI (v1)
//! - `uint32_t capture_asr_engine_abi_version(void)` — must match the loader's expected ABI.
//! - `const char* capture_asr_engine_name(void)` — a static engine id (`"whisper.cpp"`).
//! - `void* capture_asr_engine_load(const char* model_ref, const char* language)` — load a model
//!   (engine-interpreted ref: a GGML path for whisper); `language` nullable. NULL on failure.
//! - `char* capture_asr_engine_transcribe(void* h, const float* pcm, size_t n, uint32_t sample_rate)`
//!   — returns a malloc'd JSON array of `{start,end,text}` (caller frees via `free_string`); NULL on error.
//! - `void capture_asr_engine_set_language(void* h, const char* language)` — live language change.
//! - `void capture_asr_engine_free_string(char*)` / `void capture_asr_engine_free(void*)` — releases.
//! - `const char* capture_asr_engine_last_error(void)` — thread-local message for the last NULL return.

use std::cell::RefCell;
use std::ffi::{c_char, c_float, c_void, CStr, CString};
use std::ptr;
use std::sync::Mutex;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use capture_asr::{resample_linear, Segment, TARGET_SAMPLE_RATE};

/// The engine ABI version. Bump on any breaking change to the C ABI; the loader checks it.
const ABI_VERSION: u32 = 1;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_error(msg: impl Into<String>) {
    let c = CString::new(msg.into()).unwrap_or_else(|_| CString::new("error").unwrap());
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(c));
}

/// A *const c_char → Option<String> (NULL → None, invalid UTF-8 → None).
unsafe fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok().map(String::from)
}

// ── The engine ────────────────────────────────────────────────────────────────────────────────

/// A loaded whisper.cpp model (the piece-1 `WhisperLocal`, now living inside the engine dylib).
struct WhisperEngine {
    ctx: WhisperContext,
    language: Mutex<Option<String>>,
    n_threads: i32,
}

impl WhisperEngine {
    fn load(model_path: &str, language: Option<String>) -> Result<WhisperEngine, String> {
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| format!("load whisper model {model_path:?}: {e}"))?;
        Ok(WhisperEngine {
            ctx,
            language: Mutex::new(normalize_lang(language)),
            n_threads: default_threads(),
        })
    }

    fn transcribe(&self, pcm: &[f32], sample_rate: u32) -> Result<Vec<Segment>, String> {
        if pcm.is_empty() {
            return Ok(Vec::new());
        }
        let audio: std::borrow::Cow<[f32]> = if sample_rate == TARGET_SAMPLE_RATE {
            std::borrow::Cow::Borrowed(pcm)
        } else {
            std::borrow::Cow::Owned(resample_linear(pcm, sample_rate, TARGET_SAMPLE_RATE))
        };

        let mut state =
            self.ctx.create_state().map_err(|e| format!("whisper create_state: {e}"))?;

        // Snapshot the language to an owned local; set_language stores the &str into params, so it
        // must outlive full() below.
        let lang: Option<String> = self.language.lock().unwrap().clone();
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.n_threads);
        params.set_translate(false);
        params.set_language(lang.as_deref());
        // #45 hallucination guards (no cross-chunk priming + drop low-confidence/degenerate decodes).
        params.set_no_context(true);
        params.set_no_speech_thold(0.6);
        params.set_logprob_thold(-1.0);
        params.set_entropy_thold(2.4);
        params.set_suppress_blank(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state.full(params, &audio).map_err(|e| format!("whisper transcribe: {e}"))?;

        let n = state.full_n_segments();
        let mut out = Vec::new();
        for i in 0..n {
            let Some(seg) = state.get_segment(i) else { continue };
            let text = seg.to_str().map_err(|e| format!("segment utf8: {e}"))?.trim().to_string();
            if text.is_empty() {
                continue;
            }
            out.push(Segment {
                start: seg.start_timestamp() as f64 / 100.0, // centiseconds → seconds
                end: seg.end_timestamp() as f64 / 100.0,
                text,
            });
        }
        Ok(out)
    }
}

/// Blank / `"auto"` → `None` (auto-detect); else the lowercased ISO code.
fn normalize_lang(language: Option<String>) -> Option<String> {
    language
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty() && s != "auto")
}

/// `CAPTURE_ASR_THREADS` else min(8, available parallelism).
fn default_threads() -> i32 {
    if let Ok(Ok(n)) = std::env::var("CAPTURE_ASR_THREADS").map(|s| s.parse::<i32>()) {
        if n > 0 {
            return n;
        }
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8) as i32
}

// ── C ABI ───────────────────────────────────────────────────────────────────────────────────────

/// The engine ABI version this dylib implements.
#[no_mangle]
pub extern "C" fn capture_asr_engine_abi_version() -> u32 {
    ABI_VERSION
}

/// A static engine id for diagnostics / the runtime registry.
#[no_mangle]
pub extern "C" fn capture_asr_engine_name() -> *const c_char {
    c"whisper.cpp".as_ptr()
}

/// The thread-local message for the most recent NULL return (copy it immediately).
#[no_mangle]
pub extern "C" fn capture_asr_engine_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ref().map(|s| s.as_ptr()).unwrap_or(ptr::null()))
}

/// Load a model → an opaque handle (NULL on failure; see `last_error`).
///
/// # Safety
/// `model_ref` must be a valid NUL-terminated C string; `language` may be NULL.
#[no_mangle]
pub unsafe extern "C" fn capture_asr_engine_load(
    model_ref: *const c_char,
    language: *const c_char,
) -> *mut c_void {
    let Some(model) = cstr(model_ref) else {
        set_error("model_ref is null or invalid utf-8");
        return ptr::null_mut();
    };
    match WhisperEngine::load(&model, cstr(language)) {
        Ok(engine) => Box::into_raw(Box::new(engine)) as *mut c_void,
        Err(e) => {
            set_error(e);
            ptr::null_mut()
        }
    }
}

/// Transcribe one chunk → a malloc'd JSON array of `{start,end,text}` (free via `free_string`),
/// or NULL on error (see `last_error`).
///
/// # Safety
/// `handle` must be a live handle from `load`; `pcm` must point to `n` valid floats.
#[no_mangle]
pub unsafe extern "C" fn capture_asr_engine_transcribe(
    handle: *mut c_void,
    pcm: *const c_float,
    n: usize,
    sample_rate: u32,
) -> *mut c_char {
    if handle.is_null() || (pcm.is_null() && n != 0) {
        set_error("null handle or pcm");
        return ptr::null_mut();
    }
    let engine = &*(handle as *const WhisperEngine);
    let samples = if n == 0 { &[][..] } else { std::slice::from_raw_parts(pcm, n) };
    match engine.transcribe(samples, sample_rate) {
        Ok(segs) => match serde_json::to_string(&segs) {
            Ok(json) => CString::new(json).map(CString::into_raw).unwrap_or(ptr::null_mut()),
            Err(e) => {
                set_error(format!("serialize segments: {e}"));
                ptr::null_mut()
            }
        },
        Err(e) => {
            set_error(e);
            ptr::null_mut()
        }
    }
}

/// Change the pinned language for subsequent chunks (live, no reload). NULL/`""`/`"auto"` = auto.
///
/// # Safety
/// `handle` must be a live handle; `language` may be NULL.
#[no_mangle]
pub unsafe extern "C" fn capture_asr_engine_set_language(
    handle: *mut c_void,
    language: *const c_char,
) {
    if handle.is_null() {
        return;
    }
    let engine = &*(handle as *const WhisperEngine);
    *engine.language.lock().unwrap() = normalize_lang(cstr(language));
}

/// Free a string returned by `transcribe`.
///
/// # Safety
/// `s` must be a pointer returned by `capture_asr_engine_transcribe` (or NULL).
#[no_mangle]
pub unsafe extern "C" fn capture_asr_engine_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// Free an engine handle from `load`.
///
/// # Safety
/// `handle` must be a pointer returned by `capture_asr_engine_load` (or NULL), freed once.
#[no_mangle]
pub unsafe extern "C" fn capture_asr_engine_free(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle as *mut WhisperEngine));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lang_maps_blank_and_auto_to_none() {
        assert_eq!(normalize_lang(None), None);
        assert_eq!(normalize_lang(Some("".into())), None);
        assert_eq!(normalize_lang(Some("  ".into())), None);
        assert_eq!(normalize_lang(Some("auto".into())), None);
        assert_eq!(normalize_lang(Some("RU".into())), Some("ru".into()));
        assert_eq!(normalize_lang(Some(" En ".into())), Some("en".into()));
    }

    #[test]
    fn default_threads_is_positive() {
        assert!(default_threads() >= 1);
    }
}
