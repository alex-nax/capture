//! Session + read handlers: list/get/transcript, start/import/stop, delete/prune/mic/retranscribe.

use super::*;

/// `GET /v1/health` — liveness + counts. The read-only daemon has no in-memory live sessions, so
/// `live` is 0; `history` is the count of `capture-*` session dirs (cheap, no JSON parse).
pub(crate) async fn health() -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": VERSION,
        "api_version": API_VERSION,
        "pid": std::process::id(),
        "platform": platform_str(),
        "sessions": { "live": 0, "history": count_session_dirs() },
    }))
}

/// Cheap session-dir count for `/v1/health` (no recovery): `capture-*` folders under the runs dir.
fn count_session_dirs() -> usize {
    std::fs::read_dir(sessions::runs_dir())
        .map(|it| {
            it.flatten()
                .filter(|e| {
                    e.path().is_dir()
                        && e.file_name().to_string_lossy().starts_with("capture-")
                })
                .count()
        })
        .unwrap_or(0)
}

/// `GET /v1/sessions` — the session list (oldest first), from disk, with any **live** capture's entry
/// replaced by its authoritative in-memory summary (a recovered "running" reads as "interrupted").
pub(crate) async fn list_sessions(State(state): State<AppState>) -> Json<Value> {
    let disk = sessions::list_sessions(&sessions::runs_dir());
    let live: std::collections::HashMap<String, Value> = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .map(|s| (s.id().to_string(), s.summary()))
        .collect();
    let mut out: Vec<Value> = disk
        .into_iter()
        .map(|s| match live.get(&s.session_id) {
            Some(v) => v.clone(),
            None => serde_json::to_value(s).unwrap_or(Value::Null),
        })
        .collect();
    // Live sessions whose dir isn't under the runs dir won't be in the disk list — append them.
    let seen: std::collections::HashSet<String> = out
        .iter()
        .filter_map(|v| v.get("session_id").and_then(|x| x.as_str()).map(String::from))
        .collect();
    for (id, summary) in live {
        if !seen.contains(&id) {
            out.push(summary);
        }
    }
    Json(json!({ "sessions": out }))
}

/// `GET /v1/sessions/{id}` — a live capture's in-memory summary if running, else the recovered
/// on-disk session, or 404.
pub(crate) async fn get_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    if let Some(s) = state.sessions.lock().unwrap().get(&id).cloned() {
        return Ok(Json(s.summary()));
    }
    sessions::find_session(&sessions::runs_dir(), &id)
        .map(|s| Json(serde_json::to_value(s).unwrap_or(Value::Null)))
        .ok_or_else(|| ApiError::unknown_session(&id))
}

#[derive(Deserialize)]
pub(crate) struct TailQuery {
    tail: Option<i64>,
}

/// `GET /v1/sessions/{id}/transcript[?tail=N]` — the raw transcript segments (each `transcript.jsonl`
/// line passed through verbatim), optionally the last `N`. Mirrors `_transcript`.
pub(crate) async fn transcript(
    UrlPath(id): UrlPath<String>,
    Query(q): Query<TailQuery>,
) -> Result<Json<Value>, ApiError> {
    let s = sessions::find_session(&sessions::runs_dir(), &id)
        .ok_or_else(|| ApiError::unknown_session(&id))?;
    let path = Path::new(&s.dir).join("transcript.jsonl");
    let mut segs: Vec<Value> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for ln in text.lines() {
            let ln = ln.trim();
            if !ln.is_empty() {
                if let Ok(v) = serde_json::from_str::<Value>(ln) {
                    segs.push(v);
                }
            }
        }
    }
    if let Some(t) = q.tail {
        if t >= 0 {
            let t = t as usize;
            if segs.len() > t {
                segs = segs.split_off(segs.len() - t);
            }
        }
    }
    let count = segs.len();
    Ok(Json(json!({ "session_id": id, "segments": segs, "count": count })))
}

/// `POST /v1/sessions` — start a live capture and return its summary. Two modes: **attach** (pid /
/// app_name / window_id) and **launch** (`command` → spawn a child, capture its window + audio, tee
/// its stdout/stderr to disk). Mirrors `_start_session`.
pub(crate) async fn start_session(State(state): State<AppState>, body: Bytes) -> Result<Json<Value>, ApiError> {
    let req: v1::StartSessionRequest = parse_json(&body)?;
    let config = CaptureConfig {
        output_dir: req.output_dir,
        command: req.command,
        cwd: req.cwd,
        pid: req.pid,
        window_id: req.window_id,
        app_name: req.app_name,
        bundle_id: req.bundle_id,
        screenshot_interval: req.screenshot_interval,
        screenshot_format: req.screenshot_format,
        screenshot_resolution: req.screenshot_resolution,
        screenshot_jpeg_quality: req.screenshot_jpeg_quality,
        capture_screenshots: req.capture_screenshots,
        capture_audio: req.capture_audio,
        audio_source: req.audio_source,
        mic_device: req.mic_device,
        audio_chunk_seconds: req.audio_chunk_seconds,
        asr_backend: req.asr_backend,
        preset: req.preset,
    };
    let events = state.events.clone();
    let emit: EventSink = Arc::new(move |v: Value| {
        let _ = events.send(v.to_string());
    });
    let session = Arc::new(CaptureSession::new(config, emit));
    let id = session.id().to_string();
    state.sessions.lock().unwrap().insert(id.clone(), session.clone());

    // start() does window-server queries + spawns the capture threads — off the async worker.
    let s2 = session.clone();
    let result = tokio::task::spawn_blocking(move || s2.start())
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("start task: {e}")))?;
    match result {
        Ok(summary) => {
            maybe_start_live_index(&state, &id, &summary);
            Ok(Json(summary))
        }
        Err(e) => {
            state.sessions.lock().unwrap().remove(&id);
            Err(ApiError::bad(format!("capture failed to start: {e}")))
        }
    }
}

/// #84: if a vision endpoint was probed reachable (`state.last_index`, set by `GET /v1/index/status`),
/// build the session's index LIVE while it captures — a background worker driving
/// `capture_index::live::run_worker` off the capture hot path, finalizing when the capture stops
/// (`stop_session` flips the stop flag). No endpoint ⇒ no-op (the post-capture build still works).
/// Never blocks the start path; a flaky endpoint can't break capture.
fn maybe_start_live_index(state: &AppState, id: &str, summary: &Value) {
    use std::sync::atomic::AtomicBool;
    let Some((url, model)) = state.last_index.lock().unwrap().clone() else {
        return;
    };
    let Some(dir) = summary.get("dir").and_then(|d| d.as_str()).map(String::from) else {
        return;
    };
    let preset = summary
        .get("index_preset")
        .and_then(|p| p.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("auto")
        .to_string();
    let stop = Arc::new(AtomicBool::new(false));
    state.live_index_stops.lock().unwrap().insert(id.to_string(), stop.clone());
    let events = state.events.clone();
    let stops = state.live_index_stops.clone();
    let sid = id.to_string();
    tokio::task::spawn_blocking(move || {
        // Build the vision client; bail (no-op) if it isn't actually reachable right now.
        let client = match capture_index::vision::load(Some(&url), model.as_deref()) {
            Ok(c) if c.available() => c,
            _ => {
                stops.lock().unwrap().remove(&sid);
                return;
            }
        };
        let model_label = client.model.clone();
        let ev = events.clone();
        let sid2 = sid.clone();
        let on_progress = move |leaves: usize| {
            // Live indexing is open-ended → a soft asymptotic fraction (never 100% until finalize) so
            // the dashboard's indexing row shows a growing bar; phase "live" marks it as the auto build.
            let frac = 1.0 - 1.0 / (1.0 + leaves as f64 / 30.0);
            let _ = ev.send(
                json!({ "type": "index", "session_id": sid2, "phase": "live",
                        "done": leaves, "total": 0, "fraction": (frac * 10000.0).round() / 10000.0 })
                    .to_string(),
            );
        };
        let _ = capture_index::live::run_worker(
            std::path::Path::new(&dir), &client, &preset, 0.5, true,
            model_label.as_deref(), &stop, 1.0, 8, on_progress,
        );
        let _ = events.send(json!({ "type": "index_done", "session_id": sid }).to_string());
        stops.lock().unwrap().remove(&sid);
    });
}

/// Expand a leading `~/` to `$HOME` (the daemon validates the source path before importing).
fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        // Windows: %USERPROFILE% (match dirs::home_dir()); $HOME is unset outside a shell.
        #[cfg(windows)]
        if let Some(home) = std::env::var_os("USERPROFILE") {
            return PathBuf::from(home).join(rest);
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

/// `POST /v1/sessions/import` — turn an existing audio/video file into a session (extract audio +
/// frames, run ASR), returning its summary. Synchronous (runs off the async worker); progress streams
/// over `/v1/events` as `import` events. 400 if the file is missing or has no audio/video. Mirrors
/// `_import_session` + `import_file`.
pub(crate) async fn import_session(State(state): State<AppState>, body: Bytes) -> Result<Json<Value>, ApiError> {
    let req: v1::ImportMediaRequest = parse_json(&body)?;
    let src = expand_tilde(&req.path);
    if !src.is_file() {
        return Err(ApiError::bad(format!("file not found: {}", req.path)));
    }
    let output_dir = req
        .output_dir
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sessions::runs_dir().to_string_lossy().into_owned());

    let events = state.events.clone();
    let emit: EventSink = Arc::new(move |v: Value| {
        let _ = events.send(v.to_string());
    });
    let backend = req.asr_backend.clone();
    let interval = req.screenshot_interval;
    let summary = tokio::task::spawn_blocking(move || {
        capture_engine::import_media(&output_dir, &src, &backend, interval, &emit)
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("import task: {e}")))?
    .map_err(ApiError::bad)?;
    Ok(Json(summary))
}

/// `POST /v1/sessions/{id}/stop` — stop a live capture (finalize on disk), or return an
/// already-finished session. 404 if unknown. Mirrors `_stop_session`.
pub(crate) async fn stop_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    // Tell the live-index worker (if any) to finalize as the capture stops (#84).
    if let Some(stop) = state.live_index_stops.lock().unwrap().get(&id) {
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let live = state.sessions.lock().unwrap().get(&id).cloned();
    if let Some(session) = live {
        let summary = tokio::task::spawn_blocking(move || session.stop())
            .await
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("stop task: {e}")))?;
        state.sessions.lock().unwrap().remove(&id);
        return Ok(Json(summary));
    }
    sessions::find_session(&sessions::runs_dir(), &id)
        .map(|s| Json(serde_json::to_value(s).unwrap_or(Value::Null)))
        .ok_or_else(|| ApiError::unknown_session(&id))
}

/// True iff `id` is a live capture in the registry (one a manage-route must refuse). The read layer
/// rewrites a recovered live state to "interrupted", so the registry is the authority on "live".
fn is_live(state: &AppState, id: &str) -> bool {
    state
        .sessions
        .lock()
        .unwrap()
        .get(id)
        .map(|s| LIVE_STATES.contains(&s.state().as_str()))
        .unwrap_or(false)
}

/// `POST /v1/sessions/{id}/delete` — remove a finished capture's dir + forget it. 404 unknown, 400
/// live. Mirrors `_delete_session`.
pub(crate) async fn delete_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    if is_live(&state, &id) {
        return Err(ApiError::bad("stop the capture before deleting it"));
    }
    let session = sessions::find_session(&sessions::runs_dir(), &id)
        .ok_or_else(|| ApiError::unknown_session(&id))?;
    // Remove the dir only if it really is a capture dir (has a session.json) — never an arbitrary path.
    let dir = std::path::PathBuf::from(&session.dir);
    if dir.is_dir() && dir.join("session.json").is_file() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    sessions::remove_from_index(&id);
    Ok(Json(json!({ "deleted": true, "session_id": id })))
}

#[derive(Deserialize)]
struct PruneBody {
    #[serde(default)]
    parts: Vec<String>,
}

/// `POST /v1/sessions/{id}/prune` — free disk on a finished capture (delete/halve screenshots,
/// drop audio). Returns freed bytes + the refreshed capability flags. 404 unknown, 400 live / bad
/// parts. Mirrors `_prune_session`.
pub(crate) async fn prune_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    if is_live(&state, &id) {
        return Err(ApiError::bad("stop the capture before pruning it"));
    }
    let session = sessions::find_session(&sessions::runs_dir(), &id)
        .ok_or_else(|| ApiError::unknown_session(&id))?;
    let req: PruneBody = parse_json(&body)?;
    if req.parts.is_empty() {
        return Err(ApiError::bad(format!(
            "specify 'parts': a non-empty list of {}",
            sessions::PRUNE_PARTS.join(", ")
        )));
    }
    let bad: Vec<&str> =
        req.parts.iter().map(String::as_str).filter(|p| !sessions::PRUNE_PARTS.contains(p)).collect();
    if !bad.is_empty() {
        return Err(ApiError::bad(format!(
            "unknown prune part(s) {bad:?}; choose from {:?}",
            sessions::PRUNE_PARTS
        )));
    }
    let dir = std::path::PathBuf::from(&session.dir);
    if !dir.join("session.json").is_file() {
        return Err(ApiError::bad("session dir is missing or not a capture dir"));
    }
    let (freed, count) = sessions::prune_session_dir(&dir, &req.parts);
    let caps = sessions::capabilities(&dir);
    // Reflect the prune in session.json's summary (screenshot count + capability flags).
    let mut updates = caps.clone();
    if let Value::Object(ref mut m) = updates {
        m.insert("screenshots".into(), json!(count));
    }
    sessions::rewrite_session_summary(&dir, &updates);
    // `{pruned, freed_bytes, screenshots, <capability flags>}`.
    let mut out = caps;
    if let Value::Object(ref mut m) = out {
        m.insert("pruned".into(), json!(req.parts));
        m.insert("freed_bytes".into(), json!(freed));
        m.insert("screenshots".into(), json!(count));
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct MicBody {
    #[serde(default)]
    device: Option<String>,
}

/// `POST /v1/sessions/{id}/mic` — switch the microphone on a RUNNING capture (`{device}`: an id /
/// `"default"` = on/switch, `null`/`""` = off). 404 if unknown or finished, 400 if not live. Mirrors
/// `_set_mic`.
pub(crate) async fn set_mic(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let req: MicBody = parse_json(&body)?;
    let session = state.sessions.lock().unwrap().get(&id).cloned();
    let Some(session) = session else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("unknown or finished session_id {id:?}"),
        ));
    };
    let summary =
        tokio::task::spawn_blocking(move || session.set_mic_device(req.device.as_deref()))
            .await
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("mic task: {e}")))?
            .map_err(ApiError::bad)?;
    Ok(Json(summary))
}

#[derive(Deserialize)]
struct RetranscribeBody {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    chunk_seconds: Option<f64>,
}

/// `POST /v1/sessions/{id}/retranscribe` — re-run ASR over a finished capture's audio with the active
/// (or a just-selected) model, replacing its transcript (background; progress over SSE). 404 unknown,
/// 400 live / no audio. Mirrors `_retranscribe_session` + `start_retranscribe`.
pub(crate) async fn retranscribe(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if is_live(&state, &id) {
        return Err(ApiError::bad("stop the capture before re-transcribing it"));
    }
    let session = sessions::find_session(&sessions::runs_dir(), &id)
        .ok_or_else(|| ApiError::unknown_session(&id))?;
    if !session.can_retranscribe {
        return Err(ApiError::bad("no audio to re-transcribe (it was pruned or never captured)"));
    }
    let req: RetranscribeBody = parse_json(&body)?;
    // Optionally switch the active model / language first (persisted settings), like the Python.
    if let Some(m) = req.model.as_deref().filter(|s| !s.is_empty()) {
        models::set_active_model(m).map_err(ApiError::bad)?;
    }
    if req.language.is_some() {
        models::set_active_language(req.language.as_deref()).map_err(ApiError::bad)?;
    }
    {
        let mut set = state.retranscribing.lock().unwrap();
        if set.contains(&id) {
            return Ok((
                StatusCode::ACCEPTED,
                Json(json!({ "session_id": id, "started": false, "reason": "already re-transcribing" })),
            ));
        }
        set.insert(id.clone());
    }
    let chunk = req.chunk_seconds.filter(|s| *s > 0.0).unwrap_or_else(models::active_chunk_seconds);
    let task = RetranscribeTask {
        events: state.events.clone(),
        retranscribing: state.retranscribing.clone(),
        sid: id.clone(),
        dir: session.dir.clone(),
        chunk,
    };
    tokio::task::spawn_blocking(move || run_retranscribe(task));
    Ok((StatusCode::ACCEPTED, Json(json!({ "session_id": id, "started": true }))))
}

/// Owned inputs for one background re-transcribe (moved into the blocking task).
struct RetranscribeTask {
    events: broadcast::Sender<String>,
    retranscribing: Arc<Mutex<HashSet<String>>>,
    sid: String,
    dir: String,
    chunk: f64,
}

/// Run one re-transcribe, broadcasting throttled `retranscribe` progress then `retranscribe_done`/
/// `retranscribe_error`, updating `session.json`'s `transcript_segments`, and clearing the guard.
fn run_retranscribe(t: RetranscribeTask) {
    let RetranscribeTask { events, retranscribing, sid, dir, chunk } = t;
    let mut last = -1.0_f64;
    let on_progress = |done: u64, total: u64, segs: i64| {
        let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        if frac - last < 0.02 && frac < 1.0 {
            return;
        }
        last = frac;
        let _ = events.send(
            json!({ "type": "retranscribe", "session_id": sid, "fraction": (frac * 10000.0).round() / 10000.0, "segments": segs })
                .to_string(),
        );
    };
    match capture_engine::retranscribe_session(Path::new(&dir), chunk, on_progress) {
        Ok(n) => {
            sessions::rewrite_session_summary(Path::new(&dir), &json!({ "transcript_segments": n }));
            let _ = events.send(
                json!({ "type": "retranscribe_done", "session_id": sid, "segments": n }).to_string(),
            );
        }
        Err(e) => {
            eprintln!("captured: retranscribe failed ({sid}): {e}");
            let _ = events.send(
                json!({ "type": "retranscribe_error", "session_id": sid, "error": e }).to_string(),
            );
        }
    }
    retranscribing.lock().unwrap().remove(&sid);
}
