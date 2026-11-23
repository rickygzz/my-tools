//! Dragging windows into zones on Windows:
//!
//! - **Watching drags**: Windows tells, through a "WinEvent" hook, when the user starts and
//!   stops moving or resizing any window. While they do, a thread reads the mouse, Shift, the
//!   right button and Esc every 15 ms, and whether the window's size keeps changing (a resize).
//! - **Overlay**: a window over every monitor, click-through and never active, 60% opaque,
//!   see-through where it's magenta (the "key colour"), as the spotlight's is.

use std::error::Error;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use softbuffer::{Context, Surface};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_RBUTTON, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, GWL_EXSTYLE, GetCursorPos, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, LWA_ALPHA, LWA_COLORKEY, OBJID_WINDOW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId, WindowLevel};

use super::drag::{Pointer, ShownZone, Signal};
use super::layout::Rect;
use super::paint::{self, OPACITY, Paint};

// ---------------------------------------------------------------------------------------------
// Watching drags
// ---------------------------------------------------------------------------------------------

/// Often enough for the highlighted zone to follow the mouse smoothly.
const POLL: Duration = Duration::from_millis(15);

type Notify = Box<dyn Fn(Signal) + Send + Sync>;

static NOTIFY: OnceLock<Notify> = OnceLock::new();

/// The drag going on: its window, and the thread reading the mouse meanwhile.
struct Polling {
    window: usize,
    stop: Arc<AtomicBool>,
    escape: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

static POLLING: Mutex<Option<Polling>> = Mutex::new(None);

/// Starts telling of drags of other programs' windows. Call it on the thread that runs the
/// event loop: Windows calls the hook from that thread's message loop.
pub fn start_watching(notify: impl Fn(Signal) + Send + Sync + 'static) {
    if NOTIFY.set(Box::new(notify)).is_err() {
        return;
    }
    // SAFETY: the hook calls `on_event`, a plain function, from this thread's message loop.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_MOVESIZESTART,
            EVENT_SYSTEM_MOVESIZEEND,
            std::ptr::null_mut(),
            Some(on_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.is_null() {
        eprintln!("zones: Windows won't tell of window drags, so Shift+drag won't show the zones");
    }
}

fn notify(signal: Signal) {
    if let Some(notify) = NOTIFY.get() {
        notify(signal);
    }
}

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, event: u32, window: HWND, object: i32, _: i32, _: u32, _: u32) {
    if object != OBJID_WINDOW {
        return;
    }
    let mut polling = POLLING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if event == EVENT_SYSTEM_MOVESIZESTART && super::win32::is_movable(window) {
        if let Some(old) = polling.take() {
            old.stop.store(true, Ordering::Relaxed);
        }
        notify(Signal::Started(window as u64));
        *polling = Some(start_polling(window as usize));
    } else if event == EVENT_SYSTEM_MOVESIZEEND
        && let Some(drag) = polling.take_if(|drag| drag.window == window as usize)
    {
        drag.stop.store(true, Ordering::Relaxed);
        // After the thread's last move, so the signals come in order.
        let _ = drag.thread.join();
        let mut last = Sampler::new(drag.window).sample();
        last.escape |= drag.escape.load(Ordering::Relaxed);
        notify(Signal::Moved(last));
        notify(Signal::Ended);
    }
}

fn start_polling(window: usize) -> Polling {
    let stop = Arc::new(AtomicBool::new(false));
    let escape = Arc::new(AtomicBool::new(false));
    let (thread_stop, thread_escape) = (stop.clone(), escape.clone());
    let thread = std::thread::spawn(move || {
        let mut sampler = Sampler::new(window);
        while !thread_stop.load(Ordering::Relaxed) {
            let mut pointer = sampler.sample();
            // Remember Esc: Windows ends the drag on it, maybe before the last sample.
            if pointer.escape {
                thread_escape.store(true, Ordering::Relaxed);
            }
            pointer.escape |= thread_escape.load(Ordering::Relaxed);
            notify(Signal::Moved(pointer));
            std::thread::sleep(POLL);
        }
    });
    Polling { window, stop, escape, thread }
}

/// Reads the mouse and keys during a drag, and counts how often the window's size changed.
/// Moving a maximized or snapped window changes it once, when Windows gives it back its own
/// size; a resize changes it with every move of the mouse.
struct Sampler {
    window: usize,
    size: (i32, i32),
    changes: u32,
}

impl Sampler {
    fn new(window: usize) -> Self {
        Self { window, size: size_of(window), changes: 0 }
    }

    fn sample(&mut self) -> Pointer {
        let size = size_of(self.window);
        if size != self.size {
            self.size = size;
            self.changes += 1;
        }
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: plain calls; `point` is a valid place to write to.
        unsafe { GetCursorPos(&mut point) };
        let down = |key: u16| unsafe { GetAsyncKeyState(i32::from(key)) } as u16 & 0x8000 != 0;
        Pointer {
            x: point.x,
            y: point.y,
            shift: down(VK_SHIFT),
            resizing: self.changes >= 2,
            right_button: down(VK_RBUTTON),
            escape: down(VK_ESCAPE),
        }
    }
}

fn size_of(window: usize) -> (i32, i32) {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: writes `rect`; a closed window just fails.
    unsafe { GetWindowRect(window as HWND, &mut rect) };
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// The window a [`Signal::Started`] named.
pub fn window(id: u64) -> HWND {
    id as usize as HWND
}

// ---------------------------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------------------------

/// Magenta, drawn nowhere else: Windows makes every pixel of it see-through. (As a COLORREF,
/// 0x00BBGGRR, magenta is the same number.)
const KEY_COLOR: u32 = 0x00_FF_00_FF;

fn pixel(paint: Paint) -> u32 {
    let [red, green, blue] = paint::rgb(paint);
    u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
}

/// The rectangle around all monitors.
fn virtual_screen() -> Rect {
    // SAFETY: GetSystemMetrics only reads system values.
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

pub struct Overlay {
    window: Rc<Window>,
    surface: Surface<Rc<Window>, Rc<Window>>,
    hwnd: HWND,
    /// Over every monitor, in screen pixels.
    bounds: Rect,
    /// What it shows, to draw it again when Windows asks.
    last: Option<(Vec<ShownZone>, Option<usize>)>,
}

impl Overlay {
    pub fn new(event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        let attributes = Window::default_attributes()
            .with_title("My Tools window zones")
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        let window = Rc::new(event_loop.create_window(attributes)?);
        window.set_cursor_hittest(false)?;
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            return Err("not a Win32 window".into());
        };
        let hwnd = handle.hwnd.get() as HWND;
        let context = Context::new(window.clone())?;
        let surface = Surface::new(&context, window.clone())?;
        Ok(Self { window, surface, hwnd, bounds: Rect::default(), last: None })
    }

    pub fn owns(&self, id: WindowId) -> bool {
        self.window.id() == id
    }

    /// See-through where it's magenta, 60% opaque elsewhere, click-through, and kept out of
    /// Alt+Tab and from ever becoming the active window.
    fn apply_styles(&self) {
        // SAFETY: `hwnd` is our live window; these calls only change its style flags.
        unsafe {
            let styles = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let wanted = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, styles | wanted);
            SetLayeredWindowAttributes(self.hwnd, KEY_COLOR, (OPACITY * 255.0) as u8, LWA_COLORKEY | LWA_ALPHA);
        }
    }

    fn draw(&mut self, zones: &[ShownZone], highlighted: Option<usize>) -> Result<(), Box<dyn Error>> {
        let bounds = self.bounds;
        let (Some(width), Some(height)) = (NonZeroU32::new(bounds.width as u32), NonZeroU32::new(bounds.height as u32))
        else {
            return Ok(());
        };
        self.surface.resize(width, height)?;
        let shapes = paint::shapes(zones, highlighted, (bounds.left, bounds.top), 1.0);
        let pixels = paint::rasterize(&shapes, bounds.width, bounds.height, KEY_COLOR, pixel);
        let mut buffer = self.surface.buffer_mut()?;
        buffer.copy_from_slice(&pixels);
        buffer.present()?;
        Ok(())
    }

    pub fn window_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                if let Some((zones, highlighted)) = self.last.clone()
                    && let Err(error) = self.draw(&zones, highlighted)
                {
                    eprintln!("zones: could not draw: {error}");
                }
            }
            // Keep our size when the window changes DPI; winit would otherwise rescale it.
            WindowEvent::ScaleFactorChanged { inner_size_writer, .. } => {
                let size = PhysicalSize::new(self.bounds.width.max(1) as u32, self.bounds.height.max(1) as u32);
                let _ = inner_size_writer.clone().request_inner_size(size);
            }
            _ => {}
        }
    }
}

impl super::drag::Overlay for Overlay {
    fn show(&mut self, zones: &[ShownZone], highlighted: Option<usize>) {
        let first = self.last.is_none();
        self.last = Some((zones.to_vec(), highlighted));
        if first {
            // Monitors may have been added, removed or moved since last time.
            self.bounds = virtual_screen();
            self.apply_styles(); // winit may have reset them
        }
        if let Err(error) = self.draw(zones, highlighted) {
            eprintln!("zones: could not draw: {error}");
        }
        if first {
            let b = self.bounds;
            // SAFETY: `hwnd` is our live window.
            unsafe { SetWindowPos(self.hwnd, HWND_TOPMOST, b.left, b.top, b.width, b.height, SWP_NOACTIVATE | SWP_SHOWWINDOW) };
        }
    }

    fn hide(&mut self) {
        self.last = None;
        // SAFETY: `hwnd` is our live window.
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }
}
