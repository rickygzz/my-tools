//! The monitors and windows on macOS. Apps can't move other apps' windows there directly; the
//! Accessibility API can, once the user allows it under System Settings → Privacy & Security
//! → **Accessibility**. The first time, macOS shows a dialog that opens that page.
//!
//! Everything here is in points (not pixels), from the top left corner of the main screen,
//! as the Accessibility API has windows. AppKit has screens from the *bottom* left, so their
//! frames are turned over.
//!
//! macOS has no "maximized" as Windows does (the green button makes a window full screen, a
//! space of its own, which can't be moved), so windows are never seen as maximized.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_application_services::{
    AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions, AXUIElement, AXValue, AXValueType,
    kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGEvent;

use super::desktop::{Desktop, nearest_monitor};
use super::layout::{Monitor, Rect};

pub struct SystemDesktop {
    mtm: MainThreadMarker,
}

impl SystemDesktop {
    /// Fails, for the user, when the app isn't allowed to move windows; the first time, macOS
    /// also shows its dialog to allow it.
    pub fn new() -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("the zones only work on the main thread")?;
        // SAFETY: plain calls; the options are a dictionary of the one key the call takes.
        let trusted = unsafe {
            AXIsProcessTrusted() || {
                let options = CFDictionary::<CFString, CFBoolean>::from_slices(
                    &[kAXTrustedCheckOptionPrompt],
                    &[CFBoolean::new(true)],
                );
                AXIsProcessTrustedWithOptions(Some(options.as_opaque()))
            }
        };
        if !trusted {
            return Err("To move windows, my-tools needs your permission: in System Settings → Privacy & \
                        Security → Accessibility, turn on my-tools, then try again."
                .into());
        }
        Ok(Self { mtm })
    }
}

/// The value of an element's attribute, such as "AXPosition".
fn attribute(element: &AXUIElement, name: &'static str) -> Option<CFRetained<CFType>> {
    let name = CFString::from_static_str(name);
    let mut value: *const CFType = std::ptr::null();
    // SAFETY: `value` is a valid place for the result, which we then own.
    let error = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
    if error != AXError::Success {
        return None;
    }
    // SAFETY: a "Copy" function gives a value we must release, which CFRetained does.
    NonNull::new(value.cast_mut()).map(|value| unsafe { CFRetained::from_raw(value) })
}

fn element(element: &AXUIElement, name: &'static str) -> Option<CFRetained<AXUIElement>> {
    attribute(element, name)?.downcast::<AXUIElement>().ok()
}

/// A point or size kept in an AXValue.
fn read_value<T: Default>(element: &AXUIElement, name: &'static str, kind: AXValueType) -> Option<T> {
    let value = attribute(element, name)?.downcast::<AXValue>().ok()?;
    let mut read = T::default();
    // SAFETY: `kind` is the type of `T` (CGPoint or CGSize), so the value fits.
    unsafe { value.value(kind, NonNull::from(&mut read).cast::<c_void>()) }.then_some(read)
}

fn write_value<T>(element: &AXUIElement, name: &'static str, kind: AXValueType, mut value: T) -> Result<(), AXError> {
    // SAFETY: `kind` is the type of `T`, which AXValueCreate copies.
    let value = unsafe { AXValue::new(kind, NonNull::from(&mut value).cast::<c_void>()) }.ok_or(AXError::Failure)?;
    let name = CFString::from_static_str(name);
    // SAFETY: the attribute takes a value of this type.
    match unsafe { element.set_attribute_value(&name, &value) } {
        AXError::Success => Ok(()),
        error => Err(error),
    }
}

/// A rectangle from AppKit (from the bottom of the main screen) as one from its top.
fn turned_over(frame: CGRect, main_height: f64) -> Rect {
    let top = main_height - (frame.origin.y + frame.size.height);
    Rect::new(
        frame.origin.x.round() as i32,
        top.round() as i32,
        frame.size.width.round() as i32,
        frame.size.height.round() as i32,
    )
}

impl Desktop for SystemDesktop {
    type Window = CFRetained<AXUIElement>;

    fn monitors(&self) -> Vec<Monitor> {
        screens(self.mtm)
    }

    fn monitor_of(&self, window: Self::Window) -> Monitor {
        nearest_monitor(self.monitors(), self.window_bounds(window))
    }

    fn active_window(&self) -> Option<Self::Window> {
        // SAFETY: plain call.
        let everything = unsafe { AXUIElement::new_system_wide() };
        let app = element(&everything, "AXFocusedApplication")?;
        element(&app, "AXFocusedWindow")
    }

    fn window_bounds(&self, window: Self::Window) -> Rect {
        bounds_of(&window)
    }

    fn move_window(&mut self, window: Self::Window, bounds: Rect) -> Result<(), String> {
        let position = CGPoint::new(f64::from(bounds.left), f64::from(bounds.top));
        let size = CGSize::new(f64::from(bounds.width), f64::from(bounds.height));
        // Size, place, then size again: macOS keeps a window inside its screen, so going to
        // another screen the first size may be cut short.
        write_value(&window, "AXSize", AXValueType::CGSize, size)
            .and_then(|()| write_value(&window, "AXPosition", AXValueType::CGPoint, position))
            .and_then(|()| write_value(&window, "AXSize", AXValueType::CGSize, size))
            .map_err(|error| match error {
                AXError::APIDisabled => "Could not move the window: my-tools isn't allowed to (System Settings → \
                                         Privacy & Security → Accessibility)."
                    .to_owned(),
                AXError::AttributeUnsupported | AXError::CannotComplete => {
                    "Could not move the window: its app doesn't let it be moved or sized.".to_owned()
                }
                error => format!("Could not move the window (error {}).", error.0),
            })
    }

    fn pointer(&self) -> (i32, i32) {
        let (x, y) = pointer();
        (x.round() as i32, y.round() as i32)
    }

    fn is_maximized(&self, _window: Self::Window) -> bool {
        false
    }

    fn set_maximized(&mut self, _window: Self::Window, _maximized: bool) {}
}

/// Where a window is, in points from the top left of the main screen.
pub(super) fn bounds_of(window: &AXUIElement) -> Rect {
    let position: CGPoint = read_value(window, "AXPosition", AXValueType::CGPoint).unwrap_or_default();
    let size: CGSize = read_value(window, "AXSize", AXValueType::CGSize).unwrap_or_default();
    Rect::new(position.x.round() as i32, position.y.round() as i32, size.width.round() as i32, size.height.round() as i32)
}

/// The window at a point of the screen, if the app is allowed to see it (without asking).
pub(super) fn window_at(x: f64, y: f64) -> Option<CFRetained<AXUIElement>> {
    // SAFETY: plain call.
    if !unsafe { AXIsProcessTrusted() } {
        return None;
    }
    // SAFETY: plain calls; `found` is a valid place for the result, which we then own.
    let found = unsafe {
        let everything = AXUIElement::new_system_wide();
        // A hung app mustn't hang ours: this runs on the main thread.
        everything.set_messaging_timeout(0.2);
        let mut found: *const AXUIElement = std::ptr::null();
        if everything.copy_element_at_position(x as f32, y as f32, NonNull::from(&mut found)) != AXError::Success {
            return None;
        }
        CFRetained::from_raw(NonNull::new(found.cast_mut())?)
    };
    // The element is a button or text in the window, or the window itself.
    element(&found, "AXWindow").or(Some(found))
}

/// Where the mouse pointer is, in points from the top left of the main screen.
pub(super) fn pointer() -> (f64, f64) {
    let location = CGEvent::new(None).map_or(CGPoint::default(), |event| CGEvent::location(Some(&event)));
    (location.x, location.y)
}

/// Every monitor connected now, for the zone editor (which needs no permission).
pub fn monitors() -> Vec<Monitor> {
    MainThreadMarker::new().map(screens).unwrap_or_default()
}

fn screens(mtm: MainThreadMarker) -> Vec<Monitor> {
    let screens = NSScreen::screens(mtm);
    // The first screen is the main one, whose bottom left corner is AppKit's (0, 0).
    let Some(main_height) = screens.iter().next().map(|main| main.frame().size.height) else {
        return Vec::new();
    };
    screens
        .iter()
        .map(|screen| Monitor {
            bounds: turned_over(screen.frame(), main_height),
            work_area: turned_over(screen.visibleFrame(), main_height),
        })
        .collect()
}
