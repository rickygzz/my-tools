//! Windows side of the spotlight:
//!
//! - **Input hooks** (`start_listener`): Windows calls our functions for every key and click
//!   in any program, so we can spot the double Ctrl tap and hide on the next input.
//! - **Overlay** window: covers all monitors, black at 60% opacity, except a circle drawn in
//!   magenta, the "key colour" Windows makes fully see-through. It ignores the mouse and never
//!   takes the focus, so you keep working underneath it.
//!
//! Talking to Windows directly means `unsafe` code: the compiler can't check what the OS does
//! with our pointers, so each `unsafe` block notes why it's fine.

use std::error::Error;
use std::num::NonZeroU32;
use std::rc::Rc;

use softbuffer::{Context, Rect, Surface};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LCONTROL;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GWL_EXSTYLE, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowRect, HC_ACTION, HWND_TOPMOST, KBDLLHOOKSTRUCT, LWA_ALPHA, LWA_COLORKEY, MSG,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    SetWindowsHookExW, ShowWindow, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId, WindowLevel};

use super::shape::{self, Area, Circle, Colors};
use super::{Point, Signal};

// ---------------------------------------------------------------------------------------------
// Input hooks
// ---------------------------------------------------------------------------------------------

/// Starts listening to every key and click, calling `notify` when the spotlight should show
/// (Left Ctrl tapped twice) or hide (any key or click while it shows).
pub fn start_listener(notify: impl Fn(Signal) + Send + 'static) {
    super::set_listener(notify);

    // Low-level hooks run on the thread that installed them, and only while that thread is
    // waiting for messages. A thread of their own keeps them responsive whatever the app does.
    let spawned = std::thread::Builder::new()
        .name("spotlight-input".into())
        .spawn(|| {
            // SAFETY: plain Win32 calls with valid arguments; our hook functions match the
            // signature Windows expects and stay valid for the whole program.
            unsafe {
                let module = GetModuleHandleW(std::ptr::null());
                let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
                let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
                if keyboard.is_null() || mouse.is_null() {
                    eprintln!("spotlight: could not listen to the keyboard and mouse");
                    return;
                }
                // Wait for messages forever; Windows calls the hooks from inside GetMessageW.
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {}
            }
        });
    if let Err(error) = spawned {
        eprintln!("spotlight: could not start the input thread: {error}");
    }
}

/// Called by Windows for every key press and release, in any program.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT describing the key.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let is_left_ctrl = info.vkCode == u32::from(VK_LCONTROL);

        match wparam as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => super::key_down(is_left_ctrl),
            WM_KEYUP | WM_SYSKEYUP => super::key_up(is_left_ctrl),
            _ => {}
        }
    }
    // Always pass the key on, so every program still gets it.
    // SAFETY: forwarding exactly what Windows gave us.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Called by Windows for every mouse event, in any program. We only care about button presses.
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let is_click = matches!(
        wparam as u32,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    );
    if code == HC_ACTION as i32 && is_click {
        super::click();
    }
    // SAFETY: forwarding exactly what Windows gave us.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Where the mouse pointer is, in physical pixels.
pub fn cursor_position() -> Point {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is a valid place for Windows to write to.
    unsafe { GetCursorPos(&mut point) };
    (point.x, point.y)
}

// ---------------------------------------------------------------------------------------------
// Overlay window
// ---------------------------------------------------------------------------------------------

/// Pixel colours, as softbuffer wants them: 0x00RRGGBB.
const DIM: u32 = 0x00_00_00_00; // black, shown at OPACITY
const RING: u32 = 0x00_FF_FF_FF; // white ring around the hole
/// Magenta: drawn nowhere else, and Windows makes every pixel of it see-through.
/// (As a Windows COLORREF, 0x00BBGGRR, magenta happens to be the same number.)
const KEY_COLOR: u32 = 0x00_FF_00_FF;
/// 60% opacity.
const OPACITY: u8 = 153;
/// Width of the white ring, in pixels at 100% scale.
const RING_WIDTH: f64 = 4.0;

const COLORS: Colors = Colors { dim: DIM, ring: RING, hole: KEY_COLOR };

/// softbuffer's name for an area of the window.
fn to_rect(area: Area) -> Option<Rect> {
    Some(Rect {
        x: u32::try_from(area.left).ok()?,
        y: u32::try_from(area.top).ok()?,
        width: NonZeroU32::new(u32::try_from(area.width()).ok()?)?,
        height: NonZeroU32::new(u32::try_from(area.height()).ok()?)?,
    })
}

/// The rectangle around all monitors, in physical pixels (winit makes the app DPI-aware).
fn virtual_screen() -> Area {
    // SAFETY: GetSystemMetrics only reads system values.
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        Area {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

pub struct Overlay {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    hwnd: HWND,
    /// Where the window should be: over every monitor, in screen pixels.
    bounds: Area,
    /// The part of the buffer holding the last circle, in window pixels.
    hole: Option<Area>,
    /// The last thing drawn, to draw it again when Windows asks.
    last: Option<(Point, i32)>,
    /// The whole buffer must be redrawn next time (new size, or just shown).
    needs_full: bool,
}

impl Overlay {
    pub fn new(event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        let attributes = Window::default_attributes()
            .with_title("My Tools spotlight")
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_skip_taskbar(true);
        let window = Rc::new(event_loop.create_window(attributes)?);
        // Let clicks go through to the windows underneath.
        window.set_cursor_hittest(false)?;

        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            return Err("not a Win32 window".into());
        };
        let hwnd = handle.hwnd.get() as HWND;

        let context = Context::new(window.clone())?;
        let surface = Surface::new(&context, window.clone())?;

        let overlay = Self {
            window,
            surface,
            hwnd,
            bounds: Area { left: 0, top: 0, right: 0, bottom: 0 },
            hole: None,
            last: None,
            needs_full: true,
        };
        overlay.apply_styles();
        Ok(overlay)
    }

    /// Whether `id` is this overlay's window.
    pub fn owns(&self, id: WindowId) -> bool {
        self.window.id() == id
    }

    /// Makes the window see-through where it's magenta, 60% opaque elsewhere, click-through,
    /// and kept out of Alt+Tab and from ever becoming the active window.
    fn apply_styles(&self) {
        // SAFETY: `hwnd` is our live window; these calls only change its style flags.
        unsafe {
            let styles = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let wanted = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE)
                as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, styles | wanted);
            SetLayeredWindowAttributes(self.hwnd, KEY_COLOR, OPACITY, LWA_COLORKEY | LWA_ALPHA);
        }
    }

    /// Shows the window over every monitor, without taking the focus.
    pub fn show(&mut self) {
        // Monitors may have been added, removed or moved since last time.
        let bounds = virtual_screen();
        if bounds != self.bounds {
            self.bounds = bounds;
            if let (Some(width), Some(height)) = (
                NonZeroU32::new(bounds.width().max(0) as u32),
                NonZeroU32::new(bounds.height().max(0) as u32),
            ) && let Err(error) = self.surface.resize(width, height)
            {
                eprintln!("spotlight: could not resize: {error}");
            }
        }
        self.needs_full = true;
        self.apply_styles(); // winit may have reset them
        self.place(SWP_SHOWWINDOW);
    }

    pub fn hide(&mut self) {
        // Clear the hole first, so the next show doesn't flash the old circle for a frame.
        if let Some(hole) = self.hole.take() {
            let whole = Area::new(0, 0, self.bounds.width(), self.bounds.height());
            self.paint(|buffer| shape::fill(buffer, whole, hole, DIM), hole);
        }
        self.last = None;
        // SAFETY: `hwnd` is our live window.
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    /// Moves/sizes the window over all monitors, on top of everything, without activating it.
    fn place(&self, extra_flags: u32) {
        let b = self.bounds;
        // SAFETY: `hwnd` is our live window.
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                b.left,
                b.top,
                b.width(),
                b.height(),
                SWP_NOACTIVATE | extra_flags,
            );
        }
    }

    /// Puts the window back if Windows moved or resized it, which it does when a window
    /// spanning monitors with different scaling changes DPI.
    fn keep_in_place(&self) {
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: `rect` is a valid place for Windows to write to.
        unsafe { GetWindowRect(self.hwnd, &mut rect) };
        let now = Area { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
        if now != self.bounds {
            self.place(0);
        }
    }

    /// Draws the hole: a see-through circle of `radius` (at 100% scale) with a white ring,
    /// centred on `center` (screen pixels).
    pub fn draw(&mut self, center: Point, radius: i32) {
        self.keep_in_place();
        self.last = Some((center, radius));

        let scale = self.window.scale_factor();
        let ring = (RING_WIDTH * scale).round().max(1.0);
        // Centre in window pixels.
        let center = (center.0 - self.bounds.left, center.1 - self.bounds.top);
        let circle = Circle::new(center, f64::from(radius) * scale, ring);

        let (width, height) = (self.bounds.width(), self.bounds.height());
        let whole = Area::new(0, 0, width, height);
        let area = circle.bounds(width, height);

        let previous = self.hole;
        let full = self.needs_full;
        let damage = match previous {
            _ if full => whole,
            Some(previous) => previous.union(&area),
            None => area,
        };

        let drawn = self.paint(
            |buffer| {
                if full {
                    buffer.fill(DIM);
                }
                // Repaint the old circle's area too, which puts the dim back where it was.
                circle.paint(buffer, whole, damage, COLORS);
            },
            damage,
        );
        if drawn {
            self.hole = Some(area);
            self.needs_full = false;
        }
    }

    /// Gets the pixel buffer, lets `draw` change it, and puts `damage` on screen.
    /// Returns false if drawing wasn't possible (the next draw then redraws everything).
    fn paint(&mut self, draw: impl FnOnce(&mut [u32]), damage: Area) -> bool {
        let (width, height) = (self.bounds.width(), self.bounds.height());
        let mut buffer = match self.surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("spotlight: could not draw: {error}");
                return false;
            }
        };
        // A fresh buffer has undefined contents: it must be drawn in full.
        if buffer.age() == 0 && !self.needs_full {
            self.needs_full = true;
            return false;
        }
        draw(&mut buffer);
        let presented = match to_rect(damage.clamp_to(width, height)) {
            Some(rect) => buffer.present_with_damage(&[rect]),
            None => Ok(()),
        };
        if let Err(error) = presented {
            eprintln!("spotlight: could not show the frame: {error}");
            return false;
        }
        true
    }

    pub fn window_event(&mut self, event: &WindowEvent) {
        match event {
            // Windows wants the window painted again (e.g. after a display change).
            WindowEvent::RedrawRequested => {
                if let Some((center, radius)) = self.last {
                    self.needs_full = true;
                    self.draw(center, radius);
                }
            }
            // Keep our size when the window changes DPI; winit would otherwise rescale it.
            WindowEvent::ScaleFactorChanged { inner_size_writer, .. } => {
                let size = PhysicalSize::new(
                    self.bounds.width().max(1) as u32,
                    self.bounds.height().max(1) as u32,
                );
                let _ = inner_size_writer.clone().request_inner_size(size);
            }
            _ => {}
        }
    }
}

