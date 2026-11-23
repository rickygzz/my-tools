//! Dragging windows into zones on Linux X11:
//!
//! - **Watching drags**: X11 has no "a window is being moved" event for other apps (the window
//!   manager moves them). So a thread listens to the mouse through XInput2 *raw* events, which
//!   come even while the window manager has the mouse grabbed: while the left button is down,
//!   the active window moving means it's being dragged. Shift and the right button are read
//!   from the pointer; Esc from raw key events.
//! - **Overlay**: an override-redirect window (no frame, on top, not managed) over the whole X
//!   screen, with a 32-bit visual so each pixel has its own transparency, and an empty input
//!   shape so the mouse goes through. Transparency needs a compositor, as the spotlight's does.

use std::error::Error;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::shape::SK;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xinput::{self, ConnectionExt as _, XIEventMask};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, ImageFormat, ImageOrder,
    KeyButMask, StackMode, VisualClass, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::desktop::Desktop;
use super::drag::{Pointer, ShownZone, Signal};
use super::layout::Rect;
use super::paint::{self, OPACITY, Paint};
use super::x11::SystemDesktop;

// ---------------------------------------------------------------------------------------------
// Watching drags
// ---------------------------------------------------------------------------------------------

/// Often enough for the highlighted zone to follow the mouse smoothly; raw motion comes much
/// more often, and each look at the window is a round trip to the X server.
const POLL: Duration = Duration::from_millis(15);
/// The X keysym of Esc, and its usual key code if it can't be looked up.
const ESCAPE: u32 = 0xff1b;
const DEFAULT_ESCAPE: u32 = 9;

pub fn start_watching(notify: impl Fn(Signal) + Send + Sync + 'static) {
    let spawned = std::thread::Builder::new().name("zones-drags".into()).spawn(move || {
        if let Err(error) = watch(&notify) {
            eprintln!("zones: can't watch window drags, so Shift+drag won't show the zones: {error}");
        }
    });
    if let Err(error) = spawned {
        eprintln!("zones: could not start the drag thread: {error}");
    }
}

/// A drag of the left button, which may turn out to move a window.
struct Press {
    /// The active window and where it was, read at the first motion (the press makes it
    /// active).
    window: Option<(u32, Rect)>,
    /// Once it moved: it's being dragged.
    dragging: bool,
    size: (i32, i32),
    size_changes: u32,
    escape: bool,
    last_look: Instant,
}

fn watch(notify: &dyn Fn(Signal)) -> Result<(), Box<dyn Error>> {
    let desktop = SystemDesktop::new()?;
    let conn = &desktop.conn;
    conn.xinput_xi_query_version(2, 2)?.reply()?;
    let escape = keycode(conn, ESCAPE).unwrap_or(DEFAULT_ESCAPE);
    conn.xinput_xi_select_events(
        desktop.root,
        &[xinput::EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![
                XIEventMask::RAW_BUTTON_PRESS
                    | XIEventMask::RAW_BUTTON_RELEASE
                    | XIEventMask::RAW_MOTION
                    | XIEventMask::RAW_KEY_PRESS,
            ],
        }],
    )?;
    conn.flush()?;

    let mut press: Option<Press> = None;
    loop {
        match conn.wait_for_event()? {
            Event::XinputRawButtonPress(event) if event.detail == 1 => {
                press = Some(Press {
                    window: None,
                    dragging: false,
                    size: (0, 0),
                    size_changes: 0,
                    escape: false,
                    last_look: Instant::now() - POLL,
                });
            }
            Event::XinputRawKeyPress(event) if event.detail == escape => {
                if let Some(press) = &mut press {
                    press.escape = true;
                }
            }
            Event::XinputRawMotion(_) => {
                if let Some(press) = &mut press
                    && press.last_look.elapsed() >= POLL
                {
                    press.last_look = Instant::now();
                    look(&desktop, press, notify);
                }
            }
            Event::XinputRawButtonRelease(event) if event.detail == 1 => {
                if let Some(mut press) = press.take()
                    && press.dragging
                {
                    look(&desktop, &mut press, notify);
                    notify(Signal::Ended);
                }
            }
            _ => {}
        }
    }
}

/// Looks at the pressed window: tells the drag has started once it moves, and where the mouse
/// is while it's dragged.
fn look(desktop: &SystemDesktop, press: &mut Press, notify: &dyn Fn(Signal)) {
    let Some((window, start)) = press.window else {
        press.window = desktop.active_window().map(|window| (window, desktop.window_bounds(window)));
        press.size = press.window.map_or((0, 0), |(_, bounds)| (bounds.width, bounds.height));
        return;
    };
    let now = desktop.window_bounds(window);
    let size = (now.width, now.height);
    if size != press.size {
        press.size = size;
        press.size_changes += 1;
    }
    if !press.dragging {
        if now == start {
            return;
        }
        press.dragging = true;
        notify(Signal::Started(u64::from(window)));
    }
    let Some(pointer) = desktop.conn.query_pointer(desktop.root).ok().and_then(|cookie| cookie.reply().ok()) else {
        return;
    };
    notify(Signal::Moved(Pointer {
        x: pointer.root_x.into(),
        y: pointer.root_y.into(),
        shift: pointer.mask.contains(KeyButMask::SHIFT),
        // Moving a maximized window changes its size once, as the window manager gives it
        // back its own; a resize changes it with every move.
        resizing: press.size_changes >= 2,
        right_button: pointer.mask.contains(KeyButMask::BUTTON3),
        escape: press.escape,
    }));
}

/// Which key code gives `keysym` on this keyboard layout.
fn keycode(conn: &impl Connection, keysym: u32) -> Option<u32> {
    let setup = conn.setup();
    let (first, last) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn.get_keyboard_mapping(first, last - first + 1).ok()?.reply().ok()?;
    let per_key = usize::from(mapping.keysyms_per_keycode);
    if per_key == 0 {
        return None;
    }
    let index = mapping.keysyms.chunks(per_key).position(|keysyms| keysyms.contains(&keysym))?;
    Some(u32::from(first) + index as u32)
}

/// The window a [`Signal::Started`] named.
pub fn window(id: u64) -> u32 {
    id as u32
}

// ---------------------------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------------------------

/// Premultiplied ARGB at the overlay's opacity.
fn pixel(paint: Paint) -> u32 {
    let alpha = (OPACITY * 255.0).round() as u32;
    let [red, green, blue] = paint::rgb(paint).map(|channel| u32::from(channel) * alpha / 255);
    alpha << 24 | red << 16 | green << 8 | blue
}

pub struct Overlay {
    conn: RustConnection,
    screen: usize,
    window: u32,
    gc: u32,
}

impl Overlay {
    pub fn new(_event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        let (conn, screen_number) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_number];
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|depth| depth.depth == 32)
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.class == VisualClass::TRUE_COLOR)
            .ok_or("no 32-bit visual: transparent windows aren't available")?
            .visual_id;
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual)?;
        let window = conn.generate_id()?;
        conn.create_window(
            32,
            window,
            screen.root,
            0,
            0,
            screen.width_in_pixels,
            screen.height_in_pixels,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new().background_pixel(0).border_pixel(0).colormap(colormap).override_redirect(1),
        )?;
        // An empty input shape: the mouse goes through to the windows underneath.
        conn.xfixes_query_version(5, 0)?.reply()?;
        let region = conn.generate_id()?;
        conn.xfixes_create_region(region, &[])?;
        conn.xfixes_set_window_shape_region(window, SK::INPUT, 0, 0, region)?;
        conn.xfixes_destroy_region(region)?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?;
        conn.flush()?;
        Ok(Self { conn, screen: screen_number, window, gc })
    }

    /// The overlay is a plain X window, not a winit one.
    pub fn owns(&self, _id: WindowId) -> bool {
        false
    }

    pub fn window_event(&mut self, _event: &WindowEvent) {}

    fn draw(&self, zones: &[ShownZone], highlighted: Option<usize>) -> Result<(), Box<dyn Error>> {
        // The X screen is all monitors together; it follows them when they change.
        let root = self.conn.setup().roots[self.screen].root;
        let geometry = self.conn.get_geometry(root)?.reply()?;
        let (width, height) = (i32::from(geometry.width), i32::from(geometry.height));
        self.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new().x(0).y(0).width(width as u32).height(height as u32).stack_mode(StackMode::ABOVE),
        )?;
        self.conn.map_window(self.window)?;

        let shapes = paint::shapes(zones, highlighted, (0, 0), 1.0);
        let pixels = paint::rasterize(&shapes, width, height, 0, pixel);
        let little_endian = self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        let bytes: Vec<u8> =
            pixels.iter().flat_map(|p| if little_endian { p.to_le_bytes() } else { p.to_be_bytes() }).collect();
        // Split into requests the server accepts.
        let row_bytes = width as usize * 4;
        let max_rows = ((self.conn.maximum_request_bytes() - 64) / row_bytes).max(1);
        for (index, rows) in bytes.chunks(row_bytes * max_rows).enumerate() {
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                width as u16,
                (rows.len() / row_bytes) as u16,
                0,
                (index * max_rows) as i16,
                0,
                32,
                rows,
            )?;
        }
        self.conn.flush()?;
        Ok(())
    }
}

impl super::drag::Overlay for Overlay {
    fn show(&mut self, zones: &[ShownZone], highlighted: Option<usize>) {
        if let Err(error) = self.draw(zones, highlighted) {
            eprintln!("zones: could not draw: {error}");
        }
    }

    fn hide(&mut self) {
        let _ = self.conn.unmap_window(self.window);
        let _ = self.conn.flush();
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}
