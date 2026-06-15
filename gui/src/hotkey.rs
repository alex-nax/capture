//! Global hotkey (⌃⌘R) to toggle capture from anywhere — registered on the main
//! thread; events drained from the GPUI tray loop in app.rs. On macOS this uses
//! Carbon `RegisterEventHotKey` (no accessibility permission needed).

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyManager,
};

pub const LABEL: &str = "⌃⌘R";

/// Register the toggle hotkey. Returns the manager (keep it alive — dropping it
/// unregisters) and the hotkey id to match against events. None if registration
/// fails (e.g. another app already owns the combo).
pub fn build() -> Option<(GlobalHotKeyManager, u32)> {
    let mgr = GlobalHotKeyManager::new().ok()?;
    let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SUPER), Code::KeyR);
    let id = hk.id();
    mgr.register(hk).ok()?;
    Some((mgr, id))
}
