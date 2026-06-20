//! The `/v1` router + handlers (a port of `daemon/server.py::_route`/`_route_asr`).
//!
//! READY routes are served from `capture-core` (sessions, transcript), `capture-index` (index
//! build/get/status), `capture-asr` (the ASR runtime + GGML model manager), and `capture-platform`
//! (the read-only platform layer: windows, permissions, audio devices). Capture start (attach + launch),
//! stop, mic, import, the session-management routes, and the index provider catalog/model listing are
//! all live (#65/#43/#67). The only remaining `501` route is `/v1/schema` (the pydantic contract
//! artifact, intentionally not ported) — see [`not_implemented`].
//!
//! The handlers are grouped into domain submodules ([`session_routes`], [`index`], [`asr`],
//! [`platform`]); this module keeps the [`router`], auth, the shared `parse_json` helper, the SSE
//! stream, and stubs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use capture_asr::{models, AsrRuntimeManager};
use capture_core::sessions;
use capture_core::v1;
use capture_engine::{CaptureConfig, CaptureSession, EventSink};

use crate::{platform_str, ApiError, AppState, API_VERSION, LIVE_STATES, VERSION};

mod asr;
mod index;
mod platform;
// The session/read handlers live in `sessions.rs`; aliased to `session_routes` so the module name
// doesn't shadow the `capture_core::sessions` import the handlers themselves use.
#[path = "sessions.rs"]
mod session_routes;

/// Build the full `/v1` router with bearer auth applied to every route except `/v1/health`.
pub fn router(state: AppState) -> Router {
    Router::new()
        // -- ready: liveness + event stream --
        .route("/v1/health", get(session_routes::health))
        .route("/v1/events", get(events))
        // -- ready: session read layer (capture-core::sessions) + live start (capture-engine) --
        .route("/v1/sessions", get(session_routes::list_sessions).post(session_routes::start_session))
        .route("/v1/sessions/{id}", get(session_routes::get_session))
        .route("/v1/sessions/{id}/transcript", get(session_routes::transcript))
        // -- ready: multimodal index (capture-index) --
        .route("/v1/sessions/{id}/index", get(index::index_get).post(index::index_build))
        .route("/v1/index/status", get(index::index_status))
        // -- ready: admin --
        .route("/v1/admin/shutdown", post(shutdown))
        // -- engine: capture / import --
        .route("/v1/sessions/import", post(session_routes::import_session))
        .route("/v1/sessions/{id}/stop", post(session_routes::stop_session))
        .route("/v1/sessions/{id}/delete", post(session_routes::delete_session))
        .route("/v1/sessions/{id}/prune", post(session_routes::prune_session))
        .route("/v1/sessions/{id}/retranscribe", post(session_routes::retranscribe))
        .route("/v1/sessions/{id}/mic", post(session_routes::set_mic))
        // -- ready: platform read layer (capture-platform) --
        .route("/v1/windows", get(platform::windows))
        .route("/v1/audio/mics", get(platform::audio_mics))
        .route("/v1/permissions", get(platform::permissions))
        .route("/v1/permissions/request", post(platform::permissions_request))
        // -- ready: index provider introspection (capture-index::providers) --
        .route("/v1/index/models", get(index::index_models))
        .route("/v1/index/providers", get(index::index_providers))
        // -- not ported: pydantic schema introspection (contract-test artifact) --
        .route("/v1/schema", get(not_implemented))
        // -- ready: ASR runtime + model manager (capture-asr) --
        .route("/v1/asr/runtimes", get(asr::asr_runtimes))
        .route("/v1/asr/runtimes/install", post(asr::asr_runtime_install))
        .route("/v1/asr/runtime", post(asr::asr_set_runtime))
        .route("/v1/asr/backend", get(asr::asr_backend))
        .route("/v1/asr/models", get(asr::asr_models))
        .route("/v1/asr/models/download", post(asr::asr_model_download))
        .route("/v1/asr/models/delete", post(asr::asr_model_delete))
        .route("/v1/asr/model", post(asr::asr_set_model))
        .route("/v1/asr/language", post(asr::asr_set_language))
        .route("/v1/asr/chunk", post(asr::asr_set_chunk))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

// ── Auth ──────────────────────────────────────────────────────────────────────────────────────

/// Bearer-token gate. `/v1/health` is the only unauthenticated route (a liveness probe); every other
/// route — including the SSE stream and unknown paths — requires `Authorization: Bearer <token>`.
/// Mirrors the Python ordering (health → auth → dispatch).
async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if req.uri().path() == "/v1/health" {
        return next.run(req).await;
    }
    let want = format!("Bearer {}", state.token);
    let got = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if ct_eq(got.as_bytes(), want.as_bytes()) {
        next.run(req).await
    } else {
        ApiError::new(StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
    }
}

/// Constant-time byte compare (mirrors `secrets.compare_digest`). Length mismatch fails fast.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Parse a JSON request body; an empty body is treated as `{}` (the Python's leniency). A parse
/// failure becomes a 400 `{error}` (mirrors the daemon, not axum's default plain-text 400).
fn parse_json<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    let raw: &[u8] = if body.is_empty() { b"{}" } else { body.as_ref() };
    serde_json::from_slice(raw).map_err(|e| ApiError::bad(format!("invalid request: {e}")))
}

/// `POST /v1/admin/shutdown` — gracefully stop the server.
async fn shutdown(State(state): State<AppState>) -> Json<Value> {
    state.shutdown.notify_one();
    Json(json!({ "shutdown": true }))
}

/// `GET /v1/events` — the SSE event stream (index progress + future engine events). 15s keep-alive
/// comments; lagged subscribers drop the missed events rather than disconnect.
async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(data) => Some(Ok(Event::default().data(data))),
        Err(_lagged) => None,
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
}

// ── Engine stubs ──────────────────────────────────────────────────────────────────────────────

/// The one intentionally-unported route: `/v1/schema` (the pydantic contract-introspection artifact,
/// a v2 contract-test helper with no Rust equivalent — the serde types in `capture-core` are the v3
/// contract source of truth).
async fn not_implemented() -> ApiError {
    ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "/v1/schema is a v2 pydantic-introspection artifact, not ported (capture-core serde types are the contract)",
    )
}

/// Unknown route → `{"error":"not found"}` 404 (mirrors the Python catch-all).
async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt; // for `oneshot`

    // Serialize tests that mutate CAPTURE_RUNS_DIR (process-global env).
    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn app() -> (Router, String) {
        let state = AppState::new("test-token".to_string());
        let token = state.token.to_string();
        (router(state), token)
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn health_is_unauthenticated() {
        let (app, _token) = app();
        let resp = app
            .oneshot(HttpRequest::builder().uri("/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["api_version"], json!(API_VERSION));
    }

    #[tokio::test]
    async fn sessions_requires_bearer() {
        let (app, _token) = app();
        // No Authorization header → 401.
        let resp = app
            .oneshot(HttpRequest::builder().uri("/v1/sessions").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn engine_route_is_501_when_authed() {
        // /v1/schema (the pydantic contract artifact) is the one intentionally-unported route.
        let (app, token) = app();
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/schema")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn index_providers_lists_the_catalog() {
        let (app, token) = app();
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/index/providers")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["default"], "lmstudio");
        let ids: Vec<&str> = v["providers"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"lmstudio") && ids.contains(&"openai") && ids.contains(&"custom"));
    }

    #[tokio::test]
    async fn unknown_route_is_404_when_authed() {
        let (app, token) = app();
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/nope")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn sessions_list_reads_from_runs_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        // A temp runs dir with one recoverable session.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let runs = std::env::temp_dir().join(format!("capture-daemon-test-{nanos}"));
        let dir = runs.join("capture-20260618-aaa");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            r#"{"config":{},"summary":{"state":"stopped","audio_status":"off"}}"#,
        )
        .unwrap();
        std::env::set_var("CAPTURE_RUNS_DIR", &runs);
        std::env::set_var("CAPTURE_SESSION_INDEX", runs.join("nope.jsonl"));

        let (app, token) = app();
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/sessions")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        let arr = v["sessions"].as_array().expect("sessions array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["session_id"], json!("20260618-aaa"));
        assert_eq!(arr[0]["state"], json!("stopped"));

        std::env::remove_var("CAPTURE_RUNS_DIR");
        std::env::remove_var("CAPTURE_SESSION_INDEX");
        std::fs::remove_dir_all(&runs).ok();
    }

    /// One authed request through the router (consumes the app, like the other oneshot tests).
    async fn req(app: Router, method: &str, uri: &str, token: &str, body: &str) -> Response {
        app.oneshot(
            HttpRequest::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn asr_runtimes_lists_whisper_and_remote() {
        let (srv, token) = app();
        let resp = req(srv, "GET", "/v1/asr/runtimes", &token, "").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        let rts = v["runtimes"].as_array().expect("runtimes array");
        assert!(rts.iter().any(|r| r["engine"] == json!("whisper.cpp")));
        assert!(rts.iter().any(|r| r["id"] == json!("remote")));
        assert!(v["gpu"]["nvidia"].is_boolean());
    }

    #[tokio::test]
    async fn asr_set_model_rejects_unknown() {
        let (srv, token) = app();
        let resp = req(srv, "POST", "/v1/asr/model", &token, r#"{"repo":"nope"}"#).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn asr_chunk_persists_and_models_use_repo_field() {
        let _guard = ENV_LOCK.lock().unwrap();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!("capture-daemon-asr-{nanos}"));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("CAPTURE_CONFIG", tmp.join("config.json"));
        std::env::set_var("CAPTURE_ASR_MODELS_DIR", &tmp);

        // Setting the chunk length persists and echoes it back.
        let (srv, token) = app();
        let resp = req(srv, "POST", "/v1/asr/chunk", &token, r#"{"seconds":8}"#).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await["chunk_seconds"], json!(8.0));

        // The catalog reflects the setting and speaks the GUI's `repo` wire field.
        let (srv2, token2) = app();
        let resp2 = req(srv2, "GET", "/v1/asr/models", &token2, "").await;
        assert_eq!(resp2.status(), StatusCode::OK);
        let m = body_json(resp2).await;
        assert_eq!(m["chunk_seconds"], json!(8.0));
        let models = m["models"].as_array().expect("models array");
        assert!(models.iter().all(|x| x.get("repo").is_some() && x.get("size_label").is_some()));
        assert!(models.iter().any(|x| x["repo"] == json!("base.en")));

        std::env::remove_var("CAPTURE_CONFIG");
        std::env::remove_var("CAPTURE_ASR_MODELS_DIR");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn permissions_reports_known_states() {
        let (srv, token) = app();
        let resp = req(srv, "GET", "/v1/permissions", &token, "").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert!(v["platform"].is_string());
        let known = ["granted", "denied", "undetermined", "not_applicable", "unknown"];
        assert!(known.contains(&v["screen_recording"].as_str().unwrap_or("")));
        assert!(known.contains(&v["microphone"].as_str().unwrap_or("")));
    }

    #[tokio::test]
    async fn audio_mics_returns_a_devices_array() {
        let (srv, token) = app();
        let resp = req(srv, "GET", "/v1/audio/mics", &token, "").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_json(resp).await["devices"].is_array());
    }
}
