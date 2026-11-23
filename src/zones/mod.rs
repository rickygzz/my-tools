//! Window zones: areas of each monitor that windows snap into. Shift (or a right click) while
//! dragging a window shows them, and letting go over one puts the window in it; hotkeys move
//! the active window to a zone, or to the next one.
//!
//! | File           | What it does                                                          |
//! |----------------|-----------------------------------------------------------------------|
//! | `layout.rs`    | zones in percent of a monitor, layouts, the editor's templates        |
//! | `geometry.rs`  | where the zones are on a monitor, which one a point or window is in   |
//! | `config.rs`    | reads and writes `zones.cfg`                                          |
//! | `desktop.rs`   | the [`desktop::Desktop`] trait: what the zones need from the OS       |
//! | `mover.rs`     | moves the active window to a zone, the next one, or another monitor   |
//! | `drag.rs`      | what a drag shows and where the window goes                           |
//! | `paint.rs`     | the overlay as rectangles: zones, borders, seven-segment numbers      |
//! | `win32*.rs`    | Windows: the desktop; watching drags and the overlay                  |
//! | `macos*.rs`    | macOS, through the Accessibility API                                  |
//! | `x11*.rs`      | Linux X11, through the window manager (Wayland can't move windows)    |
//!
//! All but the OS files are plain Rust, the same on every OS, and tested with fakes.

pub mod config;
pub mod desktop;
pub mod drag;
mod editor;
pub mod geometry;
pub mod layout;
pub mod mover;
pub mod paint;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_drag;
#[cfg(windows)]
mod win32;
#[cfg(windows)]
mod win32_drag;
#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
mod x11_drag;

#[cfg(target_os = "macos")]
use {macos as system, macos_drag as system_drag};
#[cfg(windows)]
use {win32 as system, win32_drag as system_drag};
#[cfg(target_os = "linux")]
use {x11 as system, x11_drag as system_drag};

use std::time::Instant;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use desktop::Desktop as _;
use drag::{DragPresenter, Overlay, ShownZone, Signal};
use layout::Layouts;

/// What a hotkey asks of the zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Move the active window to this zone, from 1.
    MoveTo(usize),
    Next,
    Previous,
    /// Move the active window to the monitor the mouse pointer is on.
    ToMouse,
    /// Keep the active window on top of the others, or no longer.
    ToggleOnTop,
}

/// Starts telling of drags of other apps' windows, with `notify` called (on any thread) for
/// each [`Signal`]. Nothing on Wayland, where apps can't move other apps' windows.
pub fn start_watching(notify: impl Fn(Signal) + Send + Sync + 'static) {
    if !crate::is_wayland() {
        system_drag::start_watching(notify);
    }
}

/// Runs the zone hotkeys and drags, reading `zones.cfg` each time so a change applies at once.
#[derive(Default)]
pub struct Zones {
    /// The file's problems reported last, so the same ones aren't reported on every press.
    reported: String,
    drag: DragPresenter,
    /// Made at the first drag; `None` until then, or when it can't be made.
    overlay: Option<system_drag::Overlay>,
    /// The zone editor, while it's open.
    editor: Option<editor::Editor>,
}

impl Zones {
    pub fn run(&mut self, command: Command) {
        if crate::is_wayland() {
            report("Window zones don't work on Wayland: it doesn't let apps move other apps' windows.");
            return;
        }
        if command == Command::ToggleOnTop {
            match system::toggle_on_top() {
                Ok((title, on)) => {
                    let window = if title.is_empty() { "The window".to_owned() } else { title };
                    let body = if on { format!("{window} stays on top of the other windows.") } else { format!("{window} is no longer on top.") };
                    crate::toast::post("Always on top", &body);
                }
                Err(problem) => crate::toast::post("Always on top", &problem),
            }
            return;
        }
        let layouts = layouts(&mut self.reported);
        let moved = system::SystemDesktop::new().and_then(|mut desktop| match command {
            Command::MoveTo(number) => mover::move_to(&mut desktop, &layouts, number),
            Command::Next => mover::move_by(&mut desktop, &layouts, true),
            Command::Previous => mover::move_by(&mut desktop, &layouts, false),
            Command::ToMouse => mover::move_to_pointer(&mut desktop),
            Command::ToggleOnTop => Ok(()),
        });
        if let Err(problem) = moved {
            report(&problem);
        }
    }

    pub fn drag(&mut self, event_loop: &ActiveEventLoop, signal: Signal) {
        if matches!(signal, Signal::Started(_)) && self.overlay.is_none() {
            match system_drag::Overlay::new(event_loop) {
                Ok(overlay) => self.overlay = Some(overlay),
                Err(error) => eprintln!("zones: could not make the overlay: {error}"),
            }
        }
        let mut missing = NoOverlay;
        let overlay: &mut dyn Overlay = match &mut self.overlay {
            Some(overlay) => overlay,
            None => &mut missing,
        };
        let reported = &mut self.reported;
        match signal {
            Signal::Started(window) => self.drag.started(window, overlay),
            Signal::Moved(pointer) => self.drag.moved(pointer, overlay, || {
                let layouts = layouts(reported);
                let monitors = system::monitors();
                drag::zones_of(&monitors, &layouts)
            }),
            Signal::Ended => {
                if let Some((window, zone)) = self.drag.ended(overlay) {
                    let moved = system::SystemDesktop::new()
                        .and_then(|mut desktop| desktop.move_window(system_drag::window(window), zone));
                    if let Err(problem) = moved {
                        report(&problem);
                    }
                }
            }
        }
    }

    /// Opens the zone editor, or brings it to the front if it's open.
    pub fn edit(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(editor) = &self.editor {
            editor.focus();
            return;
        }
        match editor::Editor::open(event_loop, system::monitors()) {
            Ok(editor) => self.editor = Some(editor),
            Err(error) => report(&format!("Could not open the window zones editor: {error}")),
        }
    }

    pub fn owns(&self, id: WindowId) -> bool {
        self.overlay.as_ref().is_some_and(|overlay| overlay.owns(id))
            || self.editor.as_ref().is_some_and(|editor| editor.id() == id)
    }

    pub fn window_event(&mut self, id: WindowId, event: &WindowEvent) {
        if let Some(editor) = &mut self.editor
            && editor.id() == id
        {
            if !editor.window_event(event) {
                self.editor = None;
            }
        } else if let Some(overlay) = &mut self.overlay {
            overlay.window_event(event);
        }
    }

    /// When the editor wants to be drawn again (a blinking caret), if it does.
    pub fn next_wake(&self) -> Option<Instant> {
        self.editor.as_ref().and_then(editor::Editor::next_wake)
    }

    pub fn wake(&mut self, now: Instant) {
        if let Some(editor) = &mut self.editor {
            editor.wake(now);
        }
    }

    /// Closes the windows while the event loop still exists.
    pub fn close(&mut self) {
        self.overlay = None;
        self.editor = None;
    }
}

/// Stands in for an overlay that couldn't be made: a drag can still put the window in a zone,
/// unseen.
struct NoOverlay;

impl Overlay for NoOverlay {
    fn show(&mut self, _zones: &[ShownZone], _highlighted: Option<usize>) {}
    fn hide(&mut self) {}
}

/// The layouts in `zones.cfg`, creating it the first time; the default ones when it can't be
/// used. Its problems are reported once, until they change (`reported` keeps the last ones).
fn layouts(reported: &mut String) -> Layouts {
    let Some(path) = config::file_path() else {
        return Layouts::default();
    };
    let result = config::load(&path);
    let problems = result.problems_text();
    if !problems.is_empty() && problems != *reported {
        report(&format!("Some window zones in {} are off:\n{problems}", path.display()));
    }
    *reported = problems;
    result.layouts
}

/// Tells the user why something didn't work, in a toast.
fn report(problem: &str) {
    crate::toast::post("Window zones", problem);
}
