//! [`DynamicEngine`] — the loader that `dlopen`s an ASR engine cdylib (the generic
//! `capture_asr_engine_*` C ABI) and presents it as an [`AsrBackend`]. The lean daemon links no
//! engine; it loads `libcapture_asr_whisper.dylib` (or a future `…_mlx`/Windows engine) at runtime
//! when that runtime is selected. The C ABI is engine-agnostic, so one loader serves every engine.

use std::ffi::{c_char, c_float, c_void, CStr, CString};
use std::path::Path;
use std::ptr;
use std::sync::Mutex;

use libloading::{Library, Symbol};

use crate::backend::{AsrBackend, Segment};

/// The engine C ABI version this loader speaks; an engine dylib must report the same (see
/// `capture-asr-whisper`'s `capture_asr_engine_abi_version`).
pub const ENGINE_ABI_VERSION: u32 = 1;

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type NameFn = unsafe extern "C" fn() -> *const c_char;
type LastErrorFn = unsafe extern "C" fn() -> *const c_char;
type LoadFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void;
type TranscribeFn = unsafe extern "C" fn(*mut c_void, *const c_float, usize, u32) -> *mut c_char;
type SetLanguageFn = unsafe extern "C" fn(*mut c_void, *const c_char);
type FreeStringFn = unsafe extern "C" fn(*mut c_char);
type FreeFn = unsafe extern "C" fn(*mut c_void);

/// A loaded ASR engine: a `dlopen`'d cdylib + a live model handle, presented as an [`AsrBackend`].
pub struct DynamicEngine {
    handle: Mutex<*mut c_void>,
    transcribe: TranscribeFn,
    set_language: SetLanguageFn,
    free_string: FreeStringFn,
    free: FreeFn,
    last_error: LastErrorFn,
    name: String,
    // Kept loaded for the engine's lifetime; declared last so it unloads after the explicit Drop.
    _lib: Library,
}

// Safety: the engine handle is only dereferenced inside the C ABI, always under `handle`'s Mutex;
// the stored fn pointers are immutable and `_lib` stays loaded for the engine's lifetime. So a
// `DynamicEngine` is safe to share across threads (the daemon calls transcribe from a worker).
unsafe impl Send for DynamicEngine {}
unsafe impl Sync for DynamicEngine {}

impl DynamicEngine {
    /// `dlopen` the engine at `dylib`, check its ABI, and load `model_ref` (engine-interpreted — a
    /// GGML path for whisper.cpp). `language` pins the transcription language (`None` = auto). `Err`
    /// carries a clear message (missing symbol, ABI mismatch, or the engine's own load error).
    pub fn load(dylib: &Path, model_ref: &str, language: Option<&str>) -> Result<DynamicEngine, String> {
        unsafe {
            let lib = Library::new(dylib).map_err(|e| format!("dlopen {}: {e}", dylib.display()))?;

            let missing = |sym: &str, e: libloading::Error| format!("engine missing `{sym}`: {e}");
            let abi: Symbol<AbiVersionFn> = lib
                .get(b"capture_asr_engine_abi_version\0")
                .map_err(|e| missing("capture_asr_engine_abi_version", e))?;
            let got = abi();
            if got != ENGINE_ABI_VERSION {
                return Err(format!(
                    "engine ABI mismatch: dylib reports v{got}, loader expects v{ENGINE_ABI_VERSION}"
                ));
            }
            let name_fn: Symbol<NameFn> =
                lib.get(b"capture_asr_engine_name\0").map_err(|e| missing("capture_asr_engine_name", e))?;
            let name = cstr(name_fn()).unwrap_or_else(|| "unknown".to_string());

            let load_fn: Symbol<LoadFn> =
                lib.get(b"capture_asr_engine_load\0").map_err(|e| missing("capture_asr_engine_load", e))?;
            // Copy the bare fn pointers out (ending the &lib borrow) so `lib` can move into the struct.
            let transcribe = *lib
                .get::<TranscribeFn>(b"capture_asr_engine_transcribe\0")
                .map_err(|e| missing("capture_asr_engine_transcribe", e))?;
            let set_language = *lib
                .get::<SetLanguageFn>(b"capture_asr_engine_set_language\0")
                .map_err(|e| missing("capture_asr_engine_set_language", e))?;
            let free_string = *lib
                .get::<FreeStringFn>(b"capture_asr_engine_free_string\0")
                .map_err(|e| missing("capture_asr_engine_free_string", e))?;
            let free =
                *lib.get::<FreeFn>(b"capture_asr_engine_free\0").map_err(|e| missing("capture_asr_engine_free", e))?;
            let last_error = *lib
                .get::<LastErrorFn>(b"capture_asr_engine_last_error\0")
                .map_err(|e| missing("capture_asr_engine_last_error", e))?;

            let model_c = CString::new(model_ref).map_err(|_| "model_ref has an interior NUL".to_string())?;
            let lang_c = match language {
                Some(l) => Some(CString::new(l).map_err(|_| "language has an interior NUL".to_string())?),
                None => None,
            };
            let handle = load_fn(model_c.as_ptr(), lang_c.as_ref().map_or(ptr::null(), |c| c.as_ptr()));
            if handle.is_null() {
                let err = cstr(last_error()).unwrap_or_else(|| "unknown engine error".to_string());
                return Err(format!("engine load failed: {err}"));
            }

            Ok(DynamicEngine {
                handle: Mutex::new(handle),
                transcribe,
                set_language,
                free_string,
                free,
                last_error,
                name,
                _lib: lib,
            })
        }
    }

    /// The engine's self-reported id (`"whisper.cpp"`, …).
    pub fn engine_name(&self) -> &str {
        &self.name
    }

    /// Change the pinned language live (no model reload). `None`/`""`/`"auto"` = auto-detect.
    pub fn set_language(&self, language: Option<&str>) {
        let handle = *self.handle.lock().unwrap();
        let lang_c = language.and_then(|l| CString::new(l).ok());
        unsafe { (self.set_language)(handle, lang_c.as_ref().map_or(ptr::null(), |c| c.as_ptr())) };
    }
}

impl AsrBackend for DynamicEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn transcribe(&self, pcm: &[f32], sample_rate: u32) -> Result<Vec<Segment>, String> {
        let handle = *self.handle.lock().unwrap();
        let ptr = unsafe { (self.transcribe)(handle, pcm.as_ptr(), pcm.len(), sample_rate) };
        if ptr.is_null() {
            let err = unsafe { cstr((self.last_error)()) }.unwrap_or_else(|| "unknown engine error".into());
            return Err(format!("engine transcribe failed: {err}"));
        }
        let json = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        unsafe { (self.free_string)(ptr) };
        serde_json::from_str(&json).map_err(|e| format!("parse engine segments: {e}"))
    }
}

impl Drop for DynamicEngine {
    fn drop(&mut self) {
        // Free the engine handle while the library is still loaded (runs before any field drop).
        let handle = *self.handle.lock().unwrap();
        if !handle.is_null() {
            unsafe { (self.free)(handle) };
        }
    }
}

/// A *const c_char → owned String (NULL → None, invalid UTF-8 → None). Copies immediately so a
/// thread-local engine error pointer is safe to read.
unsafe fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok().map(String::from)
}

/// The platform dylib filename for an engine `stem`: `"whisper"` →
/// `libcapture_asr_whisper.dylib` (macOS) / `.so` (Linux) / `capture_asr_whisper.dll` (Windows).
pub fn engine_dylib_filename(stem: &str) -> String {
    let base = format!("capture_asr_{stem}");
    if cfg!(target_os = "macos") {
        format!("lib{base}.dylib")
    } else if cfg!(windows) {
        format!("{base}.dll")
    } else {
        format!("lib{base}.so")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_filename_per_platform() {
        let f = engine_dylib_filename("whisper");
        if cfg!(target_os = "macos") {
            assert_eq!(f, "libcapture_asr_whisper.dylib");
        } else if cfg!(windows) {
            assert_eq!(f, "capture_asr_whisper.dll");
        } else {
            assert_eq!(f, "libcapture_asr_whisper.so");
        }
    }

    #[test]
    fn loading_a_missing_dylib_errors() {
        // DynamicEngine isn't Debug (Library + fn ptrs), so match rather than unwrap_err.
        match DynamicEngine::load(Path::new("/no/such/engine.dylib"), "model", None) {
            Err(e) => assert!(e.contains("dlopen"), "got: {e}"),
            Ok(_) => panic!("expected a dlopen failure"),
        }
    }
}
