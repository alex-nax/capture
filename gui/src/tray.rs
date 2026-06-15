//! macOS menu-bar (status-item) presence via tray-icon + muda.
//!
//! Created on the main thread inside the GPUI run loop; its menu-event channel
//! (`muda::MenuEvent::receiver()`) is drained from the GPUI tray loop in app.rs.
//! Title-only (no icon asset): "● capture" idle, "⦿ N" while N captures run.

use muda::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

pub const ID_OPEN: &str = "open";
pub const ID_STOP_ALL: &str = "stop_all";
pub const ID_QUIT: &str = "quit";

pub struct Tray {
    tray: TrayIcon,
    last_title: String,
}

/// Build the status item + menu. None if the platform/AppKit init fails.
pub fn build() -> Option<Tray> {
    let menu = Menu::new();
    let open = MenuItem::with_id(ID_OPEN, "Open capture window", true, None);
    let stop_all = MenuItem::with_id(ID_STOP_ALL, "Stop all captures", true, None);
    let quit = MenuItem::with_id(ID_QUIT, "Quit", true, None);
    let sep1 = PredefinedMenuItem::separator();
    let sep2 = PredefinedMenuItem::separator();
    menu.append_items(&[&open, &sep1, &stop_all, &sep2, &quit]).ok()?;

    let tray = TrayIconBuilder::new()
        .with_id("capture")
        .with_menu(Box::new(menu))
        .with_title("● capture")
        .with_tooltip("capture")
        .build()
        .ok()?;
    Some(Tray {
        tray,
        last_title: "● capture".into(),
    })
}

impl Tray {
    /// Reflect the running-capture count in the menu-bar title (idempotent).
    pub fn set_running(&mut self, n: usize) {
        let title = if n == 0 {
            "● capture".to_string()
        } else {
            format!("⦿ {n}")
        };
        if title != self.last_title {
            let _ = self.tray.set_title(Some(&title));
            self.last_title = title;
        }
    }
}
