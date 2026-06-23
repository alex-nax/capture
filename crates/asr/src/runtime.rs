//! [`AsrRuntimeManager`] — the abstraction that manages ASR runtimes (the v3 unification of the
//! Python `core/asr/runtimes.py` + `manager.py`). A **runtime** is a selectable engine: a local
//! whisper.cpp engine (a dlopen'd cdylib, the default), `remote` (in-process HTTP), and — later —
//! mlx and Windows engines as more registry entries. The manager lists runtimes with availability,
//! persists the active choice to `~/.capture/config.json`, and loads the active runtime's
//! [`AsrBackend`] through the [`DynamicEngine`] seam.
//!
//! Models are per-engine: the whisper runtime uses **GGML `.bin` files** under the models dir
//! (`CAPTURE_ASR_MODELS_DIR` else `~/.capture/models`). The catalog + settings live in
//! [`crate::models`]; this manager binds them to the models dir (downloaded flags, the catalog
//! status payload, delete) and loads the active runtime's [`AsrBackend`]. The remote backend impl is
//! the next #64 piece.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};

use crate::backend::AsrBackend;
use crate::config;
use crate::dynamic::{engine_dylib_filename, DynamicEngine};
use crate::models;

/// The default GGML model id when none is configured (re-exported from [`crate::models`]).
pub use crate::models::DEFAULT_MODEL;

/// One runtime's status, as surfaced to the GUI / `GET /v1/asr/runtimes` (mirrors the Python
/// `status_payload` entry + the `v1::AsrRuntime` wire shape).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AsrRuntimeInfo {
    pub id: String,
    pub label: String,
    pub kind: String, // "local" | "remote"
    pub engine: String,
    pub device: Option<String>,
    pub requires: String,
    pub installed: bool, // local: the engine dylib is present; remote: always true
    pub available: bool, // installed + a usable model / a configured endpoint
    pub active: bool,
}

/// A static registry entry.
struct RuntimeDef {
    id: &'static str,
    label: &'static str,
    kind: &'static str,
    engine: &'static str,
    device: Option<&'static str>,
    requires: &'static str,
    /// For a local engine, the cdylib stem (`"whisper"` → `libcapture_asr_whisper.dylib`); `None` = remote.
    engine_stem: Option<&'static str>,
}

/// The platform runtime registry. macOS gets `whisper-metal`; Windows gets `whisper-cuda` (NVIDIA) +
/// `whisper-cpu`; other targets `whisper-cpu`; `remote` everywhere. (Future: `mlx` on macOS, AMD/Intel
/// via whisper.cpp-Vulkan / ONNX-DirectML — new entries, no mechanism change.)
fn registry() -> Vec<RuntimeDef> {
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        v.push(RuntimeDef {
            id: "whisper-metal",
            label: "Whisper (Metal GPU)",
            kind: "local",
            engine: "whisper.cpp",
            device: Some("metal"),
            requires: "Apple Silicon — runs on the Metal GPU.",
            engine_stem: Some("whisper"),
        });
    } else {
        // Windows lists the NVIDIA/CUDA runtime first (preferred on a GPU box; the GUI shows the
        // detected-NVIDIA hint). Both whisper runtimes share the `whisper` engine stem but install into
        // their own pack dir (`runtimes/<id>/`), so the CPU and CUDA DLLs never collide.
        if cfg!(target_os = "windows") {
            v.push(RuntimeDef {
                id: "whisper-cuda",
                label: "Whisper (NVIDIA CUDA GPU)",
                kind: "local",
                engine: "whisper.cpp",
                device: Some("cuda"),
                requires: "NVIDIA GPU + driver — runs on the GPU. Downloads the CUDA build (larger).",
                engine_stem: Some("whisper"),
            });
        }
        v.push(RuntimeDef {
            id: "whisper-cpu",
            label: "Whisper (CPU)",
            kind: "local",
            engine: "whisper.cpp",
            device: Some("cpu"),
            requires: "Any CPU. (GPU runtimes land per-platform.)",
            engine_stem: Some("whisper"),
        });
    }
    v.push(RuntimeDef {
        id: "remote",
        label: "Remote (OpenAI-compatible / Riva)",
        kind: "remote",
        engine: "openai-compat",
        device: None,
        requires: "A reachable endpoint; no local install. Set the URL in Settings.",
        engine_stem: None,
    });
    v
}

/// Manages the ASR runtimes: list / select / load. Cheap to construct.
pub struct AsrRuntimeManager {
    /// Dev/legacy engine location: a `cargo run` target dir, or a build that still bundles the engine
    /// beside the daemon. The fallback when a runtime's pack isn't installed.
    engine_dir: PathBuf,
    models_dir: PathBuf,
    /// Where downloaded runtime PACKS live: `runtimes_dir/<runtime-id>/<engine cdylib>` (#81). The app
    /// ships engine-less; a pack is downloaded here.
    runtimes_dir: PathBuf,
}

impl Default for AsrRuntimeManager {
    fn default() -> Self {
        AsrRuntimeManager::new()
    }
}

impl AsrRuntimeManager {
    /// Resolve the engine + models + runtime-pack dirs from env / install layout.
    pub fn new() -> Self {
        AsrRuntimeManager {
            engine_dir: engine_dir(),
            models_dir: models_dir(),
            runtimes_dir: runtimes_dir(),
        }
    }

    /// Explicit engine + models dirs (tests). The pack dir defaults to `engine_dir`, so a test that
    /// drops a dylib straight into `engine_dir` resolves via the legacy fallback unchanged.
    pub fn with_dirs(engine_dir: PathBuf, models_dir: PathBuf) -> Self {
        AsrRuntimeManager { runtimes_dir: engine_dir.clone(), engine_dir, models_dir }
    }

    /// Explicit engine + models + runtime-pack dirs (tests of the pack seam).
    pub fn with_dirs_full(engine_dir: PathBuf, models_dir: PathBuf, runtimes_dir: PathBuf) -> Self {
        AsrRuntimeManager { engine_dir, models_dir, runtimes_dir }
    }

    /// A runtime's install (pack) directory: `runtimes_dir/<id>/`.
    pub fn pack_dir(&self, id: &str) -> PathBuf {
        self.runtimes_dir.join(id)
    }

    /// The on-disk path a runtime's engine cdylib installs to (`runtimes_dir/<id>/lib…dylib`) — the
    /// download target for #81's pack install. `None` for remote / an unknown id (no local engine).
    pub fn pack_install_path(&self, id: &str) -> Option<PathBuf> {
        registry()
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.engine_stem.map(|stem| self.pack_dir(id).join(engine_dylib_filename(stem))))
    }

    /// Record the installed pack's version (the release tag) in a `.version` sidecar — read by the
    /// pack auto-update check.
    pub fn write_pack_version(&self, id: &str, version: &str) {
        let dir = self.pack_dir(id);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(".version"), version);
    }

    /// The installed pack's recorded version, if any.
    pub fn pack_version(&self, id: &str) -> Option<String> {
        std::fs::read_to_string(self.pack_dir(id).join(".version"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// The RAW configured active runtime id, or `None` (nothing chosen yet) — what `set_active`
    /// persisted. [`Self::resolved_active_id`] applies the runnable-fallback used for display + capture.
    pub fn active_id(&self) -> Option<String> {
        config::get_str(config::ASR_RUNTIME)
    }

    /// Whether a registry runtime's local engine cdylib is present (remote runtimes → false).
    fn local_engine_present(&self, r: &RuntimeDef) -> bool {
        r.engine_stem.map(|stem| self.engine_dylib(r.id, stem).is_file()).unwrap_or(false)
    }

    /// The runtime that will ACTUALLY run, resolving the persisted choice against what's runnable
    /// here: keep the configured runtime if it can run (a local engine that's installed, or a remote
    /// with a configured endpoint), else fall back to the first available LOCAL engine. So a fresh
    /// install (nothing chosen) — or a v2 config still defaulting to an unconfigured `remote` —
    /// transcribes on the local whisper engine instead of silently producing nothing. Drives both the
    /// `active` flag the GUI shows and the engine `backend()` loads. `None` only if nothing is runnable.
    pub fn resolved_active_id(&self) -> Option<String> {
        let reg = registry();
        let default_local = || reg.iter().find(|r| self.local_engine_present(r)).map(|r| r.id.to_string());
        match config::get_str(config::ASR_RUNTIME) {
            Some(id) => {
                let runnable = reg
                    .iter()
                    .find(|r| r.id == id)
                    .map(|r| if r.engine_stem.is_some() { self.local_engine_present(r) } else { remote_configured() })
                    .unwrap_or(false);
                if runnable { Some(id) } else { default_local().or(Some(id)) }
            }
            None => default_local(),
        }
    }

    /// Choose the active runtime (validated against the registry); persists to the config file.
    pub fn set_active(&self, id: &str) -> Result<(), String> {
        if !registry().iter().any(|r| r.id == id) {
            return Err(format!("unknown runtime {id:?}"));
        }
        config::set_str(config::ASR_RUNTIME, id)
    }

    /// The active GGML model id (validated against the catalog; a stale cross-engine repo or unknown
    /// id falls back to the default). Delegates to [`models::active_model`].
    pub fn active_model(&self) -> String {
        models::active_model()
    }

    /// The on-disk path for a GGML model id (`base.en` → `<models>/ggml-base.en.bin`).
    pub fn model_path(&self, model_id: &str) -> PathBuf {
        self.models_dir.join(models::model_filename(model_id))
    }

    /// Whether a model id's GGML file is present (and non-empty) in the models dir.
    pub fn is_downloaded(&self, model_id: &str) -> bool {
        self.model_path(model_id).metadata().map(|m| m.len() > 0).unwrap_or(false)
    }

    /// The resolved path to a runtime's engine cdylib: its installed PACK first
    /// (`runtimes_dir/<id>/lib…dylib`), else the dev/legacy `engine_dir` (a `cargo run` target dir or
    /// a build that still ships the engine beside the daemon). The first that EXISTS wins; if neither
    /// does, the pack path is returned (the install target the onboarding guides the user to).
    fn engine_dylib(&self, id: &str, stem: &str) -> PathBuf {
        let filename = engine_dylib_filename(stem);
        let pack = self.runtimes_dir.join(id).join(&filename);
        if pack.is_file() {
            return pack;
        }
        let legacy = self.engine_dir.join(&filename);
        if legacy.is_file() {
            return legacy;
        }
        pack
    }

    /// Whether a local whisper engine cdylib is present (the v3 analog of "an ASR runtime can run
    /// here"). Drives `catalog_status`'s `backend_available` + the GUI's model-picker gate.
    pub fn whisper_engine_installed(&self) -> bool {
        registry()
            .iter()
            .any(|r| r.engine_stem == Some("whisper") && self.engine_dylib(r.id, "whisper").is_file())
    }

    /// The runtime list with per-runtime installed/available/active flags (the `status_payload`).
    pub fn runtimes(&self) -> Vec<AsrRuntimeInfo> {
        let active = self.resolved_active_id();
        let model = self.active_model();
        registry()
            .into_iter()
            .map(|r| {
                let installed = match r.engine_stem {
                    Some(stem) => self.engine_dylib(r.id, stem).is_file(),
                    None => true, // remote needs no install
                };
                let available = if r.kind == "remote" {
                    remote_configured()
                } else {
                    installed && self.model_path(&model).is_file()
                };
                AsrRuntimeInfo {
                    id: r.id.into(),
                    label: r.label.into(),
                    kind: r.kind.into(),
                    engine: r.engine.into(),
                    device: r.device.map(String::from),
                    requires: r.requires.into(),
                    installed,
                    available,
                    active: active.as_deref() == Some(r.id),
                }
            })
            .collect()
    }

    /// The `GET /v1/asr/runtimes` payload: `{active, gpu:{nvidia}, runtimes:[…]}` (mirrors
    /// `runtimes.status_payload`). The per-runtime entries carry the extra `available` flag too.
    pub fn runtimes_payload(&self) -> Value {
        json!({
            "active": self.resolved_active_id(),
            "gpu": { "nvidia": gpu_nvidia() },
            "runtimes": self.runtimes(),
        })
    }

    /// The `GET /v1/asr/backend` report: the active runtime/engine/device, whether it can run, and the
    /// last load error (so the GUI shows *why* ASR is off — never a silent fallback). Mirrors
    /// `runtimes.backend_report`. v3 doesn't track a per-load error yet, so `error` is null.
    pub fn backend_report(&self) -> Value {
        let active = self.resolved_active_id();
        let entry = active.as_deref().and_then(|id| self.runtimes().into_iter().find(|r| r.id == id));
        json!({
            "runtime": active,
            "engine": entry.as_ref().map(|r| r.engine.clone()),
            "device": entry.as_ref().and_then(|r| r.device.clone()),
            "available": entry.as_ref().map(|r| r.available).unwrap_or(false),
            "error": Value::Null,
        })
    }

    /// The `GET /v1/asr/models` payload: the catalog with downloaded/active/downloading flags, plus
    /// the language + chunk settings. `downloading` is the daemon's in-flight set (so a fresh poll
    /// reflects an in-progress fetch). Mirrors `manager.catalog_status`.
    pub fn catalog_status(&self, downloading: &HashSet<String>) -> Value {
        let active = self.active_model();
        let models: Vec<Value> = models::catalog()
            .iter()
            .map(|m| {
                json!({
                    "repo": m.id,
                    "name": m.name,
                    "size_label": m.size_label,
                    "downloaded": self.is_downloaded(m.id),
                    "active": m.id == active,
                    "downloading": downloading.contains(m.id),
                })
            })
            .collect();
        json!({
            "backend_available": self.whisper_engine_installed(),
            "active": active,
            "language": models::active_language(),
            "chunk_seconds": models::active_chunk_seconds(),
            "models": models,
        })
    }

    /// Remove a downloaded model's GGML file. Returns `{repo, deleted, freed_bytes}`. Errors if the id
    /// isn't in the catalog (no arbitrary path deletes). Mirrors `manager.delete`.
    pub fn delete_model(&self, model_id: &str) -> Result<Value, String> {
        if !models::is_known(model_id) {
            return Err(format!("unknown model {model_id:?}"));
        }
        let path = self.model_path(model_id);
        let freed = path.metadata().map(|m| m.len()).unwrap_or(0);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
        }
        Ok(json!({ "repo": model_id, "deleted": true, "freed_bytes": freed }))
    }

    /// Build the active runtime's [`AsrBackend`]. Errors clearly when no runtime is selected, the
    /// engine dylib is missing, or the model isn't downloaded.
    pub fn backend(&self) -> Result<Box<dyn AsrBackend>, String> {
        let id = self.resolved_active_id().ok_or("no ASR runtime selected (choose one in Settings)")?;
        let def = registry()
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("active runtime {id:?} is not in the registry"))?;
        let language = config::get_str(config::WHISPER_LANGUAGE);
        match def.engine_stem {
            Some(stem) => {
                let dylib = self.engine_dylib(&id, stem);
                if !dylib.is_file() {
                    return Err(format!(
                        "the {} engine isn't installed ({})",
                        def.engine,
                        dylib.display()
                    ));
                }
                let model_id = self.active_model();
                let model = self.model_path(&model_id);
                if !model.is_file() {
                    return Err(format!(
                        "model {model_id:?} isn't downloaded — pick/download one in Settings"
                    ));
                }
                let engine =
                    DynamicEngine::load(&dylib, &model.to_string_lossy(), language.as_deref())?;
                Ok(Box::new(engine))
            }
            // The in-process remote backend (openai-compat/riva) lands in the next #64 piece.
            None => Err("the remote ASR backend isn't implemented yet (#64 piece 3)".to_string()),
        }
    }
}

fn home() -> PathBuf {
    // Windows: prefer %USERPROFILE% to match the GUI/agent's dirs::home_dir(). $HOME is unset when the
    // app is launched outside a shell (Explorer/Start Menu/tray), and the old `.` fallback then wrote
    // ~/.capture (models + runtime packs) into the cwd. See daemon::home for the full story.
    #[cfg(windows)]
    if let Some(p) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Where the engine cdylibs live: `CAPTURE_ASR_ENGINE_DIR`, else next to the running executable
/// (where the daemon ships them), else `.`.
fn engine_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CAPTURE_ASR_ENGINE_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(d);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.to_path_buf();
        }
    }
    PathBuf::from(".")
}

/// Where GGML models live: `CAPTURE_ASR_MODELS_DIR` else `~/.capture/models`.
fn models_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CAPTURE_ASR_MODELS_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(d);
    }
    home().join(".capture").join("models")
}

/// Where downloaded runtime PACKS live: `CAPTURE_ASR_RUNTIMES_DIR` else `~/.capture/runtimes` (#81).
/// Each runtime's engine cdylib (+ any deps) is installed under `<this>/<runtime-id>/`.
fn runtimes_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CAPTURE_ASR_RUNTIMES_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(d);
    }
    home().join(".capture").join("runtimes")
}

/// Whether a remote ASR endpoint is configured (the v2 `CAPTURE_OPENAI_ASR_URL`).
fn remote_configured() -> bool {
    std::env::var("CAPTURE_OPENAI_ASR_URL").map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Cheap GPU hint for `GET /v1/asr/runtimes` (no engine needed): is an NVIDIA GPU likely present?
/// Always false on macOS (Metal, not CUDA); on other platforms, `nvidia-smi` on PATH. Mirrors
/// `runtimes._detect_nvidia`.
fn gpu_nvidia() -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    let exe = if cfg!(windows) { "nvidia-smi.exe" } else { "nvidia-smi" };
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p.join(exe).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testlock;

    fn tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("capture-asr-rt-{tag}-{nanos}"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn registry_has_remote_and_a_whisper_runtime() {
        let _g = testlock::guard();
        let cfg = tmp("reg").join("config.json");
        std::env::set_var("CAPTURE_CONFIG", &cfg);
        let m = AsrRuntimeManager::with_dirs(tmp("reg-eng"), tmp("reg-mod"));
        let rs = m.runtimes();
        assert!(rs.iter().any(|r| r.id == "remote" && r.kind == "remote"));
        assert!(rs.iter().any(|r| r.engine == "whisper.cpp" && r.kind == "local"));
        std::env::remove_var("CAPTURE_CONFIG");
    }

    #[test]
    fn set_active_validates_and_persists() {
        let _g = testlock::guard();
        let cfg = tmp("active").join("config.json");
        std::env::set_var("CAPTURE_CONFIG", &cfg);
        let m = AsrRuntimeManager::with_dirs(tmp("a-eng"), tmp("a-mod"));
        assert!(m.set_active("bogus").is_err(), "unknown runtime rejected");
        m.set_active("remote").unwrap();
        assert_eq!(m.active_id().as_deref(), Some("remote"));
        assert!(m.runtimes().iter().find(|r| r.id == "remote").unwrap().active);
        std::env::remove_var("CAPTURE_CONFIG");
    }

    #[test]
    fn active_model_ignores_stale_hf_repo() {
        let _g = testlock::guard();
        let cfg = tmp("model").join("config.json");
        std::env::set_var("CAPTURE_CONFIG", &cfg);
        let m = AsrRuntimeManager::with_dirs(tmp("m-eng"), tmp("m-mod"));
        // A stale v2 mlx repo (has a slash) → ignored in favour of the default GGML id.
        config::set_str(config::WHISPER_MODEL, "mlx-community/whisper-large-v3-turbo").unwrap();
        assert_eq!(m.active_model(), DEFAULT_MODEL);
        // A bare GGML id is honoured.
        config::set_str(config::WHISPER_MODEL, "small.en").unwrap();
        assert_eq!(m.active_model(), "small.en");
        std::env::remove_var("CAPTURE_CONFIG");
    }

    #[test]
    fn whisper_available_only_with_engine_and_model() {
        let _g = testlock::guard();
        let cfg = tmp("avail").join("config.json");
        std::env::set_var("CAPTURE_CONFIG", &cfg);
        std::env::remove_var("CAPTURE_OPENAI_ASR_URL");
        let eng = tmp("av-eng");
        let mod_ = tmp("av-mod");
        let m = AsrRuntimeManager::with_dirs(eng.clone(), mod_.clone());

        let whisper = |m: &AsrRuntimeManager| {
            m.runtimes().into_iter().find(|r| r.engine == "whisper.cpp").unwrap()
        };
        // Nothing present → installed false, available false.
        assert!(!whisper(&m).installed);
        assert!(!whisper(&m).available);
        // Engine dylib present but no model → installed, not available.
        std::fs::write(eng.join(engine_dylib_filename("whisper")), b"x").unwrap();
        assert!(whisper(&m).installed);
        assert!(!whisper(&m).available);
        // Plus the default model → available.
        std::fs::write(m.model_path(DEFAULT_MODEL), b"ggml").unwrap();
        assert!(whisper(&m).available);

        // remote: available only when an endpoint is configured.
        assert!(!m.runtimes().into_iter().find(|r| r.id == "remote").unwrap().available);
        std::env::set_var("CAPTURE_OPENAI_ASR_URL", "http://localhost:9000/v1/audio/transcriptions");
        assert!(m.runtimes().into_iter().find(|r| r.id == "remote").unwrap().available);

        std::env::remove_var("CAPTURE_OPENAI_ASR_URL");
        std::env::remove_var("CAPTURE_CONFIG");
    }

    #[test]
    fn engine_resolves_from_installed_pack_dir() {
        // #81: with the engine UNBUNDLED, a runtime's cdylib is found in its installed pack dir
        // (runtimes_dir/<id>/), not beside the daemon. engine_dir here is empty (no legacy fallback).
        let _g = testlock::guard();
        let m = AsrRuntimeManager::with_dirs_full(tmp("pk-eng"), tmp("pk-mod"), tmp("pk-rt"));
        let wid = if cfg!(target_os = "macos") { "whisper-metal" } else { "whisper-cpu" };
        let whisper = |m: &AsrRuntimeManager| m.runtimes().into_iter().find(|r| r.id == wid).unwrap();
        // Nothing installed anywhere → not installed.
        assert!(!whisper(&m).installed);
        assert!(!m.whisper_engine_installed());
        // "Install" a pack: drop the cdylib into runtimes_dir/<id>/.
        let packdir = m.pack_dir(wid);
        std::fs::create_dir_all(&packdir).unwrap();
        std::fs::write(packdir.join(engine_dylib_filename("whisper")), b"x").unwrap();
        // Now it resolves from the pack dir.
        assert!(whisper(&m).installed);
        assert!(m.whisper_engine_installed());
    }

    #[test]
    fn backend_errors_are_clear() {
        let _g = testlock::guard();
        let cfg = tmp("be").join("config.json");
        std::env::set_var("CAPTURE_CONFIG", &cfg);
        let m = AsrRuntimeManager::with_dirs(tmp("be-eng"), tmp("be-mod"));
        // No runtime selected.
        assert!(m.backend().err().unwrap().contains("no ASR runtime selected"));
        // whisper selected but engine not installed.
        m.set_active(if cfg!(target_os = "macos") { "whisper-metal" } else { "whisper-cpu" }).unwrap();
        assert!(m.backend().err().unwrap().contains("isn't installed"));
        // remote selected → not-implemented (piece 3).
        m.set_active("remote").unwrap();
        assert!(m.backend().err().unwrap().contains("remote ASR backend isn't implemented"));
        std::env::remove_var("CAPTURE_CONFIG");
    }
}
