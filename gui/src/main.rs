//! capture-gui: native GPUI desktop client for the capture-mcp daemon.

mod app;
mod daemon;
mod hotkey;
mod skill;
mod tray;

use app::CaptureApp;
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

    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.0), px(680.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| CaptureApp::new(cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
