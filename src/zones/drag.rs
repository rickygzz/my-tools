//! Dragging a window into a zone. While the user drags a window, a press of Shift, or a click
//! of the right mouse button, shows the zones of every monitor, with the one under the mouse
//! highlighted. They stay until the next press, so Shift needn't be held. Letting go of the
//! window over a zone moves it into that zone. Esc cancels the drag: the window goes back,
//! into no zone. A window being resized shows none.
//!
//! The layouts are read once a drag, the first time the zones show, so a change to
//! `zones.cfg` applies at the next drag, and most drags (which never show the zones) don't
//! read it at all.
//!
//! Each OS tells of drags with [`Signal`]s; this only decides what to show and where the
//! window goes, so it's tested without any.

use super::geometry::{place, zone_at};
use super::layout::{Layouts, Monitor, Rect};

/// Where the mouse is while a window is dragged, and the keys and buttons the zones use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
    /// Held: a press shows the zones, the next hides them.
    pub shift: bool,
    /// The window is being resized by an edge, not moved: no zones then.
    pub resizing: bool,
    /// Held: a click works as Shift.
    pub right_button: bool,
    /// Down, or pressed since the last [`Pointer`]: the drag is cancelled.
    pub escape: bool,
}

/// What each OS tells of a drag of another app's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The user started moving or resizing the window: its handle, as the OS module keeps it.
    Started(u64),
    /// Now and then while the window is dragged.
    Moved(Pointer),
    /// The user let go of the window.
    Ended,
}

/// A zone as the overlay shows it: in screen pixels, with its number on its monitor (the one
/// its hotkey moves windows to).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShownZone {
    pub bounds: Rect,
    pub number: usize,
}

/// Shows the zones over every monitor while a window is dragged, without taking the mouse.
pub trait Overlay {
    /// Shows the zones, or shows another one highlighted (`None` for none).
    fn show(&mut self, zones: &[ShownZone], highlighted: Option<usize>);
    fn hide(&mut self);
}

/// The zones of every monitor, its own layout or the default one, and the widest gap: a point
/// in it is in the zones on either side, on any monitor.
pub fn zones_of(monitors: &[Monitor], layouts: &Layouts) -> (Vec<ShownZone>, i32) {
    let gap = monitors.iter().map(|monitor| layouts.for_monitor(monitor.bounds).gap).max().unwrap_or(0);
    let zones = monitors
        .iter()
        .flat_map(|monitor| {
            place(layouts.for_monitor(monitor.bounds), monitor.work_area)
                .into_iter()
                .enumerate()
                .map(|(index, bounds)| ShownZone { bounds, number: index + 1 })
        })
        .collect();
    (zones, gap)
}

#[derive(Default)]
pub struct DragPresenter {
    /// The window being dragged.
    window: Option<u64>,
    /// The zones of this drag on every monitor, and the gap, once read.
    zones: Option<(Vec<ShownZone>, i32)>,
    shown: bool,
    highlighted: Option<usize>,
    /// Whether the user asked for the zones in this drag, which each press turns over.
    asked: bool,
    /// What was down at the last move, to tell a press.
    was_shift: bool,
    was_right_button: bool,
    /// Esc cancelled the drag.
    cancelled: bool,
}

impl DragPresenter {
    pub fn started(&mut self, window: u64, overlay: &mut dyn Overlay) {
        self.hide(overlay);
        *self = Self { window: Some(window), ..Self::default() };
    }

    /// `read_zones` gives [`zones_of`] the monitors and layouts now; it's only called once a
    /// drag, when the zones first show.
    pub fn moved(&mut self, pointer: Pointer, overlay: &mut dyn Overlay, read_zones: impl FnOnce() -> (Vec<ShownZone>, i32)) {
        if self.window.is_none() || self.cancelled {
            return;
        }
        // The OS puts the window back on Esc; it mustn't then go into a zone.
        if pointer.escape {
            self.cancelled = true;
            self.hide(overlay);
            return;
        }

        let pressed = (pointer.shift && !self.was_shift) || (pointer.right_button && !self.was_right_button);
        self.was_shift = pointer.shift;
        self.was_right_button = pointer.right_button;
        if pressed {
            self.asked = !self.asked;
        }
        if !self.asked || pointer.resizing {
            self.hide(overlay);
            return;
        }

        let (zones, gap) = self.zones.get_or_insert_with(read_zones);
        let bounds: Vec<Rect> = zones.iter().map(|zone| zone.bounds).collect();
        let highlighted = zone_at(&bounds, pointer.x, pointer.y, *gap);
        if !self.shown || highlighted != self.highlighted {
            self.shown = true;
            self.highlighted = highlighted;
            overlay.show(zones, highlighted);
        }
    }

    /// Hides the zones. Returns the window and the zone it goes into, if it was let go of
    /// over one.
    pub fn ended(&mut self, overlay: &mut dyn Overlay) -> Option<(u64, Rect)> {
        let zone = if self.shown && !self.cancelled { self.highlighted } else { None };
        self.hide(overlay);
        let window = self.window.take()?;
        let (zones, _) = self.zones.as_ref()?;
        Some((window, zones[zone?].bounds))
    }

    fn hide(&mut self, overlay: &mut dyn Overlay) {
        if self.shown {
            overlay.hide();
        }
        self.shown = false;
        self.highlighted = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::desktop::testing::{FakeDesktop, RIGHT};
    use crate::zones::desktop::Desktop;
    use crate::zones::layout::{Layout, columns, equal, rows};

    const WINDOW: u64 = 42;

    /// The zones of both monitors, left to right.
    const ZONES: [Rect; 4] =
        [Rect::new(0, 0, 500, 600), Rect::new(500, 0, 500, 600), Rect::new(1000, 0, 500, 600), Rect::new(1500, 0, 500, 600)];

    #[derive(Default)]
    struct FakeOverlay {
        shown: Vec<(Vec<Rect>, Option<usize>)>,
        hides: usize,
    }

    impl Overlay for FakeOverlay {
        fn show(&mut self, zones: &[ShownZone], highlighted: Option<usize>) {
            self.shown.push((zones.iter().map(|zone| zone.bounds).collect(), highlighted));
        }

        fn hide(&mut self) {
            self.hides += 1;
        }
    }

    /// A drag on the fake desktop's two monitors, counting how often the layouts are read.
    struct Drag {
        presenter: DragPresenter,
        overlay: FakeOverlay,
        layouts: Layouts,
        loads: usize,
    }

    impl Drag {
        fn new() -> Self {
            Self { presenter: DragPresenter::default(), overlay: FakeOverlay::default(), layouts: FakeDesktop::layouts(), loads: 0 }
        }

        fn start(&mut self) {
            self.presenter.started(WINDOW, &mut self.overlay);
        }

        fn mv(&mut self, x: i32, y: i32, pointer: Pointer) {
            let monitors = FakeDesktop::default().monitors();
            let (layouts, loads) = (&self.layouts, &mut self.loads);
            self.presenter.moved(Pointer { x, y, ..pointer }, &mut self.overlay, || {
                *loads += 1;
                zones_of(&monitors, layouts)
            });
        }

        fn shift(&mut self, x: i32, y: i32) {
            self.mv(x, y, Pointer { shift: true, ..Pointer::default() });
        }

        fn plain(&mut self, x: i32, y: i32) {
            self.mv(x, y, Pointer::default());
        }

        fn end(&mut self) -> Option<(u64, Rect)> {
            self.presenter.ended(&mut self.overlay)
        }
    }

    #[test]
    fn a_drag_without_shift_shows_nothing_moves_nothing_and_reads_nothing() {
        let mut drag = Drag::new();
        drag.start();
        drag.plain(700, 300);
        assert_eq!(drag.end(), None);
        assert!(drag.overlay.shown.is_empty());
        assert_eq!(drag.loads, 0);
    }

    #[test]
    fn shift_shows_the_zones_of_every_monitor_with_the_one_under_the_mouse() {
        let mut drag = Drag::new();
        drag.start();
        drag.shift(1700, 300);
        assert_eq!(drag.overlay.shown, [(ZONES.to_vec(), Some(3))]);
    }

    #[test]
    fn the_numbers_start_from_1_on_each_monitor() {
        let monitors = FakeDesktop::default().monitors();
        let numbers: Vec<usize> = zones_of(&monitors, &FakeDesktop::layouts()).0.iter().map(|zone| zone.number).collect();
        assert_eq!(numbers, [1, 2, 1, 2]);
    }

    #[test]
    fn moving_in_the_same_zone_doesnt_show_it_again_another_zone_does() {
        let mut drag = Drag::new();
        drag.start();
        drag.shift(100, 100);
        drag.shift(200, 150);
        drag.shift(600, 150);
        let highlighted: Vec<Option<usize>> = drag.overlay.shown.iter().map(|(_, highlighted)| *highlighted).collect();
        assert_eq!(highlighted, [Some(0), Some(1)]);
    }

    #[test]
    fn letting_go_over_a_zone_moves_the_window_there_and_hides_the_zones() {
        let mut drag = Drag::new();
        drag.start();
        drag.shift(600, 300);
        assert_eq!(drag.end(), Some((WINDOW, ZONES[1])));
        assert_eq!(drag.overlay.hides, 1);
    }

    #[test]
    fn letting_go_of_shift_keeps_the_zones() {
        let mut drag = Drag::new();
        drag.start();
        drag.shift(600, 300);
        drag.plain(620, 300);
        drag.plain(1700, 300);
        assert_eq!(drag.end(), Some((WINDOW, ZONES[3])));
    }

    #[test]
    fn shift_held_is_one_press_another_hides_the_zones_and_a_third_shows_them_without_reading_them_again() {
        let mut drag = Drag::new();
        drag.start();
        drag.shift(600, 300);
        drag.shift(610, 300);
        drag.shift(620, 300);
        drag.plain(630, 300);
        drag.shift(640, 300);
        assert_eq!((drag.overlay.shown.len(), drag.overlay.hides), (1, 1));
        drag.plain(650, 300);
        drag.shift(660, 300);
        assert_eq!(drag.overlay.shown.len(), 2);
        assert_eq!(drag.loads, 1);
    }

    #[test]
    fn shift_pressed_twice_leaves_no_zones_so_the_window_drops_where_it_is() {
        let mut drag = Drag::new();
        drag.start();
        drag.shift(600, 300);
        drag.plain(610, 300);
        drag.shift(620, 300);
        assert_eq!(drag.end(), None);
    }

    #[test]
    fn a_right_click_shows_the_zones_as_shift_does() {
        let mut drag = Drag::new();
        drag.start();
        drag.mv(600, 300, Pointer { right_button: true, ..Pointer::default() });
        drag.plain(1700, 300);
        assert_eq!(drag.end(), Some((WINDOW, ZONES[3])));
    }

    #[test]
    fn escape_cancels_the_drag_even_with_shift_again() {
        let mut drag = Drag::new();
        drag.start();
        drag.shift(600, 300);
        drag.mv(610, 300, Pointer { escape: true, ..Pointer::default() });
        drag.shift(620, 300);
        assert_eq!(drag.end(), None);
        assert_eq!((drag.overlay.shown.len(), drag.overlay.hides), (1, 1));
    }

    #[test]
    fn a_window_being_resized_shows_no_zones_even_with_shift() {
        let mut drag = Drag::new();
        drag.start();
        drag.mv(600, 300, Pointer { shift: true, resizing: true, ..Pointer::default() });
        assert_eq!(drag.end(), None);
        assert!(drag.overlay.shown.is_empty());
    }

    #[test]
    fn each_drag_reads_the_layouts_again() {
        let mut drag = Drag::new();
        drag.start();
        drag.shift(600, 300);
        drag.end();
        drag.layouts.default = Layout { zones: columns(&[100.0]), gap: 0 };
        drag.start();
        drag.shift(600, 300);
        let last = &drag.overlay.shown.last().unwrap().0;
        assert_eq!(last, &[Rect::new(0, 0, 1000, 600), RIGHT]);
    }

    #[test]
    fn a_monitor_with_its_own_layout_shows_it_the_others_the_default() {
        let mut drag = Drag::new();
        drag.layouts.monitors.push((RIGHT, Layout { zones: rows(&equal(2)), gap: 0 }));
        drag.start();
        drag.shift(1500, 500);
        assert_eq!(
            drag.overlay.shown[0].0,
            [ZONES[0], ZONES[1], Rect::new(1000, 0, 1000, 300), Rect::new(1000, 300, 1000, 300)]
        );
        assert_eq!(drag.end(), Some((WINDOW, Rect::new(1000, 300, 1000, 300))));
    }

    #[test]
    fn moves_without_a_drag_are_ignored() {
        let mut drag = Drag::new();
        drag.shift(600, 300);
        assert_eq!(drag.end(), None);
        assert!(drag.overlay.shown.is_empty());
    }
}
