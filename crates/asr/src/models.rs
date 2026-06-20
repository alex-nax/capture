//! The GGML whisper model catalog + settings (a port of `core/asr/manager.py`, adapted to
//! whisper.cpp). whisper.cpp loads **GGML `.bin` files** — one file per model — so the catalog is a
//! list of model **ids** (`base.en`, `large-v3-turbo`, …), each downloaded on demand as
//! `ggml-<id>.bin` from the `ggerganov/whisper.cpp` HF repo. This is a much simpler shape than the
//! Python's HF-snapshot repos: no `huggingface_hub`, no multi-file blobs, just a single streamed GET
//! (done in the daemon, so the engine cdylib that links this crate stays HTTP-free).
//!
//! This module owns the **catalog + validation + the persisted settings** (active model id, language,
//! chunk length); the filesystem-touching parts (`is_downloaded`, `catalog_status`, `delete`, the
//! download itself) live on [`crate::AsrRuntimeManager`] / the daemon, which know the models dir.

use crate::config;

/// The default GGML model id when none is configured (small, English, fast; the catalog adds the rest).
/// Kept in sync with [`crate::runtime::DEFAULT_MODEL`].
pub const DEFAULT_MODEL: &str = "base.en";

/// Default transcription chunk length. Whisper is trained on 30 s windows; shorter chunks make it
/// hallucinate phantom phrases ("Thank you.") on pauses / non-English audio, so 30 s is the reliable
/// default (the v2 `manager.DEFAULT_CHUNK_SECONDS`). Tunable via [`set_chunk_seconds`].
pub const DEFAULT_CHUNK_SECONDS: f64 = 30.0;
/// The accepted chunk-length range, in seconds (inclusive).
pub const CHUNK_BOUNDS: (f64, f64) = (1.0, 120.0);

/// One catalog entry: a whisper.cpp GGML model id + a human label + an approximate download size.
pub struct CatalogEntry {
    /// The GGML model id (`ggml-<id>.bin`); also the wire `repo` value and the `whisper_model` setting.
    pub id: &'static str,
    pub name: &'static str,
    pub size_label: &'static str,
}

/// The curated GGML catalog offered in the GUI, ordered by download size. Ids VERIFIED against the
/// `ggerganov/whisper.cpp` HF file list (`ggml-<id>.bin`). English-only (`.en`) models are smaller +
/// faster but transcribe English only; the `large-v3*` models are multilingual.
const CATALOG: &[CatalogEntry] = &[
    CatalogEntry { id: "tiny.en", name: "Whisper Tiny (English)", size_label: "~75 MB" },
    CatalogEntry { id: "base.en", name: "Whisper Base (English)", size_label: "~142 MB" },
    CatalogEntry { id: "small.en", name: "Whisper Small (English)", size_label: "~466 MB" },
    CatalogEntry {
        id: "large-v3-turbo-q5_0",
        name: "Whisper Large v3 Turbo (quantized)",
        size_label: "~574 MB",
    },
    CatalogEntry {
        id: "large-v3-turbo",
        name: "Whisper Large v3 Turbo (multilingual)",
        size_label: "~1.6 GB",
    },
    CatalogEntry { id: "large-v3", name: "Whisper Large v3 (multilingual)", size_label: "~3.1 GB" },
];

/// The catalog entries (ordered by size).
pub fn catalog() -> &'static [CatalogEntry] {
    CATALOG
}

/// Whether `id` is a model the catalog offers (so we never fetch/select an arbitrary file).
pub fn is_known(id: &str) -> bool {
    CATALOG.iter().any(|m| m.id == id)
}

/// The on-disk filename for a GGML model id (`base.en` → `ggml-base.en.bin`).
pub fn model_filename(id: &str) -> String {
    format!("ggml-{id}.bin")
}

/// The HF base URL the GGML `.bin` files are fetched from: `CAPTURE_GGML_BASE_URL` (test/mirror),
/// else the canonical `ggerganov/whisper.cpp` resolve root.
pub fn base_url() -> String {
    match std::env::var("CAPTURE_GGML_BASE_URL") {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_end_matches('/').to_string(),
        _ => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main".to_string(),
    }
}

/// The download URL for a GGML model id (`base.en` → `<base>/ggml-base.en.bin`).
pub fn model_url(id: &str) -> String {
    format!("{}/{}", base_url(), model_filename(id))
}

/// The configured active model id, validated against the catalog (config → default). A stale
/// cross-engine value (e.g. an mlx `mlx-community/...` repo that whisper.cpp can't load) falls back to
/// [`DEFAULT_MODEL`], so the backend never gets a model it can't open. Mirrors `manager.active_model`.
pub fn active_model() -> String {
    config::get_str(config::WHISPER_MODEL)
        .filter(|m| is_known(m))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

/// Persist `id` as the active model. Errors if it isn't in the catalog (no arbitrary models).
pub fn set_active_model(id: &str) -> Result<String, String> {
    if !is_known(id) {
        let known: Vec<&str> = CATALOG.iter().map(|m| m.id).collect();
        return Err(format!("unknown model {id:?}; choose from {known:?}"));
    }
    config::set_str(config::WHISPER_MODEL, id)?;
    Ok(id.to_string())
}

/// The configured transcription language (ISO code like `ru`/`en`), or `None` for auto-detect.
pub fn active_language() -> Option<String> {
    config::get_str(config::WHISPER_LANGUAGE)
}

/// Persist the transcription language. `None`/`""`/`"auto"` = auto-detect. Accepts a short ISO-639
/// code (loosely validated: 2–5 letters). Mirrors `manager.set_active_language`.
pub fn set_active_language(language: Option<&str>) -> Result<Option<String>, String> {
    let lang = language.unwrap_or("").trim().to_lowercase();
    if lang.is_empty() || lang == "auto" {
        config::set_str(config::WHISPER_LANGUAGE, "")?;
        return Ok(None);
    }
    if !(2..=5).contains(&lang.len()) || !lang.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!(
            "invalid language {language:?}; use an ISO code like 'ru', 'en' (or 'auto')"
        ));
    }
    config::set_str(config::WHISPER_LANGUAGE, &lang)?;
    Ok(Some(lang))
}

/// The configured transcription chunk length (seconds), clamped to [`CHUNK_BOUNDS`]; default 30 s.
pub fn active_chunk_seconds() -> f64 {
    match config::get_f64(config::AUDIO_CHUNK_SECONDS) {
        Some(s) => clamp_chunk(s),
        None => DEFAULT_CHUNK_SECONDS,
    }
}

/// Persist the transcription chunk length (clamped to [`CHUNK_BOUNDS`]).
pub fn set_chunk_seconds(seconds: f64) -> Result<f64, String> {
    if !seconds.is_finite() {
        return Err(format!("invalid chunk length {seconds:?}; give seconds (e.g. 30)"));
    }
    let secs = clamp_chunk(seconds);
    config::set_f64(config::AUDIO_CHUNK_SECONDS, secs)?;
    Ok(secs)
}

fn clamp_chunk(secs: f64) -> f64 {
    let (lo, hi) = CHUNK_BOUNDS;
    secs.clamp(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testlock;
    use std::path::PathBuf;

    fn tmp_cfg(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("capture-asr-models-{tag}-{nanos}"));
        std::fs::create_dir_all(&d).unwrap();
        d.join("config.json")
    }

    #[test]
    fn catalog_default_is_present_and_urls_compose() {
        assert!(is_known(DEFAULT_MODEL), "the default must be offered in the catalog");
        assert_eq!(model_filename("base.en"), "ggml-base.en.bin");
        assert!(model_url("base.en").ends_with("/ggml-base.en.bin"));
        assert!(!is_known("mlx-community/whisper-large-v3-turbo"));
    }

    #[test]
    fn base_url_env_override() {
        let _g = testlock::guard();
        std::env::set_var("CAPTURE_GGML_BASE_URL", "http://127.0.0.1:9/m/");
        assert_eq!(model_url("tiny.en"), "http://127.0.0.1:9/m/ggml-tiny.en.bin");
        std::env::remove_var("CAPTURE_GGML_BASE_URL");
    }

    #[test]
    fn set_active_model_validates_and_persists() {
        let _g = testlock::guard();
        std::env::set_var("CAPTURE_CONFIG", tmp_cfg("model"));
        assert!(set_active_model("bogus").is_err());
        assert_eq!(set_active_model("small.en").unwrap(), "small.en");
        assert_eq!(active_model(), "small.en");
        std::env::remove_var("CAPTURE_CONFIG");
    }

    #[test]
    fn language_round_trips_and_validates() {
        let _g = testlock::guard();
        std::env::set_var("CAPTURE_CONFIG", tmp_cfg("lang"));
        assert_eq!(set_active_language(Some("RU")).unwrap().as_deref(), Some("ru"));
        assert_eq!(active_language().as_deref(), Some("ru"));
        assert_eq!(set_active_language(Some("auto")).unwrap(), None);
        assert_eq!(active_language(), None);
        assert!(set_active_language(Some("123")).is_err());
        std::env::remove_var("CAPTURE_CONFIG");
    }

    #[test]
    fn chunk_clamps_and_persists_as_number() {
        let _g = testlock::guard();
        let cfg = tmp_cfg("chunk");
        std::env::set_var("CAPTURE_CONFIG", &cfg);
        assert_eq!(active_chunk_seconds(), DEFAULT_CHUNK_SECONDS); // unset → default
        assert_eq!(set_chunk_seconds(8.0).unwrap(), 8.0);
        assert_eq!(active_chunk_seconds(), 8.0);
        assert_eq!(set_chunk_seconds(9999.0).unwrap(), CHUNK_BOUNDS.1); // clamps high
        // Persisted as a JSON number (so the Python daemon reads it back as a float).
        let text = std::fs::read_to_string(&cfg).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(v[config::AUDIO_CHUNK_SECONDS].is_number());
        std::env::remove_var("CAPTURE_CONFIG");
    }
}
