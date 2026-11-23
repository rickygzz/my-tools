//! **Ctrl+Alt+C** in every app shows the calculator, through the `global-hotkey` crate: it
//! registers the keys with the system (`RegisterHotKey` on Windows, Carbon hot keys on macOS,
//! a key grab on X11), so other apps don't also get them.
//!
//! Wayland doesn't let apps register keys this way; there, the tray menu opens the calculator.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// Registers Ctrl+Alt+C; `pressed` runs, on another thread, each time it's pressed. The keys
/// stay registered while the returned manager is kept.
pub fn register(pressed: impl Fn() + Send + Sync + 'static) -> Option<GlobalHotKeyManager> {
    if is_wayland() {
        eprintln!("calculator: Wayland has no shortcuts for apps; open it from the tray menu");
        return None;
    }

    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyC);
    let manager = match GlobalHotKeyManager::new().and_then(|manager| {
        manager.register(hotkey)?;
        Ok(manager)
    }) {
        Ok(manager) => manager,
        Err(error) => {
            // Usually another app has the same keys.
            eprintln!("calculator: Ctrl+Alt+C isn't available: {error}");
            return None;
        }
    };

    let id = hotkey.id();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == id && event.state == HotKeyState::Pressed {
            pressed();
        }
    }));
    Some(manager)
}

fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland"))
}
