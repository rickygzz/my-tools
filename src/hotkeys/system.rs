//! Shortcuts through the `global-hotkey` crate: it registers the keys with the system
//! (`RegisterHotKey` on Windows, Carbon hot keys on macOS, a key grab on X11), so other apps
//! don't also get them.

use std::collections::HashMap;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use super::{Action, Pressed};

/// Registers each action's keys. An action whose keys another app already has is skipped.
pub fn register(actions: &[Action], pressed: Pressed) -> Option<GlobalHotKeyManager> {
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(error) => {
            eprintln!("hotkeys: can't register keyboard shortcuts: {error}");
            return None;
        }
    };

    let mut registered = HashMap::new();
    for &action in actions {
        let result = action
            .keys()
            .parse::<HotKey>()
            .map_err(|e| e.to_string())
            .and_then(|hotkey| manager.register(hotkey).map(|()| hotkey).map_err(|e| e.to_string()));
        match result {
            Ok(hotkey) => {
                registered.insert(hotkey.id(), action);
            }
            // Usually another app has the same keys.
            Err(error) => eprintln!("hotkeys: {} ({}) isn't available: {error}", action.keys(), action.description()),
        }
    }

    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state == HotKeyState::Pressed
            && let Some(&action) = registered.get(&event.id)
        {
            pressed(action, None);
        }
    }));
    Some(manager)
}
