// Release builds on Windows are a GUI app (no console window; closing a stray console must not
// kill the app). Debug keeps the console for dev diagnostics. No-op on non-Windows targets.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! capture-gui: native GPUI desktop client for the capture-mcp daemon.

mod app;
mod assets;
mod daemon;
mod hotkey;
mod skill;
mod tray;
mod update;

use app::CaptureApp;
use assets::Assets;
use gpui::{px, size, App, AppContext, Application, Bounds, WindowBounds, WindowOptions};

fn main() {
    // Headless affordance: `capture-gui --install-skill ["Claude Code"|"Codex"]`
    // installs the bundled skill and exits (same action as the GUI button).
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--skill-status") {
        for agent in skill::AGENTS {
            let s = match skill::status(agent) {
                skill::SkillStatus::NotInstalled => "not installed",
                skill::SkillStatus::UpToDate => "up to date",
                skill::SkillStatus::UpdateAvailable => "update available",
            };
            println!("{}: {}", agent.label, s);
        }
        std::process::exit(0);
    }
    if let Some(pos) = args.iter().position(|a| a == "--install-skill") {
        let label = args.get(pos + 1).map(String::as_str).unwrap_or("Claude Code");
        match skill::AGENTS.iter().find(|a| a.label.eq_ignore_ascii_case(label)) {
            Some(agent) => match skill::install(agent) {
                Ok(path) => {
                    println!("installed the capture skill -> {}", path.display());
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("skill install failed: {e}");
                    std::process::exit(1);
                }
            },
            None => {
                eprintln!("unknown agent {label:?}; known: Claude Code, Codex");
                std::process::exit(2);
            }
        }
    }

    Application::new().with_assets(Assets).run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.0), px(680.0)), cx);
        if let Err(e) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| CaptureApp::new(cx)),
        ) {
            eprintln!("capture-gui: could not open a window / create the renderer: {e:?}");
            #[cfg(target_os = "windows")]
            eprintln!(
                "capture-gui: the GPU/DirectX renderer needs the interactive desktop (a logged-on \
                 session). Launch it from your desktop or via the tray agent — not from a \
                 service/SSH/non-interactive context."
            );
            cx.quit();
            return;
        }
        cx.activate(true);
        // Launched by the native menu-bar agent: this process is *just* the window.
        // GPUI doesn't quit on last-window-close, so exit explicitly when the window
        // closes — that way each agent "Open Window" is a fresh, non-lingering window
        // and the persistent menu-bar presence is the agent's job, not ours.
        if std::env::var_os("CAPTURE_AGENT").is_some() {
            cx.on_window_closed(|cx| cx.quit()).detach();
        }
    });
}
