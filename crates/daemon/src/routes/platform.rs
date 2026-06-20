//! Platform read-layer handlers (capture-platform): windows, audio mics, permissions.

use super::*;

#[derive(Deserialize)]
pub(crate) struct WindowsQuery {
    pid: Option<i64>,
    app_name: Option<String>,
}

/// `GET /v1/windows[?pid=&app_name=]` — on-screen top-level windows for the picker, largest first.
/// Mirrors `core.list_windows`; 503 if the OS query fails (on macOS, Screen Recording not granted).
pub(crate) async fn windows(Query(q): Query<WindowsQuery>) -> Result<Json<Value>, ApiError> {
    let pid = q.pid.map(|p| p as i32);
    let app = q.app_name.clone();
    // SCShareableContent queries the window server — run it off the async worker.
    let wins = tokio::task::spawn_blocking(move || capture_platform::list_windows(pid, app.as_deref()))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("windows task: {e}")))?
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e))?;
    let count = wins.len();
    Ok(Json(json!({ "windows": wins, "count": count })))
}

/// `GET /v1/audio/mics` — available microphones (the system default flagged). Mirrors the platform's
/// `list_input_devices`.
pub(crate) async fn audio_mics() -> Json<Value> {
    let devices = tokio::task::spawn_blocking(capture_platform::audio_input_devices)
        .await
        .unwrap_or_default();
    Json(json!({ "devices": devices }))
}

/// `GET /v1/permissions` — Screen-Recording + Microphone TCC status. Mirrors `permissions.status`.
pub(crate) async fn permissions() -> Json<Value> {
    Json(json!({
        "platform": platform_str(),
        "screen_recording": capture_platform::screen_recording_status(),
        "microphone": capture_platform::microphone_status(),
    }))
}

#[derive(Deserialize)]
struct PermissionRequest {
    kind: Option<String>,
}

/// `POST /v1/permissions/request {kind}` — report the status for `kind` WITHOUT prompting. The daemon
/// must never trigger the dialog (it aborts this headless process; the GUI prompts). Mirrors
/// `permissions.request`; `kind` defaults to `screen_recording`.
pub(crate) async fn permissions_request(body: Bytes) -> Result<Json<Value>, ApiError> {
    let req: PermissionRequest = parse_json(&body)?;
    match req.kind.as_deref().unwrap_or("screen_recording") {
        "screen_recording" => Ok(Json(json!({
            "platform": platform_str(),
            "screen_recording": capture_platform::screen_recording_status(),
        }))),
        "microphone" => Ok(Json(json!({
            "platform": platform_str(),
            "microphone": capture_platform::microphone_status(),
        }))),
        other => Err(ApiError::bad(format!("unknown permission {other:?}"))),
    }
}
