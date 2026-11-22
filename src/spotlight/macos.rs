//! macOS side of the spotlight:
//!
//! - **Event tap** (`start_listener`): a CGEventTap sees every key and click in every app. It
//!   only listens (never changes or blocks input). macOS asks the user once to allow this under
//!   System Settings → Privacy & Security → **Input Monitoring**.
//! - **Overlay**: one borderless, see-through, click-through window per display (a macOS window
//!   can't reliably span displays). Each holds two Core Animation shape layers: a 60% black
//!   rectangle with a circular hole (even-odd fill), and a white ring. The GPU draws them, so
//!   moving the circle is just giving the layers new paths.
//!
//! macOS works in *points*, not pixels: Retina scaling is handled for us, and its coordinates
//! start at the bottom-left of the main display, with y going up.

use std::error::Error;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSScreen, NSScreenSaverWindowLevel, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CGFloat, CGPoint, CGRect, CGSize, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGColor, CGEvent, CGEventField, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType, CGMutablePath, CGPreflightListenEventAccess,
    CGRequestListenEventAccess,
};
use objc2_quartz_core::{CAShapeLayer, CATransaction, kCAFillRuleEvenOdd};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::{Point, Signal};

// ---------------------------------------------------------------------------------------------
// Event tap
// ---------------------------------------------------------------------------------------------

/// macOS's key code for Left Control (`kVK_Control`).
const LEFT_CONTROL_KEYCODE: i64 = 59;
/// Bit in an event's flags that's set while *Left* Control is down (`NX_DEVICELCTLKEYMASK`).
const LEFT_CONTROL_DOWN: u64 = 0x0000_0001;
/// How long to wait before trying again when Input Monitoring hasn't been allowed yet.
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// Starts listening to every key and click, calling `notify` when the spotlight should show
/// (Left Control tapped twice) or hide (any key or click while it shows).
pub fn start_listener(notify: impl Fn(Signal) + Send + 'static) {
    super::set_listener(notify);

    let spawned = std::thread::Builder::new()
        .name("spotlight-input".into())
        .spawn(run_event_tap);
    if let Err(error) = spawned {
        eprintln!("spotlight: could not start the input thread: {error}");
    }
}

/// Creates the event tap and runs this thread's run loop, which calls `tap_callback`.
fn run_event_tap() {
    // Asks macOS for Input Monitoring; the first time, it shows the system prompt.
    if !CGPreflightListenEventAccess() {
        CGRequestListenEventAccess();
    }

    let mask: CGEventMask = [
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged, // modifier keys (Control, Shift, ...) only send this
        CGEventType::LeftMouseDown,
        CGEventType::RightMouseDown,
        CGEventType::OtherMouseDown,
    ]
    .iter()
    .fold(0, |mask, kind| mask | (1 << kind.0));

    // Until the user allows Input Monitoring, creating the tap fails: keep trying.
    let mut warned = false;
    let tap = loop {
        // SAFETY: `tap_callback` matches the callback type; it gets no user data yet (null).
        let tap = unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                mask,
                Some(tap_callback),
                std::ptr::null_mut(),
            )
        };
        if let Some(tap) = tap {
            break tap;
        }
        if !warned {
            eprintln!(
                "spotlight: allow My Tools under System Settings → Privacy & Security → \
                 Input Monitoring to use the double-Control spotlight"
            );
            warned = true;
        }
        std::thread::sleep(RETRY_AFTER);
    };
    TAP.with(|cell| *cell.borrow_mut() = Some(tap.clone()));

    let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
        eprintln!("spotlight: could not listen to the keyboard and mouse");
        return;
    };
    let Some(run_loop) = CFRunLoop::current() else {
        return;
    };
    // SAFETY: reading an Apple-provided constant.
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
    CFRunLoop::run(); // runs forever, calling tap_callback for each event
}

thread_local! {
    /// The tap, so the callback can switch it back on (see below). Only the tap thread uses it.
    static TAP: std::cell::RefCell<Option<CFRetained<CFMachPort>>> =
        const { std::cell::RefCell::new(None) };
}

/// Called by macOS for each key or click in any app. It must return the event unchanged.
unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    _user_info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: macOS passes a valid event that lives for the duration of this call.
    let event_ref = unsafe { event.as_ref() };

    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        // macOS switches off taps it thinks are too slow; switch it back on.
        TAP.with(|cell| {
            if let Some(tap) = cell.borrow().as_ref() {
                CGEvent::tap_enable(tap, true);
            }
        });
    } else if kind == CGEventType::KeyDown {
        super::key_down(false);
    } else if kind == CGEventType::KeyUp {
        super::key_up(false);
    } else if kind == CGEventType::FlagsChanged {
        let keycode =
            CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode);
        if keycode == LEFT_CONTROL_KEYCODE {
            let flags = CGEvent::flags(Some(event_ref)).0;
            if flags & LEFT_CONTROL_DOWN != 0 {
                super::key_down(true);
            } else {
                super::key_up(true);
            }
        } else {
            // Another modifier (Shift, Option, Command...) went down or up.
            super::key_down(false);
        }
    } else {
        super::click(); // one of the mouse-down events we asked for
    }
    event.as_ptr()
}

/// Where the mouse pointer is, in points (bottom-left origin, like the overlay windows).
pub fn cursor_position() -> Point {
    let location = NSEvent::mouseLocation();
    (location.x.round() as i32, location.y.round() as i32)
}

// ---------------------------------------------------------------------------------------------
// Overlay windows
// ---------------------------------------------------------------------------------------------

/// How dark the dimmed area is.
const OPACITY: CGFloat = 0.6;
/// Width of the white ring, in points.
const RING_WIDTH: CGFloat = 4.0;

/// One display's overlay window.
struct ScreenOverlay {
    window: Retained<NSWindow>,
    /// The display's area, in global points.
    frame: CGRect,
    /// Dim rectangle with a hole cut out.
    dim: Retained<CAShapeLayer>,
    /// White ring around the hole.
    ring: Retained<CAShapeLayer>,
}

pub struct Overlay {
    mtm: MainThreadMarker,
    screens: Vec<ScreenOverlay>,
}

impl Overlay {
    pub fn new(_event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        // AppKit windows may only be touched on the main thread; this proves we're on it.
        let mtm = MainThreadMarker::new().ok_or("the spotlight must start on the main thread")?;
        Ok(Self { mtm, screens: Vec::new() })
    }

    /// The overlay uses AppKit windows directly, not winit ones.
    pub fn owns(&self, _id: WindowId) -> bool {
        false
    }

    pub fn window_event(&mut self, _id: WindowId, _event: &WindowEvent) {}

    pub fn show(&mut self, _event_loop: &ActiveEventLoop) {
        // Displays may have been added, removed or moved since last time.
        let frames: Vec<CGRect> = NSScreen::screens(self.mtm).iter().map(|s| s.frame()).collect();
        let current: Vec<CGRect> = self.screens.iter().map(|s| s.frame).collect();
        if frames != current {
            self.close_all();
            self.screens = frames.into_iter().map(|frame| self.make_window(frame)).collect();
        }
        for screen in &self.screens {
            screen.window.orderFrontRegardless(); // show without becoming the active app
        }
    }

    pub fn hide(&mut self) {
        for screen in &self.screens {
            screen.window.orderOut(None);
        }
    }

    /// Cuts a hole of `radius` points around `center` (global points) in every display's overlay.
    pub fn draw(&mut self, center: Point, radius: i32) {
        let radius = CGFloat::from(radius);

        // Apply all the changes at once, without Core Animation's default fade.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for screen in &self.screens {
            // The circle in this window's own coordinates.
            let circle = CGRect::new(
                CGPoint::new(
                    CGFloat::from(center.0) - screen.frame.origin.x - radius,
                    CGFloat::from(center.1) - screen.frame.origin.y - radius,
                ),
                CGSize::new(radius * 2.0, radius * 2.0),
            );
            let whole = CGRect::new(CGPoint::new(0.0, 0.0), screen.frame.size);

            let dim = CGMutablePath::new();
            let ring = CGMutablePath::new();
            // SAFETY: null transform means "none"; the paths are valid and ours.
            unsafe {
                CGMutablePath::add_rect(Some(&dim), std::ptr::null(), whole);
                CGMutablePath::add_ellipse_in_rect(Some(&dim), std::ptr::null(), circle);
                CGMutablePath::add_ellipse_in_rect(Some(&ring), std::ptr::null(), circle);
            }
            screen.dim.setPath(Some(&dim));
            screen.ring.setPath(Some(&ring));
        }
        CATransaction::commit();
    }

    /// Makes a hidden overlay window covering one display.
    fn make_window(&self, frame: CGRect) -> ScreenOverlay {
        // SAFETY: standard NSWindow initialiser with valid arguments.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                self.mtm.alloc(),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: we keep our own reference to the window, so AppKit mustn't free it on close.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true); // clicks go through to the apps underneath
        window.setLevel(NSScreenSaverWindowLevel); // above everything, menu bar and Dock too
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces // on every desktop (Space)
                | NSWindowCollectionBehavior::Stationary // stays put in Mission Control
                | NSWindowCollectionBehavior::FullScreenAuxiliary // over full-screen apps too
                | NSWindowCollectionBehavior::IgnoresCycle, // not in Cmd+` window cycling
        );

        let dim = CAShapeLayer::new();
        // SAFETY: reading an Apple-provided constant.
        dim.setFillRule(unsafe { kCAFillRuleEvenOdd }); // the circle inside the rectangle is a hole
        dim.setFillColor(Some(&CGColor::new_generic_rgb(0.0, 0.0, 0.0, OPACITY)));

        let ring = CAShapeLayer::new();
        ring.setFillColor(None);
        ring.setStrokeColor(Some(&CGColor::new_generic_rgb(1.0, 1.0, 1.0, 1.0)));
        ring.setLineWidth(RING_WIDTH);

        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
        dim.setFrame(bounds);
        ring.setFrame(bounds);

        if let Some(view) = window.contentView() {
            view.setWantsLayer(true); // back the view with a Core Animation layer
            if let Some(root) = view.layer() {
                root.addSublayer(&dim);
                root.addSublayer(&ring);
            }
        }

        ScreenOverlay { window, frame, dim, ring }
    }

    fn close_all(&mut self) {
        for screen in self.screens.drain(..) {
            screen.window.orderOut(None);
            screen.window.close();
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.close_all();
    }
}
