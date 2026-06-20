//! Multimodal index handlers: get/status + the background build (capture-index).

use super::*;

/// `GET /v1/sessions/{id}/index` — the built index tree (the `index.json` passed through verbatim),
/// 404 if not indexed yet. Mirrors `_index_get` / `indexer.load_index`.
pub(crate) async fn index_get(UrlPath(id): UrlPath<String>) -> Result<Json<Value>, ApiError> {
    let s = sessions::find_session(&sessions::runs_dir(), &id)
        .ok_or_else(|| ApiError::unknown_session(&id))?;
    let not_indexed = || ApiError::new(StatusCode::NOT_FOUND, "session is not indexed yet");
    let txt = std::fs::read_to_string(Path::new(&s.dir).join("index.json")).map_err(|_| not_indexed())?;
    let v: Value = serde_json::from_str(&txt).map_err(|_| not_indexed())?;
    Ok(Json(v))
}

#[derive(Deserialize)]
pub(crate) struct IndexStatusQuery {
    url: Option<String>,
    model: Option<String>,
}

/// `GET /v1/index/status[?url=&model=]` — whether indexing is available: a configured endpoint that
/// answers a `/v1/models` preflight. Drives the GUI's gate. Mirrors `_index_status`.
pub(crate) async fn index_status(
    State(state): State<AppState>,
    Query(q): Query<IndexStatusQuery>,
) -> Json<Value> {
    let configured = capture_index::vision::configured_url(q.url.as_deref());
    if configured.is_empty() {
        return Json(json!({ "available": false, "configured": false, "url": null, "model": null }));
    }
    // The preflight is a blocking HTTP call — run it off the async worker.
    let url = q.url.clone();
    let model = q.model.clone();
    let (available, used_model) = tokio::task::spawn_blocking(move || {
        let fallback = model
            .clone()
            .or_else(|| std::env::var(capture_index::vision::ENV_MODEL).ok());
        match capture_index::vision::load(url.as_deref(), model.as_deref()) {
            Ok(client) => {
                let m = client.model.clone();
                (client.available(), m.or(fallback))
            }
            Err(_) => (false, fallback),
        }
    })
    .await
    .unwrap_or((false, None));
    // Remember a reachable endpoint so a capture can index LIVE without the GUI passing it (#84).
    if available {
        *state.last_index.lock().unwrap() = Some((configured.clone(), used_model.clone()));
    }
    Json(json!({
        "available": available,
        "configured": true,
        "url": configured,
        "model": used_model,
    }))
}

#[derive(Deserialize)]
pub(crate) struct IndexModelsQuery {
    provider: Option<String>,
    host: Option<String>,
    port: Option<String>,
    key: Option<String>,
    /// A full chat/base URL — overrides provider/host/port (the GUI's back-compat path).
    url: Option<String>,
}

/// `GET /v1/index/models[?provider=&host=&port=&key=]` (or `?url=`) — list a provider's available
/// models for the GUI dropdown (#53). Always 200 `{models, provider, reachable}`; `[]` +
/// `reachable:false` if unreachable/unauthenticated. Mirrors `_index_models`.
pub(crate) async fn index_models(Query(q): Query<IndexModelsQuery>) -> Json<Value> {
    let provider = q
        .provider
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| capture_index::providers::DEFAULT_PROVIDER.to_string());
    let key = q
        .key
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var(capture_index::vision::ENV_KEY).ok());
    let host = q.host.clone();
    let port = q.port.as_deref().and_then(|p| p.trim().parse::<u32>().ok());
    let url = q.url.clone().filter(|s| !s.trim().is_empty());
    let prov = provider.clone();

    // `list_models` does a blocking HTTP GET — run it off the async worker.
    let models = tokio::task::spawn_blocking(move || {
        if let Some(u) = url {
            // explicit URL (full chat or base) → derive the base, list via the `custom` provider.
            let base = u
                .rsplit_once("/chat/completions")
                .map(|(b, _)| b)
                .unwrap_or(&u)
                .trim_end_matches('/')
                .to_string();
            capture_index::providers::list_models("custom", Some(&base), None, key.as_deref())
        } else {
            capture_index::providers::list_models(&prov, host.as_deref(), port, key.as_deref())
        }
    })
    .await
    .unwrap_or_default();

    let reachable = !models.is_empty();
    Json(json!({ "models": models, "provider": provider, "reachable": reachable }))
}

/// `GET /v1/index/providers` — the provider catalog for the GUI's endpoint config. Mirrors the
/// Python: each provider's id + its public fields (the internal `fixed_base` is omitted).
pub(crate) async fn index_providers() -> Json<Value> {
    let providers: Vec<Value> = capture_index::providers::PROVIDERS
        .iter()
        .map(|p| {
            let mut o = serde_json::Map::new();
            o.insert("id".into(), json!(p.id));
            o.insert("label".into(), json!(p.label));
            if let Some(s) = p.scheme {
                o.insert("scheme".into(), json!(s));
            }
            if let Some(dp) = p.default_port {
                o.insert("default_port".into(), json!(dp));
            }
            if p.base_url {
                o.insert("base_url".into(), json!(true));
            }
            o.insert("needs_key".into(), json!(p.needs_key));
            Value::Object(o)
        })
        .collect();
    Json(json!({ "providers": providers, "default": capture_index::providers::DEFAULT_PROVIDER }))
}

/// The endpoint a build/status should use: an explicit `endpoint` wins, else compose the
/// chat-completions URL from the structured provider config (#52), else `None` (→ env fallback).
/// Mirrors `start_index`'s `if not endpoint and req.provider: chat_url(...)`.
fn effective_endpoint(req: &v1::IndexRequest) -> Option<String> {
    if let Some(e) = req.endpoint.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(e.to_string());
    }
    let p = req.provider.as_deref().map(str::trim).filter(|s| !s.is_empty())?;
    capture_index::providers::chat_url(p, req.host.as_deref(), req.port.map(|n| n as u32)).ok()
}

/// `POST /v1/sessions/{id}/index` — build the multimodal index in the background (202 immediately;
/// progress over `/v1/events`). Mirrors `_index_session` + `start_index`. 404 unknown, 400 live /
/// no-screenshots / bad-params, 503 endpoint unset/unreachable.
pub(crate) async fn index_build(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    // Parse the IndexRequest (empty body → all defaults, like the Python leniency).
    let req: v1::IndexRequest = if body.is_empty() {
        serde_json::from_str("{}").map_err(|e| ApiError::bad(format!("invalid request: {e}")))?
    } else {
        serde_json::from_slice(&body).map_err(|e| ApiError::bad(format!("invalid request: {e}")))?
    };
    // pydantic IndexRequest validators (serde doesn't enforce these).
    if !(req.sample_rate > 0.0 && req.sample_rate <= 1.0) {
        return Err(ApiError::bad("sample_rate must be in (0, 1]"));
    }
    if req.max_leaves < 1 {
        return Err(ApiError::bad("max_leaves must be >= 1"));
    }

    let session = sessions::find_session(&sessions::runs_dir(), &id)
        .ok_or_else(|| ApiError::unknown_session(&id))?;
    if LIVE_STATES.contains(&session.state.as_str()) {
        return Err(ApiError::bad("stop the capture before indexing it"));
    }
    if !session.can_index {
        return Err(ApiError::bad(
            "no screenshots to index (capture some, or this session has none)",
        ));
    }

    // Endpoint gate: explicit `endpoint` else the structured provider config (#52) else env;
    // configured? then reachable?
    let endpoint = effective_endpoint(&req);
    let configured = capture_index::vision::configured_url(endpoint.as_deref());
    if configured.is_empty() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "indexing is disabled: set CAPTURE_INDEX_URL (or pass 'endpoint') to an LM Studio server",
        ));
    }
    let ep = endpoint.clone();
    let model = req.model.clone();
    let reachable = tokio::task::spawn_blocking(move || {
        capture_index::vision::load(ep.as_deref(), model.as_deref())
            .map(|c| c.available())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    if !reachable {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "index endpoint not reachable; check the LM Studio server is running",
        ));
    }

    // Guard: one build per session at a time.
    {
        let mut set = state.indexing.lock().unwrap();
        if set.contains(&id) {
            return Ok((
                StatusCode::ACCEPTED,
                Json(json!({ "session_id": id, "started": false, "reason": "already indexing" })),
            ));
        }
        set.insert(id.clone());
    }

    let task = IndexTask {
        events: state.events.clone(),
        indexing: state.indexing.clone(),
        sid: id.clone(),
        dir: session.dir.clone(),
        endpoint,
        model: req.model.clone(),
        max_px: req.max_px,
        sample_rate: req.sample_rate,
        max_leaves: req.max_leaves as usize,
        fuse_transcript: req.fuse_transcript,
        leaf_prompt: req.leaf_prompt.clone(),
        leaf_schema: req.leaf_schema.clone(),
        classify_prompt: req.classify_prompt.clone(),
        prompt_preset: req.prompt_preset.clone(),
    };
    tokio::task::spawn_blocking(move || run_index_build(task));

    Ok((StatusCode::ACCEPTED, Json(json!({ "session_id": id, "started": true }))))
}

/// Owned inputs for one background index build (moved into the blocking task).
struct IndexTask {
    events: broadcast::Sender<String>,
    indexing: Arc<Mutex<HashSet<String>>>,
    sid: String,
    dir: String,
    endpoint: Option<String>,
    model: Option<String>,
    max_px: Option<u32>,
    sample_rate: f64,
    max_leaves: usize,
    fuse_transcript: bool,
    leaf_prompt: Option<String>,
    leaf_schema: Option<Value>,
    classify_prompt: Option<String>,
    prompt_preset: Option<String>,
}

/// Run one index build to completion, broadcasting `index` progress then `index_done`/`index_error`,
/// and clear the in-flight guard. Mirrors `start_index::run`.
fn run_index_build(t: IndexTask) {
    let IndexTask { events, indexing, sid, .. } = &t;
    match build_one(events, sid, &t) {
        Ok((node_count, leaf_count)) => {
            let _ = events.send(
                json!({ "type": "index_done", "session_id": sid, "node_count": node_count, "leaf_count": leaf_count })
                    .to_string(),
            );
        }
        Err(e) => {
            eprintln!("captured: index failed ({sid}): {e}");
            let _ = events
                .send(json!({ "type": "index_error", "session_id": sid, "error": e }).to_string());
        }
    }
    indexing.lock().unwrap().remove(sid);
}

/// The build itself: load + preflight the vision client, then `build_index` with a throttled
/// progress callback that broadcasts `index` events. Returns `(node_count, leaf_count)`.
fn build_one(
    events: &broadcast::Sender<String>,
    sid: &str,
    t: &IndexTask,
) -> Result<(usize, usize), String> {
    let mut client = capture_index::vision::load(t.endpoint.as_deref(), t.model.as_deref())?;
    if let Some(px) = t.max_px {
        client.max_px = px;
    }
    if !client.available() {
        return Err("index endpoint not reachable (configure a working LM Studio URL)".into());
    }
    let model_label = client.model.clone();
    // #54: an index with no explicit preset defaults to the session's recorded capture preset.
    let preset = t
        .prompt_preset
        .clone()
        .or_else(|| session_index_preset(&t.dir))
        .unwrap_or_else(|| "auto".to_string());

    let mut last = -1.0_f64;
    let mut on_progress = |phase: &str, done: usize, total: usize, _lo: f64, _hi: Option<f64>| {
        let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        // Throttle within a build (mirrors the 2% gate); always emit the final node.
        if done < total && frac - last < 0.02 {
            return;
        }
        last = frac;
        let _ = events.send(
            json!({
                "type": "index", "session_id": sid, "phase": phase,
                "done": done, "total": total, "fraction": (frac * 10000.0).round() / 10000.0,
            })
            .to_string(),
        );
    };

    let opts = capture_index::build::BuildOptions {
        sample_rate: t.sample_rate,
        max_leaves: t.max_leaves,
        fuse_transcript: t.fuse_transcript,
        prompt_preset: Some(preset.as_str()),
        leaf_prompt: t.leaf_prompt.as_deref(),
        leaf_schema: t.leaf_schema.as_ref(),
        classify_prompt: t.classify_prompt.as_deref(),
        code_max_px: None,
        model_label: model_label.as_deref(),
    };
    let idx = capture_index::build::build_index(Path::new(&t.dir), &client, &opts, Some(&mut on_progress))?;
    Ok((idx.node_count, idx.leaf_count))
}

/// The `index_preset` recorded on a session at capture time (#54), or None. Mirrors
/// `daemon/server.py::_session_index_preset`.
fn session_index_preset(dir: &str) -> Option<String> {
    let txt = std::fs::read_to_string(Path::new(dir).join("session.json")).ok()?;
    let v: Value = serde_json::from_str(&txt).ok()?;
    v.get("config")?
        .get("index_preset")?
        .as_str()
        .map(String::from)
}
