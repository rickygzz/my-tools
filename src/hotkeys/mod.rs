//! Keyboard shortcuts that work in every app, such as **Ctrl+Alt+C** for the calculator.
//!
//! Each OS has its own way to register them, so this module picks one when the app starts.
//! Everything else in the app only sees [`Action`]s.
//!
//! | Where                            | How                                      | File        |
//! |----------------------------------|------------------------------------------|-------------|
//! | Windows, macOS, Linux X11        | the `global-hotkey` crate                | `system.rs` |
//! | Linux Wayland (GNOME 48+, KDE)   | the desktop's *GlobalShortcuts* portal   | `portal.rs` |
//! | Linux, any desktop               | `my-tools --show <action>` over D-Bus    | `remote.rs` |
//!
//! Wayland doesn't let apps grab keys. Instead the app asks the desktop through the portal,
//! and the desktop stays in charge of them: the first time, it shows a dialog where the user
//! confirms or changes the suggested keys. Desktops without the portal (Sway and other
//! wlroots ones) can still bind a command such as `my-tools --show calculator` to any keys
//! in their own settings. That command tells the running copy over D-Bus.

#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod remote;
mod system;

use std::sync::Arc;

/// Something a shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Calculator,
    Spotlight,
}

impl Action {
    pub const ALL: [Action; 2] = [Action::Calculator, Action::Spotlight];

    /// Its name in `--show <name>`, and its ID with the portal (the desktop remembers the
    /// user's keys under it, so don't rename it).
    pub fn id(self) -> &'static str {
        match self {
            Action::Calculator => "calculator",
            Action::Spotlight => "spotlight",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }

    /// Shown in the desktop's shortcut settings.
    fn description(self) -> &'static str {
        match self {
            Action::Calculator => "Show the calculator",
            Action::Spotlight => "Show the mouse spotlight",
        }
    }

    /// The suggested keys: modifiers, then one key, joined with `+`. On Wayland the user
    /// can pick other keys in the desktop's dialog.
    fn keys(self) -> &'static str {
        match self {
            Action::Calculator => "Ctrl+Alt+C",
            Action::Spotlight => "Ctrl+Alt+P",
        }
    }
}

/// Called, on some other thread, with the action and, on Wayland, the press's activation
/// token. A new window needs that token to be allowed to take the keyboard focus.
type Pressed = Arc<dyn Fn(Action, Option<String>) + Send + Sync>;

/// The registered shortcuts. They stay registered while this is kept.
pub struct Hotkeys {
    _system: Option<global_hotkey::GlobalHotKeyManager>,
    #[cfg(target_os = "linux")]
    _remote: Option<zbus::blocking::Connection>,
}

/// Registers a shortcut for each of `actions`, and calls `pressed` when one is pressed.
///
/// On Linux, `my-tools --show <action>` works for every action, bound to keys or not.
pub fn start(actions: &[Action], pressed: impl Fn(Action, Option<String>) + Send + Sync + 'static) -> Hotkeys {
    let pressed: Pressed = Arc::new(pressed);

    #[cfg(target_os = "linux")]
    let remote = remote::serve(pressed.clone());

    let system = if crate::is_wayland() {
        #[cfg(target_os = "linux")]
        portal::start(actions.to_vec(), pressed);
        None
    } else {
        system::register(actions, pressed)
    };

    Hotkeys {
        _system: system,
        #[cfg(target_os = "linux")]
        _remote: remote,
    }
}

/// Handles `my-tools --show <action>`: asks the running copy to do it, and returns the exit
/// code. Returns `None` if the program wasn't started that way.
pub fn run_command_line() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next()? != "--show" {
        return None;
    }
    let names = Action::ALL.map(Action::id).join(", ");
    let Some(action) = args.next().as_deref().and_then(Action::from_id) else {
        eprintln!("usage: my-tools --show <action>, where <action> is one of: {names}");
        return Some(2);
    };

    #[cfg(target_os = "linux")]
    let result = remote::send(action);
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), String> = {
        let _ = action;
        Err("--show only works on Linux; use the keyboard shortcut".into())
    };

    match result {
        Ok(()) => Some(0),
        Err(error) => {
            eprintln!("my-tools: {error}");
            Some(1)
        }
    }
}

/// The command that does `action`, to bind to keys in the desktop's settings.
#[cfg(target_os = "linux")]
fn command_for(action: Action) -> String {
    let exe = std::env::current_exe().ok();
    let exe = exe.as_deref().and_then(|path| path.to_str()).unwrap_or("my-tools");
    format!("{exe} --show {}", action.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for action in Action::ALL {
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("nope"), None);
    }

    #[test]
    fn suggested_keys_parse() {
        for action in Action::ALL {
            assert!(action.keys().parse::<global_hotkey::hotkey::HotKey>().is_ok(), "{action:?}");
        }
    }
}
