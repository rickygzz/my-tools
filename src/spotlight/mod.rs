//! Mouse spotlight: tap **Left Ctrl twice** to dim every screen except a circle around the
//! pointer. The circle starts large and shrinks onto the pointer so the eye follows it there,
//! then follows the pointer. It goes away at the next key or click, or once the pointer has
//! stayed still for a few seconds.
//!
//! The timing rules (`detector`) and the animation (`presenter`) are plain Rust and work on
//! any OS. Listening to keys in every program and drawing the overlay need OS-specific code:
//! only Windows has it so far (`win32`). Elsewhere `unsupported` stands in and the spotlight
//! simply never shows.

// On macOS/Linux the shared logic exists but nothing calls it yet.
#![cfg_attr(not(windows), allow(dead_code))]

mod detector;
mod presenter;

#[cfg(not(windows))]
mod unsupported;
#[cfg(windows)]
mod win32;

#[cfg(not(windows))]
use unsupported as platform;
#[cfg(windows)]
use win32 as platform;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

pub use detector::DoubleTapDetector;
pub use platform::start_listener;
use presenter::Presenter;

/// A point on the screen, in physical pixels.
pub type Point = (i32, i32);

/// What the input listener asks the app to do.
#[derive(Debug, Clone, Copy)]
pub enum Signal {
    Show,
    Hide,
}

/// Whether the spotlight is showing, so the input listener only asks to hide it when it is.
static SHOWN: AtomicBool = AtomicBool::new(false);

/// How often the spotlight moves and resizes while it shows (~60 times a second).
const TICK: Duration = Duration::from_millis(15);

/// The spotlight, owned by the app. The overlay window is made the first time it shows.
pub struct Spotlight {
    overlay: Option<platform::Overlay>,
    presenter: Option<Presenter>,
    next_tick: Instant,
}

impl Default for Spotlight {
    fn default() -> Self {
        Self {
            overlay: None,
            presenter: None,
            next_tick: Instant::now(),
        }
    }
}

impl Spotlight {
    pub fn handle(&mut self, event_loop: &ActiveEventLoop, signal: Signal) {
        match signal {
            Signal::Show => self.show(event_loop),
            Signal::Hide => self.hide(),
        }
    }

    /// Shows the spotlight, from the start even if it's already showing.
    fn show(&mut self, event_loop: &ActiveEventLoop) {
        if self.overlay.is_none() {
            match platform::Overlay::new(event_loop) {
                Ok(overlay) => self.overlay = Some(overlay),
                Err(error) => {
                    eprintln!("spotlight unavailable: {error}");
                    return;
                }
            }
        }
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };

        let now = Instant::now();
        let cursor = platform::cursor_position();
        let presenter = Presenter::new(now, cursor);

        overlay.show();
        overlay.draw(cursor, presenter.radius(now));
        self.presenter = Some(presenter);
        self.next_tick = now + TICK;
        SHOWN.store(true, Ordering::Relaxed);
    }

    fn hide(&mut self) {
        if self.presenter.take().is_some() {
            if let Some(overlay) = self.overlay.as_mut() {
                overlay.hide();
            }
            SHOWN.store(false, Ordering::Relaxed);
        }
    }

    /// Call when the event loop is about to wait. Moves the spotlight along if it's time, and
    /// returns when to wake up next, or `None` when it isn't showing (nothing to wake up for).
    pub fn tick(&mut self) -> Option<Instant> {
        let presenter = self.presenter.as_mut()?;
        let now = Instant::now();
        if now < self.next_tick {
            return Some(self.next_tick);
        }

        match presenter.tick(now, platform::cursor_position()) {
            Some((center, radius)) => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.draw(center, radius);
                }
                self.next_tick = now + TICK;
                Some(self.next_tick)
            }
            None => {
                self.hide(); // the pointer stayed still long enough
                None
            }
        }
    }

    /// Passes on window events meant for the overlay window.
    pub fn window_event(&mut self, id: WindowId, event: &WindowEvent) {
        if let Some(overlay) = self.overlay.as_mut()
            && overlay.id() == id
        {
            overlay.window_event(event);
        }
    }
}
