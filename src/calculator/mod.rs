//! The calculator: a small window, opened from the tray menu or with **Ctrl+Alt+C**, that
//! calculates the line typed in it: `2+2*3`, `10 km to mi`, `today + 30 d`,
//! `10:30 Mexico to India`, `x = 5`.
//!
//! | File           | What it does                                                    |
//! |----------------|-----------------------------------------------------------------|
//! | `engine/`      | turns a line into a result; plain Rust, no window               |
//! | `suggester.rs` | what to offer while typing (places, units, functions, names)    |
//! | `presenter.rs` | the window's behaviour: history, names, Up/Down, suggestions    |
//! | `view.rs`      | draws the window with egui, and turns keys into presenter calls |
//! | `window.rs`    | the winit window egui draws into, through wgpu                  |
//! | `hotkey.rs`    | Ctrl+Alt+C in every app                                         |
//!
//! The window is made the first time it's shown, then hidden and shown again, so the history
//! and the names given values last until the app exits.

pub mod engine;
mod hotkey;
mod presenter;
mod suggester;
mod view;
mod window;

use std::time::Instant;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

pub use hotkey::register as register_hotkey;
use presenter::Presenter;
use window::{Screen, Window};

pub struct Calculator {
    presenter: Presenter,
    window: Option<Window>,
}

impl Default for Calculator {
    fn default() -> Self {
        Self { presenter: Presenter::new(engine::Calculator::default(), chrono::Utc::now), window: None }
    }
}

impl Calculator {
    /// Shows the window on the screen the mouse is on, ready to type.
    pub fn show(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            match Window::new(event_loop) {
                Ok(window) => self.window = Some(window),
                Err(error) => {
                    eprintln!("calculator: could not open its window: {error}");
                    return;
                }
            }
        }
        self.presenter.shown();
        if let Some(window) = &mut self.window
            && let Err(error) = window.show(event_loop, screen_under_cursor(event_loop))
        {
            eprintln!("calculator: could not open its window again: {error}");
        }
    }

    pub fn owns(&self, id: WindowId) -> bool {
        self.window.as_ref().is_some_and(|window| window.id() == Some(id))
    }

    pub fn window_event(&mut self, event: &WindowEvent) {
        if let Some(window) = &mut self.window {
            window.window_event(&mut self.presenter, event);
        }
    }

    /// When the window wants to be drawn again (a blinking caret), if it does.
    pub fn next_wake(&self) -> Option<Instant> {
        self.window.as_ref().and_then(Window::next_wake)
    }

    pub fn wake(&mut self, now: Instant) {
        if let Some(window) = &mut self.window {
            window.wake(now);
        }
    }

    /// Closes the window while the event loop still exists.
    pub fn close(&mut self) {
        self.window = None;
    }
}

/// The screen the mouse pointer is on, or the main one. `None` when the system doesn't say
/// (Wayland, where apps can't place their windows anyway).
fn screen_under_cursor(event_loop: &ActiveEventLoop) -> Option<Screen> {
    let (x, y) = crate::spotlight::cursor_position();
    let (x, y) = (f64::from(x), f64::from(y));

    // macOS gives the pointer in points from the bottom of the main screen; the monitors are
    // in pixels from its top.
    #[cfg(target_os = "macos")]
    let (y, in_points) = {
        let main_height = event_loop
            .primary_monitor()
            .map_or(0.0, |main| main.size().to_logical::<f64>(main.scale_factor()).height);
        (main_height - y, true)
    };
    #[cfg(not(target_os = "macos"))]
    let in_points = false;

    let under_cursor = event_loop.available_monitors().find(|monitor| {
        let scale = if in_points { monitor.scale_factor() } else { 1.0 };
        let (position, size) = (monitor.position(), monitor.size());
        let (left, top) = (f64::from(position.x) / scale, f64::from(position.y) / scale);
        let (width, height) = (f64::from(size.width) / scale, f64::from(size.height) / scale);
        (left..left + width).contains(&x) && (top..top + height).contains(&y)
    });

    under_cursor.or_else(|| event_loop.primary_monitor()).map(|monitor| (monitor.position(), monitor.size()))
}
