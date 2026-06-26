//! capture-core — the v3 contract layer.
//!
//! Defines the serde types for the `/v1` HTTP API (requests + responses) and the on-disk
//! session formats. These replace the v2 pydantic `daemon/models.py` + the `v1_schema` golden
//! as the SOURCE OF TRUTH for the contract firewall: the GUI (and the future Rust daemon / MCP)
//! depend on these, so the wire + on-disk shapes stay byte-identical across the incremental port.
//!
//! See `docs/specs/v3-architecture.md`. The contract type modules (`v1` requests/responses,
//! `ondisk` session formats) land in #61's type-port phase.

pub mod frames;
pub mod sessions;
pub mod time;
pub mod transcript;
pub mod v1;

/// The `/v1` API version this contract describes (matches `HealthResponse.api_version`).
pub const API_VERSION: u32 = 1;

/// Exit this process when its parent (the CaptureBar agent that launched it) dies — the macOS analog
/// of the Windows tray agent's kill-on-close Job Object, so closing the agent closes the whole app
/// (window + daemon), and a force-quit/crash of the agent doesn't leave orphans. Call once at startup;
/// it self-gates on `CAPTURE_AGENT` (only the agent sets it — a CLI-started daemon keeps running) and
/// is a no-op off Unix (Windows uses the Job Object). Detects the parent's death via `getppid()`
/// changing (the kernel reparents an orphan to launchd/init).
pub fn exit_when_parent_dies() {
    #[cfg(unix)]
    {
        if std::env::var_os("CAPTURE_AGENT").is_none() {
            return;
        }
        let parent = unsafe { libc::getppid() };
        if parent <= 1 {
            return; // already orphaned / not actually under an agent
        }
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if unsafe { libc::getppid() } != parent {
                std::process::exit(0);
            }
        });
    }
}
