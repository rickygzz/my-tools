//! Linux: picks the X11 or Wayland version of the spotlight, depending on the session.
//!
//! - **X11** (`x11.rs`): double-tap Left Ctrl, click-through overlay, like Windows and macOS.
//! - **Wayland** (`wayland/`): Wayland doesn't let apps see other apps' keys or the pointer,
//!   so a keyboard shortcut registered with the desktop shows it instead, and a maximized
//!   window learns where the pointer is when it's over it.

use std::error::Error;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{Point, Signal, wayland, x11};

/// Whether this is a Wayland session (even if X11 apps can still run in it).
fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland")
}

pub fn start_listener(notify: impl Fn(Signal) + Send + 'static) {
    if is_wayland() {
        wayland::start_listener(notify);
    } else {
        x11::start_listener(notify);
    }
}

pub fn cursor_position() -> Point {
    if is_wayland() {
        wayland::cursor_position()
    } else {
        x11::cursor_position()
    }
}

pub enum Overlay {
    X11(x11::Overlay),
    Wayland(wayland::Overlay),
}

impl Overlay {
    pub fn new(event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        if is_wayland() {
            Ok(Self::Wayland(wayland::Overlay::new(event_loop)?))
        } else {
            Ok(Self::X11(x11::Overlay::new(event_loop)?))
        }
    }

    pub fn owns(&self, id: WindowId) -> bool {
        match self {
            Self::X11(overlay) => overlay.owns(id),
            Self::Wayland(overlay) => overlay.owns(id),
        }
    }

    pub fn show(&mut self, event_loop: &ActiveEventLoop) {
        match self {
            Self::X11(overlay) => overlay.show(event_loop),
            Self::Wayland(overlay) => overlay.show(event_loop),
        }
    }

    pub fn hide(&mut self) {
        match self {
            Self::X11(overlay) => overlay.hide(),
            Self::Wayland(overlay) => overlay.hide(),
        }
    }

    pub fn draw(&mut self, center: Point, radius: i32) {
        match self {
            Self::X11(overlay) => overlay.draw(center, radius),
            Self::Wayland(overlay) => overlay.draw(center, radius),
        }
    }

    pub fn window_event(&mut self, id: WindowId, event: &WindowEvent) {
        match self {
            Self::X11(overlay) => overlay.window_event(id, event),
            Self::Wayland(overlay) => overlay.window_event(id, event),
        }
    }
}
