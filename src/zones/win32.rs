//! The monitors and windows on Windows, through the Win32 API, in physical pixels: winit makes
//! the app per-monitor DPI aware, so every monitor's pixels are its own.
//!
//! Windows 10 and 11 put an invisible border of a few pixels around windows, for resizing.
//! The zones are about what the user sees, so the bounds here leave it out (DWM's "extended
//! frame bounds"), and moving a window puts the border outside the zone.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GWL_EXSTYLE, GWL_STYLE, GetAncestor, GetCursorPos, GetForegroundWindow, GetShellWindow,
    GetWindowLongPtrW, GetWindowRect, IsIconic, IsWindowVisible, IsZoomed, SW_RESTORE, SW_SHOWMAXIMIZED,
    SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos, ShowWindow, WS_CAPTION, WS_EX_TOOLWINDOW,
    WS_THICKFRAME,
};
use windows_sys::core::BOOL;

use super::desktop::Desktop;
use super::layout::{Monitor, Rect};

const ERROR_ACCESS_DENIED: u32 = 5;

#[derive(Default)]
pub struct SystemDesktop;

impl SystemDesktop {
    pub fn new() -> Result<Self, String> {
        Ok(Self)
    }
}

impl Desktop for SystemDesktop {
    type Window = HWND;

    fn monitors(&self) -> Vec<Monitor> {
        unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, monitors: LPARAM) -> BOOL {
            // SAFETY: `monitors` is the `Vec` passed to EnumDisplayMonitors below, which waits
            // for every call of this.
            let monitors = unsafe { &mut *(monitors as *mut Vec<Monitor>) };
            monitors.extend(monitor_info(monitor));
            1 // go on
        }
        let mut monitors: Vec<Monitor> = Vec::new();
        // SAFETY: `add` only uses `monitors` while this runs.
        unsafe { EnumDisplayMonitors(null_mut(), std::ptr::null(), Some(add), &raw mut monitors as LPARAM) };
        monitors
    }

    fn monitor_of(&self, window: HWND) -> Monitor {
        // SAFETY: plain calls; NEAREST always gives a monitor.
        let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
        monitor_info(monitor).unwrap_or_else(|| {
            let bounds = Rect::new(0, 0, 1920, 1080);
            Monitor { bounds, work_area: bounds }
        })
    }

    fn active_window(&self) -> Option<HWND> {
        // SAFETY: plain call.
        let window = unsafe { GetForegroundWindow() };
        is_movable(window).then_some(window)
    }

    fn window_bounds(&self, window: HWND) -> Rect {
        visible_bounds(window)
    }

    fn move_window(&mut self, window: HWND, bounds: Rect) -> Result<(), String> {
        // SAFETY: plain calls on a window handle; a closed window just makes them fail.
        unsafe {
            if IsIconic(window) != 0 || IsZoomed(window) != 0 {
                ShowWindow(window, SW_RESTORE);
            }
        }
        // Twice: on a monitor with other scaling, the program may change its own size to that
        // scaling after the first move, and the invisible borders with it.
        set_visible_bounds(window, bounds).and_then(|()| set_visible_bounds(window, bounds)).map_err(|error| {
            if error == ERROR_ACCESS_DENIED {
                "Could not move the window: it runs as administrator, and Windows lets only programs that run \
                 as administrator too move it."
                    .to_owned()
            } else {
                format!("Could not move the window: {}", std::io::Error::from_raw_os_error(error as i32))
            }
        })
    }

    fn pointer(&self) -> (i32, i32) {
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: writes the point.
        unsafe { GetCursorPos(&mut point) };
        (point.x, point.y)
    }

    fn is_maximized(&self, window: HWND) -> bool {
        // SAFETY: plain call.
        unsafe { IsZoomed(window) != 0 }
    }

    fn set_maximized(&mut self, window: HWND, maximized: bool) {
        // SAFETY: plain call.
        unsafe { ShowWindow(window, if maximized { SW_SHOWMAXIMIZED } else { SW_RESTORE }) };
    }
}

fn monitor_info(monitor: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        rcWork: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        dwFlags: 0,
    };
    // SAFETY: `info` has its size set, as the call needs.
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0)
        .then(|| Monitor { bounds: rect(info.rcMonitor), work_area: rect(info.rcWork) })
}

fn rect(rect: RECT) -> Rect {
    Rect::from_edges(rect.left, rect.top, rect.right, rect.bottom)
}

/// Whether it's a window the user moves around: a visible top-level window with a title bar
/// or a border to resize it with, and not the desktop or a tool window such as a floating
/// toolbar.
pub fn is_movable(window: HWND) -> bool {
    // SAFETY: plain calls on a window handle.
    unsafe {
        if window.is_null() || window == GetShellWindow() || GetAncestor(window, GA_ROOT) != window || IsWindowVisible(window) == 0 {
            return false;
        }
        let style = GetWindowLongPtrW(window, GWL_STYLE) as u32;
        let extended = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
        extended & WS_EX_TOOLWINDOW == 0 && (style & WS_CAPTION == WS_CAPTION || style & WS_THICKFRAME != 0)
    }
}

/// The window as the user sees it, without the invisible border.
fn visible_bounds(window: HWND) -> Rect {
    let mut frame = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: writes `frame`, of the size given.
    let result = unsafe {
        DwmGetWindowAttribute(window, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&raw mut frame).cast(), size_of::<RECT>() as u32)
    };
    if result != 0 {
        // SAFETY: writes `frame`.
        unsafe { GetWindowRect(window, &mut frame) };
    }
    rect(frame)
}

/// Moves and sizes the window so that what the user sees of it fills `bounds`, without
/// activating it or changing which windows are in front. `Err` is the Windows error code.
fn set_visible_bounds(window: HWND, bounds: Rect) -> Result<(), u32> {
    let mut outer = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: writes `outer`.
    unsafe { GetWindowRect(window, &mut outer) };
    let visible = visible_bounds(window);
    let (left, top) = (visible.left - outer.left, visible.top - outer.top);
    let (right, bottom) = (outer.right - visible.right(), outer.bottom - visible.bottom());

    // SAFETY: plain call on a window handle.
    let moved = unsafe {
        SetWindowPos(
            window,
            null_mut(),
            bounds.left - left,
            bounds.top - top,
            bounds.width + left + right,
            bounds.height + top + bottom,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    };
    // SAFETY: plain call.
    if moved == 0 { Err(unsafe { GetLastError() }) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SW_SHOWNOACTIVATE, WNDCLASSW,
        WS_OVERLAPPEDWINDOW,
    };

    /// A real window of its own, which the user can see for a moment: run with
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore = "opens a window on the screen"]
    fn a_real_window_is_moved_to_fill_the_zone_without_its_invisible_border() {
        let name: Vec<u16> = "my-tools zones test\0".encode_utf16().collect();
        // SAFETY: a plain window class and window, destroyed at the end.
        let window = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&class);
            let window = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                null_mut(),
                null_mut(),
                instance,
                std::ptr::null(),
            );
            ShowWindow(window, SW_SHOWNOACTIVATE);
            window
        };
        assert!(is_movable(window));

        let mut desktop = SystemDesktop;
        let work_area = desktop.monitor_of(window).work_area;
        let zone = Rect::new(work_area.left + 8, work_area.top + 8, 600, 400);
        desktop.move_window(window, zone).unwrap();
        let seen = desktop.window_bounds(window);
        let mut outer = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: writes `outer`; then the window is gone.
        unsafe {
            GetWindowRect(window, &mut outer);
            DestroyWindow(window);
        }
        println!("zone {zone:?}, seen {seen:?}, with the invisible border {:?}", rect(outer));
        assert_eq!(seen, zone);
    }
}
