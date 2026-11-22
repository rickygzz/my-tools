//! Linux side of the spotlight, for **X11** sessions:
//!
//! - **Listener** (`start_listener`): asks the X server for XInput2 *raw* key and button
//!   events on the root window, which arrive for every app, without special permissions.
//! - **Overlay**: an "override-redirect" window (the window manager leaves it alone: no
//!   frame, no taskbar entry, on top) covering the whole X screen, with a 32-bit visual so each
//!   pixel has its own transparency. Its background is the dim colour; we only upload the
//!   pixels around the circle. XFixes gives it an empty input area, so clicks go through.
//!   Per-pixel transparency needs a *compositor* (built into GNOME, KDE, Cinnamon, XFCE...).
//!
//! **Wayland** sessions get nothing: Wayland doesn't let an app see other apps' keys or where
//! the pointer is, by design. GNOME (Settings → Accessibility → Locate Pointer) and KDE
//! (the Track Mouse effect) have a built-in equivalent instead.

use std::error::Error;
use std::sync::OnceLock;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::shape::SK;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xinput::{self, ConnectionExt as _, XIEventMask};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    ImageFormat, ImageOrder, StackMode, VisualClass, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::shape::{Area, Circle, Colors};
use super::{Point, Signal};

const WAYLAND_NOTE: &str = "spotlight: not available on Wayland, which doesn't let apps see \
     other apps' keys or the pointer. Use your desktop's own: GNOME Settings → Accessibility → \
     Locate Pointer, or KDE's Track Mouse effect.";

/// Whether this is a Wayland session (even if X11 apps can still run in it).
fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland")
}

// ---------------------------------------------------------------------------------------------
// Listener
// ---------------------------------------------------------------------------------------------

/// The X keysym for Left Control.
const CONTROL_L: u32 = 0xffe3;
/// Left Control's usual key code, if it can't be looked up.
const DEFAULT_LEFT_CONTROL: u32 = 37;
/// Mouse "buttons" 4 to 7 are the scroll wheel, which isn't a click.
const SCROLL_BUTTONS: std::ops::RangeInclusive<u32> = 4..=7;

/// Starts listening to every key and click, calling `notify` when the spotlight should show
/// (Left Ctrl tapped twice) or hide (any key or click while it shows).
pub fn start_listener(notify: impl Fn(Signal) + Send + 'static) {
    if is_wayland() {
        eprintln!("{WAYLAND_NOTE}");
        return;
    }
    super::set_listener(notify);

    let spawned = std::thread::Builder::new()
        .name("spotlight-input".into())
        .spawn(|| {
            if let Err(error) = listen() {
                eprintln!("spotlight: could not listen to the keyboard and mouse: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("spotlight: could not start the input thread: {error}");
    }
}

/// Runs forever on its own thread, with its own connection to the X server.
fn listen() -> Result<(), Box<dyn Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;

    // Raw events need XInput 2.0 or later.
    conn.xinput_xi_query_version(2, 2)?.reply()?;
    let left_control = left_control_keycode(&conn).unwrap_or(DEFAULT_LEFT_CONTROL);

    conn.xinput_xi_select_events(
        root,
        &[xinput::EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![
                XIEventMask::RAW_KEY_PRESS
                    | XIEventMask::RAW_KEY_RELEASE
                    | XIEventMask::RAW_BUTTON_PRESS,
            ],
        }],
    )?;
    conn.flush()?;

    loop {
        match conn.wait_for_event()? {
            Event::XinputRawKeyPress(event) => super::key_down(event.detail == left_control),
            Event::XinputRawKeyRelease(event) => super::key_up(event.detail == left_control),
            Event::XinputRawButtonPress(event) if !SCROLL_BUTTONS.contains(&event.detail) => {
                super::click();
            }
            _ => {}
        }
    }
}

/// Looks up which key code produces Left Control on this keyboard layout.
fn left_control_keycode(conn: &impl Connection) -> Option<u32> {
    let setup = conn.setup();
    let (first, last) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn.get_keyboard_mapping(first, last - first + 1).ok()?.reply().ok()?;
    let per_key = usize::from(mapping.keysyms_per_keycode);
    if per_key == 0 {
        return None;
    }
    mapping
        .keysyms
        .chunks(per_key)
        .position(|keysyms| keysyms.contains(&CONTROL_L))
        .map(|index| u32::from(first) + index as u32)
}

// ---------------------------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------------------------

/// The main thread's connection to the X server, made the first time it's needed.
struct Display {
    conn: RustConnection,
    screen: usize,
}

static DISPLAY: OnceLock<Option<Display>> = OnceLock::new();

fn display() -> Option<&'static Display> {
    DISPLAY
        .get_or_init(|| {
            let (conn, screen) = x11rb::connect(None).ok()?;
            Some(Display { conn, screen })
        })
        .as_ref()
}

/// Where the mouse pointer is, in pixels of the X screen (all monitors together).
pub fn cursor_position() -> Point {
    display()
        .and_then(|d| {
            let root = d.conn.setup().roots[d.screen].root;
            d.conn.query_pointer(root).ok()?.reply().ok()
        })
        .map_or((0, 0), |pointer| (i32::from(pointer.root_x), i32::from(pointer.root_y)))
}

/// Premultiplied ARGB: 60% black, opaque white, fully transparent.
const COLORS: Colors = Colors { dim: 0x9900_0000, ring: 0xFFFF_FFFF, hole: 0x0000_0000 };
/// Width of the white ring, in pixels.
const RING_WIDTH: f64 = 4.0;

pub struct Overlay {
    display: &'static Display,
    window: u32,
    gc: u32,
    width: u16,
    height: u16,
    /// The area holding the last circle, to put the dim back there next time.
    hole: Option<Area>,
}

impl Overlay {
    pub fn new(_event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        if is_wayland() {
            return Err(WAYLAND_NOTE.into());
        }
        let display = display().ok_or("could not connect to the X server")?;
        let conn = &display.conn;
        let screen = &conn.setup().roots[display.screen];

        // A 32-bit visual gives every pixel its own transparency (alpha).
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|depth| depth.depth == 32)
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.class == VisualClass::TRUE_COLOR)
            .ok_or("no 32-bit visual: transparent windows aren't available")?;

        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual.visual_id)?;

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
            visual.visual_id,
            &CreateWindowAux::new()
                .background_pixel(COLORS.dim) // X fills the window with this by itself
                .border_pixel(0)
                .colormap(colormap)
                .override_redirect(1), // no frame, no taskbar, not managed: just on top
        )?;

        // An empty input shape: the pointer "falls through" to the windows underneath.
        conn.xfixes_query_version(5, 0)?.reply()?;
        let region = conn.generate_id()?;
        conn.xfixes_create_region(region, &[])?;
        conn.xfixes_set_window_shape_region(window, SK::INPUT, 0, 0, region)?;
        conn.xfixes_destroy_region(region)?;

        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?;
        conn.flush()?;

        Ok(Self {
            display,
            window,
            gc,
            width: screen.width_in_pixels,
            height: screen.height_in_pixels,
            hole: None,
        })
    }

    /// The overlay is a plain X window, not a winit one.
    pub fn owns(&self, _id: WindowId) -> bool {
        false
    }

    pub fn window_event(&mut self, _event: &WindowEvent) {}

    pub fn show(&mut self) {
        if let Err(error) = self.try_show() {
            eprintln!("spotlight: could not show: {error}");
        }
    }

    fn try_show(&mut self) -> Result<(), Box<dyn Error>> {
        let conn = &self.display.conn;
        // Monitors may have been added or moved: the X screen's size follows them.
        let root = conn.setup().roots[self.display.screen].root;
        let geometry = conn.get_geometry(root)?.reply()?;
        (self.width, self.height) = (geometry.width, geometry.height);

        conn.configure_window(
            self.window,
            &ConfigureWindowAux::new()
                .x(0)
                .y(0)
                .width(u32::from(self.width))
                .height(u32::from(self.height))
                .stack_mode(StackMode::ABOVE),
        )?;
        conn.map_window(self.window)?; // shows it, filled with the dim background
        conn.flush()?;
        self.hole = None;
        Ok(())
    }

    pub fn hide(&mut self) {
        let conn = &self.display.conn;
        let _ = conn.unmap_window(self.window);
        let _ = conn.flush();
        self.hole = None;
    }

    /// Cuts a hole of `radius` pixels with a white ring around `center` (screen pixels).
    pub fn draw(&mut self, center: Point, radius: i32) {
        if let Err(error) = self.try_draw(center, radius) {
            eprintln!("spotlight: could not draw: {error}");
        }
    }

    fn try_draw(&mut self, center: Point, radius: i32) -> Result<(), Box<dyn Error>> {
        let (width, height) = (i32::from(self.width), i32::from(self.height));
        let circle = Circle::new(center, f64::from(radius), RING_WIDTH);
        let area = circle.bounds(width, height);
        // Redraw the old circle's area too, which puts the dim back there.
        let damage = match self.hole {
            Some(previous) => previous.union(&area),
            None => area,
        };
        if damage.width() <= 0 || damage.height() <= 0 {
            return Ok(());
        }

        let mut pixels = vec![0u32; (damage.width() * damage.height()) as usize];
        circle.paint(&mut pixels, damage, damage, COLORS);
        self.upload(&pixels, damage)?;
        self.hole = Some(area);
        Ok(())
    }

    /// Sends `pixels` (covering `area`) to the window, split into requests the server accepts.
    fn upload(&self, pixels: &[u32], area: Area) -> Result<(), Box<dyn Error>> {
        let conn = &self.display.conn;
        let little_endian = conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        let bytes: Vec<u8> = pixels
            .iter()
            .flat_map(|p| if little_endian { p.to_le_bytes() } else { p.to_be_bytes() })
            .collect();

        let row_bytes = area.width() as usize * 4;
        let max_rows = ((conn.maximum_request_bytes() - 64) / row_bytes).max(1);
        for (index, rows) in bytes.chunks(row_bytes * max_rows).enumerate() {
            let top = area.top + (index * max_rows) as i32;
            conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                area.width() as u16,
                (rows.len() / row_bytes) as u16,
                area.left as i16,
                top as i16,
                0,
                32,
                rows,
            )?;
        }
        conn.flush()?;
        Ok(())
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        let conn = &self.display.conn;
        let _ = conn.destroy_window(self.window);
        let _ = conn.flush();
    }
}
