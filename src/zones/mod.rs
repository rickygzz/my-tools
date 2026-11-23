//! Window zones: areas of each monitor that windows snap into. Hotkeys move the active window
//! to a zone, or to the next one; Shift while dragging a window shows them (a later step).
//!
//! | File          | What it does                                                         |
//! |---------------|----------------------------------------------------------------------|
//! | `layout.rs`   | zones in percent of a monitor, layouts, the editor's templates       |
//! | `geometry.rs` | where the zones are on a monitor, which one a point or window is in  |
//! | `config.rs`   | reads and writes `zones.cfg`                                         |
//! | `desktop.rs`  | the [`desktop::Desktop`] trait: what the zones need from the OS      |
//! | `mover.rs`    | moves the active window to a zone, the next one, or another monitor  |
//! | `win32.rs`    | [`desktop::Desktop`] on Windows                                      |
//! | `macos.rs`    | on macOS, through the Accessibility API                              |
//! | `x11.rs`      | on Linux X11, through the window manager (Wayland can't move windows)|
//!
//! All but the last three are plain Rust, the same on every OS, and tested with a fake
//! desktop.

pub mod config;
pub mod desktop;
pub mod geometry;
pub mod layout;
pub mod mover;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod win32;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "macos")]
use macos as system;
#[cfg(windows)]
use win32 as system;
#[cfg(target_os = "linux")]
use x11 as system;

use layout::Layouts;

/// What a hotkey asks of the zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Move the active window to this zone, from 1.
    MoveTo(usize),
    Next,
    /// Move the active window to the monitor the mouse pointer is on.
    ToMouse,
    Previous,
}

/// Runs the zone hotkeys, reading `zones.cfg` each time so a change applies at once.
#[derive(Default)]
pub struct Zones {
    /// The file's problems reported last, so the same ones aren't reported on every press.
    reported: String,
}

impl Zones {
    pub fn run(&mut self, command: Command) {
        if crate::is_wayland() {
            report("Window zones don't work on Wayland: it doesn't let apps move other apps' windows.");
            return;
        }
        let layouts = self.layouts();
        let moved = system::SystemDesktop::new().and_then(|mut desktop| match command {
            Command::MoveTo(number) => mover::move_to(&mut desktop, &layouts, number),
            Command::Next => mover::move_by(&mut desktop, &layouts, true),
            Command::Previous => mover::move_by(&mut desktop, &layouts, false),
            Command::ToMouse => mover::move_to_pointer(&mut desktop),
        });
        if let Err(problem) = moved {
            report(&problem);
        }
    }

    /// The layouts in `zones.cfg`, creating it the first time; the default ones when it can't
    /// be used. Its problems are reported once, until they change.
    fn layouts(&mut self) -> Layouts {
        let Some(path) = config::file_path() else {
            return Layouts::default();
        };
        let result = config::load(&path);
        let problems = result.problems_text();
        if !problems.is_empty() && problems != self.reported {
            report(&format!("Some window zones in {} are off:\n{problems}", path.display()));
        }
        self.reported = problems;
        result.layouts
    }
}

/// Tells the user why something didn't work. For now on stderr; a notification would be better.
fn report(problem: &str) {
    eprintln!("zones: {problem}");
}
