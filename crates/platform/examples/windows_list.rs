//! Smoke runner for the Windows window-enumeration backend (#66 slice A):
//!   cargo run -p capture-platform --example windows_list -- [app-name-filter]
//! Prints the on-screen top-level windows (largest first), optionally filtered by app-name substring.

fn main() {
    let filter = std::env::args().nth(1);
    match capture_platform::list_windows(None, filter.as_deref()) {
        Ok(ws) => {
            eprintln!("{} window(s):", ws.len());
            for w in &ws {
                println!(
                    "  id={:<10} pid={:<7} {:<22} {}x{}  {}",
                    w.window_id, w.pid, w.app_name, w.width, w.height, w.title
                );
            }
        }
        Err(e) => {
            eprintln!("list_windows error: {e}");
            std::process::exit(1);
        }
    }
}
