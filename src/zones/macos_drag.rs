//! Dragging windows into zones on macOS:
//!
//! - **Watching drags**: macOS doesn't tell apps that another app's window is being moved. So
//!   "global monitors" see the mouse in every app (on the main thread; they need the
//!   Accessibility permission the zones need anyway): when the left button is dragged, the
//!   window under where it was pressed moving means it's being dragged.
//! - **Overlay**: a borderless, click-through window over each screen, 60% opaque, with a Core
//!   Animation layer for each rectangle [`paint::shapes`] gives.

use std::cell::RefCell;
use std::error::Error;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSScreen,
    NSScreenSaverWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_application_services::AXUIElement;
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGColor;
use objc2_quartz_core::{CALayer, CATransaction};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::drag::{Pointer, ShownZone, Signal};
use super::layout::Rect;
use super::macos::{bounds_of, pointer, window_at};
use super::paint::{self, OPACITY, Paint};

// ---------------------------------------------------------------------------------------------
// Watching drags
// ---------------------------------------------------------------------------------------------

/// Often enough for the highlighted zone to follow the mouse smoothly; each look at the window
/// asks its app.
const POLL: Duration = Duration::from_millis(15);
/// Esc's key code on every Mac keyboard.
const ESCAPE: u16 = 53;

type Notify = Box<dyn Fn(Signal)>;

/// A press of the left button, which may turn out to drag a window.
struct Press {
    /// Where it was pressed.
    at: (f64, f64),
    /// The window there and where it was, read at the first drag.
    window: Option<(CFRetained<AXUIElement>, Rect)>,
    dragging: bool,
    size: (i32, i32),
    size_changes: u32,
    escape: bool,
    last_look: Instant,
}

struct Watcher {
    notify: Notify,
    press: Option<Press>,
    /// The monitor, kept so it goes on.
    _monitor: Option<Retained<AnyObject>>,
}

thread_local! {
    static WATCHER: RefCell<Option<Watcher>> = const { RefCell::new(None) };
    /// The window being dragged, which [`window`] gives back.
    static DRAGGED: RefCell<Option<CFRetained<AXUIElement>>> = const { RefCell::new(None) };
}

pub fn start_watching(notify: impl Fn(Signal) + Send + Sync + 'static) {
    let block = RcBlock::new(|event: NonNull<NSEvent>| {
        // SAFETY: macOS passes a valid event for the duration of the call.
        let event = unsafe { event.as_ref() };
        WATCHER.with(|watcher| {
            if let Some(watcher) = watcher.borrow_mut().as_mut() {
                watcher.handle(event);
            }
        });
    });
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp
        | NSEventMask::RightMouseDown
        | NSEventMask::KeyDown;
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &block);
    if monitor.is_none() {
        eprintln!("zones: macOS won't tell of window drags, so Shift+drag won't show the zones");
    }
    WATCHER.with(|watcher| *watcher.borrow_mut() = Some(Watcher { notify: Box::new(notify), press: None, _monitor: monitor }));
}

impl Watcher {
    fn handle(&mut self, event: &NSEvent) {
        let kind = event.r#type();
        if kind == NSEventType::LeftMouseDown {
            self.press = Some(Press {
                at: pointer(),
                window: None,
                dragging: false,
                size: (0, 0),
                size_changes: 0,
                escape: false,
                last_look: Instant::now() - POLL,
            });
        } else if kind == NSEventType::LeftMouseDragged {
            if let Some(press) = &mut self.press
                && press.last_look.elapsed() >= POLL
            {
                press.last_look = Instant::now();
                look(press, &self.notify);
            }
        } else if kind == NSEventType::LeftMouseUp {
            if let Some(mut press) = self.press.take()
                && press.dragging
            {
                look(&mut press, &self.notify);
                (self.notify)(Signal::Ended);
            }
        } else if let Some(press) = &mut self.press
            && press.dragging
        {
            // A right click, or a key: Esc cancels the drag.
            press.escape |= kind == NSEventType::KeyDown && event.keyCode() == ESCAPE;
            look(press, &self.notify);
        }
    }
}

/// Looks at the pressed window: tells the drag has started once it moves, and where the mouse
/// is while it's dragged.
fn look(press: &mut Press, notify: &Notify) {
    let Some((window, start)) = &press.window else {
        press.window = window_at(press.at.0, press.at.1).map(|window| {
            let bounds = bounds_of(&window);
            (window, bounds)
        });
        press.size = press.window.as_ref().map_or((0, 0), |(_, bounds)| (bounds.width, bounds.height));
        return;
    };
    let now = bounds_of(window);
    if (now.width, now.height) != press.size {
        press.size = (now.width, now.height);
        press.size_changes += 1;
    }
    if !press.dragging {
        if now == *start {
            return;
        }
        press.dragging = true;
        DRAGGED.with(|dragged| *dragged.borrow_mut() = Some(window.clone()));
        notify(Signal::Started(0));
    }
    let (x, y) = pointer();
    notify(Signal::Moved(Pointer {
        x: x.round() as i32,
        y: y.round() as i32,
        shift: NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Shift),
        resizing: press.size_changes >= 2,
        right_button: NSEvent::pressedMouseButtons() & 0b10 != 0,
        escape: press.escape,
    }));
}

/// The window a [`Signal::Started`] named: the one being dragged.
pub fn window(_id: u64) -> CFRetained<AXUIElement> {
    DRAGGED
        .with(|dragged| dragged.borrow().clone())
        // SAFETY: plain call; moving the system-wide element just fails.
        .unwrap_or_else(|| unsafe { AXUIElement::new_system_wide() })
}

// ---------------------------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------------------------

/// One screen's window, and its frame in points from the top left of the main screen.
struct ScreenWindow {
    window: Retained<NSWindow>,
    frame: Rect,
}

pub struct Overlay {
    mtm: MainThreadMarker,
    screens: Vec<ScreenWindow>,
    /// The AppKit frames the windows were made for, to tell when screens change.
    frames: Vec<CGRect>,
}

impl Overlay {
    pub fn new(_event_loop: &ActiveEventLoop) -> Result<Self, Box<dyn Error>> {
        let mtm = MainThreadMarker::new().ok_or("the zones overlay must be made on the main thread")?;
        Ok(Self { mtm, screens: Vec::new(), frames: Vec::new() })
    }

    /// The overlay uses AppKit windows directly, not winit ones.
    pub fn owns(&self, _id: WindowId) -> bool {
        false
    }

    pub fn window_event(&mut self, _event: &WindowEvent) {}

    /// A window per screen, made again when screens were added, removed or moved.
    fn update_screens(&mut self) {
        let screens = NSScreen::screens(self.mtm);
        let frames: Vec<CGRect> = screens.iter().map(|screen| screen.frame()).collect();
        if frames == self.frames {
            return;
        }
        for screen in &self.screens {
            screen.window.close();
        }
        let main_height = frames.first().map_or(0.0, |main| main.size.height);
        self.screens = frames
            .iter()
            .map(|&frame| ScreenWindow {
                window: self.make_window(frame),
                frame: Rect::new(
                    frame.origin.x.round() as i32,
                    (main_height - frame.origin.y - frame.size.height).round() as i32,
                    frame.size.width.round() as i32,
                    frame.size.height.round() as i32,
                ),
            })
            .collect();
        self.frames = frames;
    }

    fn make_window(&self, frame: CGRect) -> Retained<NSWindow> {
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
        window.setAlphaValue(OPACITY);
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSScreenSaverWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        if let Some(view) = window.contentView() {
            view.setWantsLayer(true);
            if let Some(root) = view.layer() {
                // From the top left, as the shapes are.
                root.setGeometryFlipped(true);
            }
        }
        window
    }
}

fn color(paint: Paint) -> CFRetained<CGColor> {
    let [red, green, blue] = paint::rgb(paint).map(|channel| CGFloat::from(channel) / 255.0);
    CGColor::new_generic_rgb(red, green, blue, 1.0)
}

impl super::drag::Overlay for Overlay {
    fn show(&mut self, zones: &[ShownZone], highlighted: Option<usize>) {
        self.update_screens();
        // All the changes at once, without Core Animation's default fade.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for screen in &self.screens {
            let Some(root) = screen.window.contentView().and_then(|view| view.layer()) else { continue };
            // SAFETY: removing every sublayer; `None` is allowed.
            unsafe { root.setSublayers(None) };
            for (shape, paint) in paint::shapes(zones, highlighted, (screen.frame.left, screen.frame.top), 1.0) {
                let layer = CALayer::new();
                layer.setFrame(CGRect::new(
                    CGPoint::new(shape.left.into(), shape.top.into()),
                    CGSize::new(shape.width.into(), shape.height.into()),
                ));
                layer.setBackgroundColor(Some(&color(paint)));
                root.addSublayer(&layer);
            }
            screen.window.orderFrontRegardless();
        }
        CATransaction::commit();
    }

    fn hide(&mut self) {
        for screen in &self.screens {
            screen.window.orderOut(None);
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        for screen in &self.screens {
            screen.window.close();
        }
    }
}
