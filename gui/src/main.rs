//! capture-gui: native GPUI desktop client for the capture-mcp daemon.

mod app;
mod daemon;
mod hotkey;
mod tray;

use app::CaptureApp;
use gpui::{px, size, App, AppContext, Application, Bounds, WindowBounds, WindowOptions};

fn main() {
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
