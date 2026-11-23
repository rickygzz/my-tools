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
    /// Move the active window to this zone, from 1 to [`ZONE_KEYS`].
    Zone(u8),
    NextZone,
    /// Move the active window to the monitor the mouse pointer is on.
    WindowToMouse,
    /// Keep the active window on top of the others, or no longer.
    AlwaysOnTop,
    PreviousZone,
}

/// How many zones have a shortcut of their own: 1 to 9.
const ZONE_KEYS: usize = 9;

impl Action {
    pub const ALL: [Action; 6 + ZONE_KEYS] = [
        Action::Calculator,
        Action::Spotlight,
        Action::NextZone,
        Action::PreviousZone,
        Action::WindowToMouse,
        Action::AlwaysOnTop,
        Action::Zone(1),
        Action::Zone(2),
        Action::Zone(3),
        Action::Zone(4),
        Action::Zone(5),
        Action::Zone(6),
        Action::Zone(7),
        Action::Zone(8),
        Action::Zone(9),
    ];

    /// Whether it moves windows into zones, which Wayland doesn't allow.
    pub fn is_zone(self) -> bool {
        matches!(self, Action::Zone(_) | Action::NextZone | Action::PreviousZone | Action::WindowToMouse | Action::AlwaysOnTop)
    }

    /// Its name in `--show <name>`, and its ID with the portal (the desktop remembers the
    /// user's keys under it, so don't rename it).
    pub fn id(self) -> &'static str {
        const ZONES: [&str; ZONE_KEYS] =
            ["zone-1", "zone-2", "zone-3", "zone-4", "zone-5", "zone-6", "zone-7", "zone-8", "zone-9"];
        match self {
            Action::Calculator => "calculator",
            Action::Spotlight => "spotlight",
            Action::Zone(number) => ZONES[usize::from(number) - 1],
            Action::NextZone => "next-zone",
            Action::PreviousZone => "previous-zone",
            Action::WindowToMouse => "window-to-mouse",
            Action::AlwaysOnTop => "always-on-top",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }

    /// Shown in the desktop's shortcut settings.
    fn description(self) -> &'static str {
        const ZONES: [&str; ZONE_KEYS] = [
            "Move the window to zone 1",
            "Move the window to zone 2",
            "Move the window to zone 3",
            "Move the window to zone 4",
            "Move the window to zone 5",
            "Move the window to zone 6",
            "Move the window to zone 7",
            "Move the window to zone 8",
            "Move the window to zone 9",
        ];
        match self {
            Action::Calculator => "Show the calculator",
            Action::Spotlight => "Show the mouse spotlight",
            Action::Zone(number) => ZONES[usize::from(number) - 1],
            Action::NextZone => "Move the window to the next zone",
            Action::PreviousZone => "Move the window to the previous zone",
            Action::WindowToMouse => "Move the window to the screen the mouse is on",
            Action::AlwaysOnTop => "Keep the window on top, or no longer",
        }
    }

    /// The suggested keys: modifiers, then one key, joined with `+`. On Wayland the user
    /// can pick other keys in the desktop's dialog.
    ///
    /// The zones use Ctrl+Alt+Win (Ctrl+Option+Command on macOS): Ctrl+Alt alone is AltGr on
    /// many keyboards, where AltGr with a digit types a character such as @ or #.
    fn keys(self) -> &'static str {
        const ZONES: [&str; ZONE_KEYS] = [
            "Ctrl+Alt+Super+1",
            "Ctrl+Alt+Super+2",
            "Ctrl+Alt+Super+3",
            "Ctrl+Alt+Super+4",
            "Ctrl+Alt+Super+5",
            "Ctrl+Alt+Super+6",
            "Ctrl+Alt+Super+7",
            "Ctrl+Alt+Super+8",
            "Ctrl+Alt+Super+9",
        ];
        match self {
            Action::Calculator => "Ctrl+Alt+C",
            Action::Spotlight => "Ctrl+Alt+P",
            Action::Zone(number) => ZONES[usize::from(number) - 1],
            Action::NextZone => "Ctrl+Alt+Super+Right",
            Action::PreviousZone => "Ctrl+Alt+Super+Left",
            Action::WindowToMouse => "Ctrl+Alt+Super+M",
            Action::AlwaysOnTop => "Ctrl+Alt+Super+T",
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
