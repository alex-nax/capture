//! `captured` — the v3 capture daemon entry point.
//!
//! Starts the axum `/v1` server (single-instance guarded), writes the `~/.capture/daemon.json`
//! discovery file, and serves until SIGINT/SIGTERM or `POST /v1/admin/shutdown`. Logs to stderr.

fn main() {
    if let Err(e) = capture_daemon::run() {
        eprintln!("captured: fatal: {e}");
        std::process::exit(1);
    }
}
