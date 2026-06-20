//! ASR runtime + model-manager handlers (capture-asr): runtimes, models, language/chunk, downloads.

use super::*;

/// `GET /v1/asr/runtimes` — the selectable ASR runtimes + per-runtime installed/available/active.
pub(crate) async fn asr_runtimes() -> Json<Value> {
    Json(AsrRuntimeManager::new().runtimes_payload())
}

/// `GET /v1/asr/backend` — the active runtime/engine/device + whether it can run (never a silent fallback).
pub(crate) async fn asr_backend() -> Json<Value> {
    Json(AsrRuntimeManager::new().backend_report())
}

#[derive(Deserialize)]
struct RuntimeIdBody {
    id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // accepted for parity (pack source); unused on macOS where the engine is bundled
    source: Option<String>,
}

/// `POST /v1/asr/runtime` — choose the active runtime `{id}` → `{active}`. 400 unknown, or a local
/// runtime whose engine isn't installed (mirror the Python guard).
pub(crate) async fn asr_set_runtime(body: Bytes) -> Result<Json<Value>, ApiError> {
    let req: RuntimeIdBody = parse_json(&body)?;
    let id = req.id.unwrap_or_default();
    let mgr = AsrRuntimeManager::new();
    if let Some(rt) = mgr.runtimes().into_iter().find(|r| r.id == id) {
        if rt.kind == "local" && !rt.installed {
            return Err(ApiError::bad(format!("runtime {id:?} is not installed")));
        }
    }
    mgr.set_active(&id).map_err(ApiError::bad)?;
    Ok(Json(json!({ "active": id })))
}

/// The GitHub repo runtime packs are released from (same repo as the app, #81).
const PACK_REPO: &str = "alex-nax/capture";

/// `POST /v1/asr/runtimes/install {id, source?}` — install a runtime engine PACK (#81). The app ships
/// engine-less; this downloads the pack into `~/.capture/runtimes/<id>/` (SSE `asr_runtime_install`
/// progress → `asr_runtime_install_done`/`_error`) and makes it active. `source` (a URL or local path)
/// overrides the default GitHub-release pack — used for dev/testing. Remote needs no engine (selecting
/// it IS the install); an already-installed local runtime just activates (no re-download).
pub(crate) async fn asr_runtime_install(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let req: RuntimeIdBody = parse_json(&body)?;
    let id = req.id.unwrap_or_default();
    let mgr = AsrRuntimeManager::new();
    let rt = mgr
        .runtimes()
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::bad(format!("unknown runtime {id:?}")))?;

    // Remote runtimes have no engine to download — selecting it is the whole "install".
    if rt.kind != "local" {
        mgr.set_active(&id).map_err(ApiError::bad)?;
        let _ = state.events.send(json!({ "type": "asr_runtime_install_done", "id": id }).to_string());
        return Ok((StatusCode::ACCEPTED, Json(json!({ "id": id, "started": false }))));
    }
    // Already installed (and not forcing a re-download via `source`) → just activate.
    if rt.installed && req.source.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_none() {
        mgr.set_active(&id).map_err(ApiError::bad)?;
        let _ = state.events.send(json!({ "type": "asr_runtime_install_done", "id": id }).to_string());
        return Ok((StatusCode::ACCEPTED, Json(json!({ "id": id, "started": false }))));
    }

    let dest = mgr
        .pack_install_path(&id)
        .ok_or_else(|| ApiError::bad(format!("runtime {id:?} has no installable engine")))?;

    // One install per runtime at a time (reuse the download guard, runtime-prefixed).
    let guard_key = format!("runtime:{id}");
    {
        let mut set = state.asr_downloading.lock().unwrap();
        if set.contains(&guard_key) {
            return Ok((StatusCode::ACCEPTED, Json(json!({ "id": id, "started": false, "reason": "already installing" }))));
        }
        set.insert(guard_key.clone());
    }
    // Resolve the source: an explicit `source` (URL or local path) wins; else the GitHub-release pack.
    let (url, version) = match req.source.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(src) => (src, None),
        None => match tokio::task::spawn_blocking({ let id = id.clone(); move || resolve_pack_url(&id) }).await.unwrap_or_else(|e| Err(e.to_string())) {
            Ok((u, v)) => (u, Some(v)),
            Err(e) => {
                state.asr_downloading.lock().unwrap().remove(&guard_key);
                return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, format!("no runtime pack available for {id:?}: {e}")));
            }
        },
    };
    let task = PackInstallTask {
        events: state.events.clone(),
        guard: state.asr_downloading.clone(),
        guard_key,
        id: id.clone(),
        url,
        dest,
        version,
    };
    tokio::task::spawn_blocking(move || run_pack_install(task));
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": id, "started": true }))))
}

/// Owned inputs for one background runtime-pack install (moved into the blocking task).
struct PackInstallTask {
    events: broadcast::Sender<String>,
    guard: Arc<Mutex<HashSet<String>>>,
    guard_key: String,
    id: String,
    url: String,
    dest: PathBuf,
    version: Option<String>,
}

/// Download the pack into its install dir, record the version, activate it, broadcasting
/// `asr_runtime_install` progress then `asr_runtime_install_done`/`_error`; clear the in-flight guard.
fn run_pack_install(t: PackInstallTask) {
    let PackInstallTask { events, guard, guard_key, id, url, dest, version } = t;
    let result = stream_to_file(&url, &dest, |done, total| {
        let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        let _ = events.send(
            json!({
                "type": "asr_runtime_install", "id": id, "downloaded": done, "total": total,
                "fraction": (frac * 10000.0).round() / 10000.0,
            })
            .to_string(),
        );
    });
    match result {
        Ok(()) => {
            let mgr = AsrRuntimeManager::new();
            if let Some(v) = &version {
                mgr.write_pack_version(&id, v);
            }
            let _ = mgr.set_active(&id);
            let _ = events.send(json!({ "type": "asr_runtime_install_done", "id": id }).to_string());
        }
        Err(e) => {
            eprintln!("captured: runtime pack install failed ({id}): {e}");
            let _ = events.send(json!({ "type": "asr_runtime_install_error", "id": id, "error": e }).to_string());
        }
    }
    guard.lock().unwrap().remove(&guard_key);
}

/// `(major, minor, patch)` from a `X.Y.Z` string (mirrors the app updater).
fn parse_pack_semver(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().trim_start_matches('v').split('.');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// This machine's pack tag, e.g. `macos-arm64` / `windows-x86_64`.
fn os_arch_tag() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether a release asset name is the runtime pack for `id` on this OS/arch. Lenient: it must mention
/// the runtime id and a matching OS + arch token and be a `.dylib`/`.so`/`.dll` or `.tar.gz`.
fn asset_matches_pack(name: &str, id: &str) -> bool {
    let n = name.to_lowercase();
    let os_ok = match std::env::consts::OS {
        "macos" => n.contains("macos") || n.contains("darwin"),
        "windows" => n.contains("windows") || n.contains("win"),
        other => n.contains(other),
    };
    let arch_ok = match std::env::consts::ARCH {
        "aarch64" => n.contains("arm64") || n.contains("aarch64"),
        "x86_64" => n.contains("x86_64") || n.contains("x64") || n.contains("amd64"),
        other => n.contains(other),
    };
    let ext_ok = n.ends_with(".dylib") || n.ends_with(".so") || n.ends_with(".dll") || n.ends_with(".tar.gz");
    n.contains(id) && os_ok && arch_ok && ext_ok
}

/// Resolve a runtime pack's download URL + version from the project's GitHub releases (#81): the newest
/// release tagged `pack-<id>-v<semver>` with an asset matching this OS/arch. Errors clearly when no pack
/// is published yet (the GUI surfaces it as "not available"). Mirrors `gui/src/update.rs` asset matching.
fn resolve_pack_url(id: &str) -> Result<(String, String), String> {
    let api = format!("https://api.github.com/repos/{PACK_REPO}/releases?per_page=100");
    let client = reqwest::blocking::Client::builder().build().map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(&api)
        .header("User-Agent", "capture-daemon")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("list releases: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("list releases: HTTP {}", resp.status().as_u16()));
    }
    let releases: Value = resp.json().map_err(|e| format!("parse releases: {e}"))?;
    let prefix = format!("pack-{id}-v");
    let mut best: Option<((u32, u32, u32), String, String)> = None;
    for rel in releases.as_array().into_iter().flatten() {
        let tag = rel.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
        let Some(ver) = tag.strip_prefix(&prefix) else { continue };
        let Some(sv) = parse_pack_semver(ver) else { continue };
        let asset_url = rel
            .get("assets")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .find_map(|a| {
                let name = a.get("name").and_then(|n| n.as_str())?;
                asset_matches_pack(name, id).then(|| a.get("browser_download_url")?.as_str().map(String::from))?
            });
        if let Some(u) = asset_url {
            if best.as_ref().map(|(b, _, _)| sv > *b).unwrap_or(true) {
                best = Some((sv, ver.to_string(), u));
            }
        }
    }
    best.map(|(_, v, u)| (u, v))
        .ok_or_else(|| format!("no `{prefix}*` release with a {} asset", os_arch_tag()))
}

/// `GET /v1/asr/models` — the GGML model catalog + downloaded/active/downloading + language/chunk.
pub(crate) async fn asr_models(State(state): State<AppState>) -> Json<Value> {
    let downloading = state.asr_downloading.lock().unwrap().clone();
    Json(AsrRuntimeManager::new().catalog_status(&downloading))
}

/// `POST /v1/asr/model` — set the active GGML model `{repo}` → `{active}`. 400 if not in the catalog.
pub(crate) async fn asr_set_model(body: Bytes) -> Result<Json<Value>, ApiError> {
    let req: v1::AsrModelRequest = parse_json(&body)?;
    let active = models::set_active_model(&req.repo).map_err(ApiError::bad)?;
    Ok(Json(json!({ "active": active })))
}

#[derive(Deserialize)]
struct LanguageBody {
    language: Option<String>,
}

/// `POST /v1/asr/language` — set the transcription language `{language}` → `{language}` (None = auto).
pub(crate) async fn asr_set_language(body: Bytes) -> Result<Json<Value>, ApiError> {
    let req: LanguageBody = parse_json(&body)?;
    let lang = models::set_active_language(req.language.as_deref()).map_err(ApiError::bad)?;
    Ok(Json(json!({ "language": lang })))
}

#[derive(Deserialize)]
struct ChunkBody {
    seconds: Option<f64>,
}

/// `POST /v1/asr/chunk` — set the transcription chunk length `{seconds}` → `{chunk_seconds}`.
pub(crate) async fn asr_set_chunk(body: Bytes) -> Result<Json<Value>, ApiError> {
    let req: ChunkBody = parse_json(&body)?;
    let secs = req
        .seconds
        .ok_or_else(|| ApiError::bad("invalid chunk length; give seconds (e.g. 30)"))?;
    let secs = models::set_chunk_seconds(secs).map_err(ApiError::bad)?;
    Ok(Json(json!({ "chunk_seconds": secs })))
}

/// `POST /v1/asr/models/delete` — remove a downloaded GGML model `{repo}`. 409 if it's downloading.
pub(crate) async fn asr_model_delete(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let req: v1::AsrModelRequest = parse_json(&body)?;
    if state.asr_downloading.lock().unwrap().contains(&req.repo) {
        return Err(ApiError::new(StatusCode::CONFLICT, "model is downloading; cannot delete"));
    }
    let out = AsrRuntimeManager::new().delete_model(&req.repo).map_err(ApiError::bad)?;
    Ok(Json(out))
}

/// `POST /v1/asr/models/download` — fetch a GGML model `{repo}` in the background (202 immediately;
/// progress over `/v1/events` as `asr_download` → `asr_download_done`/`asr_download_error`). A repo
/// already downloading is a no-op (`started:false`). Mirrors `server.start_asr_download`.
pub(crate) async fn asr_model_download(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let req: v1::AsrModelRequest = parse_json(&body)?;
    let repo = req.repo;
    if !models::is_known(&repo) {
        return Err(ApiError::bad(format!("unknown model {repo:?}")));
    }
    {
        let mut set = state.asr_downloading.lock().unwrap();
        if set.contains(&repo) {
            return Ok((
                StatusCode::ACCEPTED,
                Json(json!({ "repo": repo, "started": false, "reason": "already downloading" })),
            ));
        }
        set.insert(repo.clone());
    }
    let task = DownloadTask {
        events: state.events.clone(),
        asr_downloading: state.asr_downloading.clone(),
        url: models::model_url(&repo),
        dest: AsrRuntimeManager::new().model_path(&repo),
        repo: repo.clone(),
    };
    tokio::task::spawn_blocking(move || run_download(task));
    Ok((StatusCode::ACCEPTED, Json(json!({ "repo": repo, "started": true }))))
}

/// Owned inputs for one background model download (moved into the blocking task).
struct DownloadTask {
    events: broadcast::Sender<String>,
    asr_downloading: Arc<Mutex<HashSet<String>>>,
    repo: String,
    url: String,
    dest: PathBuf,
}

/// Stream a GGML `.bin` to disk, broadcasting `asr_download` progress then `asr_download_done`/
/// `asr_download_error`, and clear the in-flight guard.
fn run_download(t: DownloadTask) {
    let DownloadTask { events, asr_downloading, repo, url, dest } = t;
    match download_to(&events, &repo, &url, &dest) {
        Ok(()) => {
            let _ = events.send(json!({ "type": "asr_download_done", "repo": repo }).to_string());
        }
        Err(e) => {
            eprintln!("captured: asr model download failed ({repo}): {e}");
            let _ = events
                .send(json!({ "type": "asr_download_error", "repo": repo, "error": e }).to_string());
        }
    }
    asr_downloading.lock().unwrap().remove(&repo);
}

/// The model download: stream the GGML file in, emitting throttled `asr_download` progress.
fn download_to(
    events: &broadcast::Sender<String>,
    repo: &str,
    url: &str,
    dest: &Path,
) -> Result<(), String> {
    stream_to_file(url, dest, |done, total| {
        let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        let _ = events.send(
            json!({
                "type": "asr_download", "repo": repo, "file": "",
                "downloaded": done, "total": total,
                "fraction": (frac * 10000.0).round() / 10000.0,
            })
            .to_string(),
        );
    })
}

/// Stream `url` (or a `file://` / local path) into `dest` via a `.part` sidecar (atomically renamed on
/// success), calling `on_progress(downloaded, total)` at ≥1% steps + a final 100% tick. Shared by the
/// model + runtime-pack downloads. The `.part` keeps a partial/aborted download from masquerading as a
/// complete file. A local source path is copied (used for dev / an explicit pack `source`).
fn stream_to_file(url: &str, dest: &Path, mut on_progress: impl FnMut(u64, u64)) -> Result<(), String> {
    use std::io::{Read, Write};

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));

    // A local file source (an explicit pack path or file:// URL) → copy; else HTTP stream.
    let local = url.strip_prefix("file://").map(PathBuf::from).or_else(|| {
        (!url.contains("://")).then(|| PathBuf::from(url))
    });
    if let Some(src) = local {
        let total = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
        std::fs::copy(&src, &part).map_err(|e| format!("copy {} -> {}: {e}", src.display(), part.display()))?;
        std::fs::rename(&part, dest).map_err(|e| format!("rename {} -> {}: {e}", part.display(), dest.display()))?;
        on_progress(total, total);
        return Ok(());
    }

    let client = reqwest::blocking::Client::builder().build().map_err(|e| format!("http client: {e}"))?;
    let mut resp = client
        .get(url)
        .header("User-Agent", "capture-daemon")
        .send()
        .map_err(|e| format!("fetch {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("fetch {url}: HTTP {}", resp.status().as_u16()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(&part).map_err(|e| format!("create {}: {e}", part.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    let mut last = -1.0_f64;
    loop {
        let n = resp.read(&mut buf).map_err(|e| format!("read {url}: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("write {}: {e}", part.display()))?;
        done += n as u64;
        let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        if frac - last >= 0.01 {
            last = frac;
            on_progress(done, total);
        }
    }
    file.flush().map_err(|e| format!("flush {}: {e}", part.display()))?;
    drop(file);
    std::fs::rename(&part, dest)
        .map_err(|e| format!("rename {} -> {}: {e}", part.display(), dest.display()))?;
    on_progress(done, if total > 0 { total } else { done });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{asset_matches_pack, parse_pack_semver};

    #[test]
    fn pack_asset_matching() {
        // The right OS/arch + runtime id + a library/archive extension matches.
        let want = format!(
            "whisper-metal-{}-{}.dylib",
            if cfg!(target_os = "macos") { "macos" } else { std::env::consts::OS },
            if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" },
        );
        assert!(asset_matches_pack(&want, "whisper-metal"));
        // Wrong runtime id, or a non-library asset (the .dmg app), or a docs file → no match.
        assert!(!asset_matches_pack(&want, "mlx"));
        assert!(!asset_matches_pack("Capture-0.2.6.dmg", "whisper-metal"));
        assert!(!asset_matches_pack("whisper-metal-notes.txt", "whisper-metal"));
    }

    #[test]
    fn pack_semver_parses_with_optional_v() {
        assert_eq!(parse_pack_semver("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_pack_semver("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_pack_semver("nope"), None);
    }
}
