//! Mouse spotlight: tap **Left Ctrl twice** to dim every screen except a circle around the
//! pointer. The circle starts large and shrinks onto the pointer so the eye follows it there,
//! then follows the pointer. It goes away at the next key or click, or once the pointer has
//! stayed still for a few seconds.
//!
//! The timing rules (`detector`) and the animation (`presenter`) are plain Rust and work on
//! any OS. Listening to keys in every program and drawing the overlay need OS-specific code,
//! one file per OS, each with the same three things: `start_listener`, `cursor_position`
//! and `Overlay`.
//!
//! | OS            | File             | Keys from          | Overlay drawn with          |
//! |---------------|------------------|--------------------|-----------------------------|
//! | Windows       | `win32.rs`       | low-level hooks    | layered window + softbuffer |
//! | macOS         | `macos.rs`       | CGEventTap         | NSWindow + CAShapeLayer     |
//! | Linux (X11)   | `x11.rs`         | XInput2 raw events | ARGB X window               |
//! | anything else | `unsupported.rs` | -                  | -                           |
//!
//! Wayland doesn't let apps see other apps' keys or the pointer, so on a Wayland session
//! `x11.rs` stands down (see there).

mod detector;
mod presenter;
mod shape;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod unsupported;
#[cfg(windows)]
mod win32;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
use unsupported as platform;
#[cfg(windows)]
use win32 as platform;
#[cfg(target_os = "linux")]
use x11 as platform;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use detector::DoubleTapDetector;
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

// ---------------------------------------------------------------------------------------------
// Input: each OS's listener reports keys and clicks here; this decides what they mean.
// ---------------------------------------------------------------------------------------------

/// What the listener needs between events. The OS calls listeners as plain functions with no
/// `self`, so it lives in a global; the Mutex makes sharing it between threads safe.
struct Listener {
    detector: DoubleTapDetector,
    notify: Box<dyn Fn(Signal) + Send>,
}

static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

/// Sets who to tell when the spotlight should show or hide. Each OS's `start_listener`
/// calls this before it starts reporting input.
fn set_listener(notify: impl Fn(Signal) + Send + 'static) {
    *LISTENER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Listener {
        detector: DoubleTapDetector::default(),
        notify: Box::new(notify),
    });
}

/// Runs `f` with the listener. Never panics: OS callbacks often can't cope with a panic.
fn with_listener(f: impl FnOnce(&mut Listener)) {
    let mut guard = LISTENER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(listener) = guard.as_mut() {
        f(listener);
    }
}

fn hide_if_shown(listener: &Listener) {
    if SHOWN.load(Ordering::Relaxed) {
        (listener.notify)(Signal::Hide);
    }
}

/// A key went down anywhere. `is_trigger` says whether it's Left Ctrl.
fn key_down(is_trigger: bool) {
    let now = Instant::now();
    with_listener(|listener| {
        hide_if_shown(listener);
        listener.detector.key_down(is_trigger, now);
    });
}

/// A key went up anywhere. Shows the spotlight if this ended a double Left Ctrl tap.
fn key_up(is_trigger: bool) {
    let now = Instant::now();
    with_listener(|listener| {
        if listener.detector.key_up(is_trigger, now) {
            (listener.notify)(Signal::Show);
        }
    });
}

/// A mouse button went down anywhere.
fn click() {
    with_listener(|listener| {
        hide_if_shown(listener);
        listener.detector.interrupt();
    });
}

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
            && overlay.owns(id)
        {
            overlay.window_event(event);
        }
    }
}
