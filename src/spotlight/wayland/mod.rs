//! Linux side of the spotlight, for **Wayland** sessions.
//!
//! Wayland doesn't let an app see other apps' keys or where the pointer is, so:
//!
//! - **Trigger**: instead of a double Ctrl tap, a keyboard shortcut (see `crate::hotkeys`),
//!   which comes with an activation token that lets the overlay take the keyboard focus.
//! - **Overlay**: one transparent *maximized* window, created each time it shows (Wayland apps
//!   can't hide a window, only destroy it). The desktop opens it on the monitor with the
//!   pointer, and tells it where the pointer is once it's over it, which is all we need. Drawn
//!   with our own see-through pixel buffers (`canvas.rs`).
//!
//!   Not *full-screen*: GNOME paints solid black behind full-screen windows, which would hide
//!   everything but the ring. The price is that GNOME's top bar and other monitors aren't
//!   dimmed, and Wayland apps can't pick a monitor themselves.
//! - **Hiding**: any key or click while it shows. Unlike on other systems, that click
//!   goes to the overlay rather than through it to the window underneath.

mod canvas;

use std::error::Error;
use std::sync::Mutex;

use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::platform::startup_notify::WindowAttributesExtStartupNotify;
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use winit::window::{ActivationToken, Window, WindowId};

use super::shape::Colors;
use super::{Point, Signal};
use crate::APP_ID;
use canvas::{Canvas, Display, Spot};

/// Premultiplied ARGB: 60% black, opaque white, fully transparent.
const COLORS: Colors = Colors { dim: 0x9900_0000, ring: 0xFFFF_FFFF, hole: 0x0000_0000 };
/// Width of the white ring, in pixels at 100% scale.
const RING_WIDTH: f64 = 4.0;

/// The overlay window the pointer is over and where in it (physical pixels), as last reported.
static POINTER: Mutex<Option<(WindowId, Point)>> = Mutex::new(None);
/// From the latest shortcut press: lets the overlay window take the keyboard focus.
static ACTIVATION_TOKEN: Mutex<Option<String>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Remembers who to tell when a key or click should hide the spotlight. The shortcut that
/// shows it is registered by `crate::hotkeys`.
pub fn start_listener(notify: impl Fn(Signal) + Send + 'static) {
    super::set_listener(notify);
}

/// Keeps the activation token from the shortcut press that's about to show the spotlight.
pub fn set_activation_token(token: Option<String>) {
    *lock(&ACTIVATION_TOKEN) = token;
}

/// Where the pointer is, in the overlay window it's over. (0, 0) until one has been told.
pub fn cursor_position() -> Point {
    lock(&POINTER).map_or((0, 0), |(_, point)| point)
}

/// One monitor's window. `canvas` is declared first so it's dropped before the window it
/// draws into.
struct Screen {
    canvas: Canvas,
    window: Window,
}

pub struct Overlay {
    screens: Vec<Screen>,
    /// The last thing drawn, to draw again when a window changes size.
    last: Option<(Point, i32)>,
    /// Dropped after `screens`, whose buffers belong to it.
    display: Display,
}

impl Overlay {
    pub fn new(event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        let RawDisplayHandle::Wayland(handle) = event_loop.display_handle()?.as_raw() else {
            return Err("the app isn't running as a Wayland client".into());
        };
        // SAFETY: the display belongs to winit's event loop, which outlives the overlay:
        // `App::exiting` drops the spotlight before the event loop goes away.
        let display = unsafe { Display::new(handle.display)? };
        Ok(Self { screens: Vec::new(), last: None, display })
    }

    pub fn owns(&self, id: WindowId) -> bool {
        self.screens.iter().any(|screen| screen.window.id() == id)
    }

    /// Opens a maximized window on the pointer's monitor, dim until the pointer is found.
    pub fn show(&mut self, event_loop: &ActiveEventLoop) {
        self.hide();
        let mut attributes = Window::default_attributes()
            .with_title("My Tools spotlight")
            .with_name(APP_ID, "")
            .with_decorations(false)
            .with_transparent(true)
            .with_maximized(true);
        // Lets the window take the keyboard focus, so a key press can hide it.
        if let Some(token) = lock(&ACTIVATION_TOKEN).take() {
            attributes = attributes.with_activation_token(ActivationToken::from_raw(token));
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("spotlight: could not open a window: {error}");
                return;
            }
        };
        match Canvas::new(&self.display, &window) {
            Ok(canvas) => self.screens.push(Screen { canvas, window }),
            Err(error) => eprintln!("spotlight: could not draw in a window: {error}"),
        }
        self.redraw_all();
    }

    pub fn hide(&mut self) {
        self.screens.clear(); // closes the windows
        self.last = None;
        *lock(&POINTER) = None;
    }

    /// Draws the hole: a see-through circle of `radius` (at 100% scale) with a white ring,
    /// centred on `center` in the window the pointer is over. The other windows stay dim.
    pub fn draw(&mut self, center: Point, radius: i32) {
        self.last = Some((center, radius));
        self.redraw_all();
    }

    fn redraw_all(&mut self) {
        let pointer_window = lock(&POINTER).map(|(id, _)| id);
        for screen in &mut self.screens {
            let spot = match self.last {
                Some((center, radius)) if pointer_window == Some(screen.window.id()) => {
                    let scale = screen.window.scale_factor();
                    Some(Spot {
                        center,
                        radius: (f64::from(radius) * scale).round() as i32,
                        ring: (RING_WIDTH * scale).round().max(1.0) as i32,
                    })
                }
                _ => None,
            };
            let size = screen.window.inner_size();
            // If the compositor still holds both buffers, this frame is skipped; the next
            // tick draws the then-current state.
            screen.canvas.draw(&mut self.display, size.width, size.height, spot, COLORS);
        }
    }

    pub fn window_event(&mut self, id: WindowId, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                *lock(&POINTER) = Some((id, (position.x.round() as i32, position.y.round() as i32)));
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, .. } => super::click(),
            // Keys still held from the shortcut are reported as "synthetic" when the window
            // gets the focus; they and key repeats don't count.
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                super::key_down(false);
            }
            WindowEvent::Resized(_) | WindowEvent::RedrawRequested => self.redraw_all(),
            _ => {}
        }
    }
}
