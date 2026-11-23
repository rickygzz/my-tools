//! What the zones need from the OS: the monitors, and the windows of every program. Each OS
//! implements [`Desktop`] in its own file; the rest of the zones code only sees the trait, so
//! it can be tested with [`testing::FakeDesktop`].

use super::layout::{Monitor, Rect};

/// The monitors and the windows of every program, in screen pixels.
pub trait Desktop {
    /// A window of any program, as the OS names it: a handle on Windows and X11, a reference-
    /// counted Accessibility object on macOS.
    type Window: Clone;

    /// Every monitor connected now.
    fn monitors(&self) -> Vec<Monitor>;

    /// The monitor the window is on, or the nearest one.
    fn monitor_of(&self, window: Self::Window) -> Monitor;

    /// The window the user works in; `None` when there's none.
    fn active_window(&self) -> Option<Self::Window>;

    /// The window as the user sees it: without the invisible borders some OSes put around it.
    fn window_bounds(&self, window: Self::Window) -> Rect;

    /// Moves and sizes the window so that what the user sees of it fills `bounds`. `Err` says
    /// why it couldn't, for the user.
    fn move_window(&mut self, window: Self::Window, bounds: Rect) -> Result<(), String>;

    /// Where the mouse pointer is.
    fn pointer(&self) -> (i32, i32);

    /// Whether the window fills its monitor, maximized.
    fn is_maximized(&self, window: Self::Window) -> bool;

    /// Maximizes the window, or gives it back its size from before.
    fn set_maximized(&mut self, window: Self::Window, maximized: bool);
}

/// The monitor a window is on: the one its middle is in, or the nearest.
#[cfg_attr(windows, allow(dead_code))]
pub fn nearest_monitor(monitors: Vec<Monitor>, window: Rect) -> Monitor {
    let (x, y) = (window.left + window.width / 2, window.top + window.height / 2);
    let distance = |monitor: &Monitor| {
        let bounds = monitor.bounds;
        let dx = i64::from((bounds.left - x).max(x - bounds.right() + 1).max(0));
        let dy = i64::from((bounds.top - y).max(y - bounds.bottom() + 1).max(0));
        dx * dx + dy * dy
    };
    monitors.into_iter().min_by_key(distance).unwrap_or_else(|| {
        let bounds = Rect::new(0, 0, 1920, 1080);
        Monitor { bounds, work_area: bounds }
    })
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use crate::zones::layout::{Layout, Layouts, columns, equal};

    pub const WINDOW: u32 = 42;
    pub const LEFT: Rect = Rect::new(0, 0, 1000, 600);
    pub const RIGHT: Rect = Rect::new(1000, 0, 1000, 600);

    /// Two monitors side by side, 1000 by 600, with one window, and [`FakeDesktop::layouts`]
    /// of two zones a monitor, in halves, without a gap. Records where the window is moved.
    pub struct FakeDesktop {
        pub active: Option<u32>,
        /// The window as the user sees it; on the left monitor at first.
        pub window_bounds: Rect,
        /// How narrow the window can be made, as some programs keep their windows.
        pub minimum_width: i32,
        /// When set, `move_window` fails with it.
        pub move_problem: Option<String>,
        pub moves: Vec<(u32, Rect)>,
        pub pointer: (i32, i32),
        pub maximized: bool,
        /// What was done to the window: "maximize" or "restore".
        pub changes: Vec<&'static str>,
    }

    impl Default for FakeDesktop {
        fn default() -> Self {
            Self {
                active: Some(WINDOW),
                window_bounds: Rect::new(100, 100, 300, 200),
                minimum_width: 0,
                move_problem: None,
                moves: Vec::new(),
                pointer: (500, 300),
                maximized: false,
                changes: Vec::new(),
            }
        }
    }

    impl FakeDesktop {
        pub fn layouts() -> Layouts {
            Layouts { default: Layout { zones: columns(&equal(2)), gap: 0 }, monitors: vec![] }
        }
    }

    impl Desktop for FakeDesktop {
        type Window = u32;

        fn monitors(&self) -> Vec<Monitor> {
            [LEFT, RIGHT].map(|bounds| Monitor { bounds, work_area: bounds }).to_vec()
        }

        fn monitor_of(&self, _window: u32) -> Monitor {
            let bounds = if self.window_bounds.left >= RIGHT.left { RIGHT } else { LEFT };
            Monitor { bounds, work_area: bounds }
        }

        fn active_window(&self) -> Option<u32> {
            self.active
        }

        fn window_bounds(&self, _window: u32) -> Rect {
            self.window_bounds
        }

        fn move_window(&mut self, window: u32, bounds: Rect) -> Result<(), String> {
            if let Some(problem) = &self.move_problem {
                return Err(problem.clone());
            }
            self.moves.push((window, bounds));
            self.window_bounds = Rect { width: bounds.width.max(self.minimum_width), ..bounds };
            Ok(())
        }

        fn pointer(&self) -> (i32, i32) {
            self.pointer
        }

        fn is_maximized(&self, _window: u32) -> bool {
            self.maximized
        }

        fn set_maximized(&mut self, _window: u32, maximized: bool) {
            self.maximized = maximized;
            self.changes.push(if maximized { "maximize" } else { "restore" });
        }
    }
}
