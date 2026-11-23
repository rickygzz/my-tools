//! Moves the window the user works in to a zone of its monitor, by number or to the next one,
//! or to the monitor the mouse pointer is on.

use super::desktop::Desktop;
use super::geometry::{move_to_area, place, zone_of};
use super::layout::{Layouts, Rect};

/// Moves the active window to zone `number`, from 1. `Err` says why it couldn't, for the user.
pub fn move_to(desktop: &mut impl Desktop, layouts: &Layouts, number: usize) -> Result<(), String> {
    let (window, zones) = active_window_zones(desktop, layouts)?;
    if !(1..=zones.len()).contains(&number) {
        let count = zones.len();
        return Err(format!("There's no zone {number}: there are {count}, from 1 to {count}."));
    }
    desktop.move_window(window, zones[number - 1])
}

/// Moves the active window to the zone after the one it's in (`forward`), or before it; from
/// the last zone to the first and back. A window in no zone goes to the first one, or the last.
pub fn move_by(desktop: &mut impl Desktop, layouts: &Layouts, forward: bool) -> Result<(), String> {
    let (window, zones) = active_window_zones(desktop, layouts)?;
    let count = zones.len();
    let next = match (zone_of(&zones, desktop.window_bounds(window)), forward) {
        (Some(index), true) => (index + 1) % count,
        (Some(index), false) => (index + count - 1) % count,
        (None, true) => 0,
        (None, false) => count - 1,
    };
    desktop.move_window(window, zones[next])
}

/// Moves the active window to the monitor the mouse pointer is on, at the same place in it and
/// as large, no larger than it; a maximized window stays maximized. Nothing happens when it's
/// on that monitor already.
pub fn move_to_pointer(desktop: &mut impl Desktop) -> Result<(), String> {
    let window = desktop.active_window().ok_or(NO_WINDOW)?;
    let (x, y) = desktop.pointer();
    let from = desktop.monitor_of(window);
    let Some(to) = desktop.monitors().into_iter().find(|monitor| monitor.bounds.contains(x, y)) else {
        return Ok(());
    };
    if to.bounds == from.bounds {
        return Ok(());
    }

    let maximized = desktop.is_maximized(window);
    if maximized {
        desktop.set_maximized(window, false);
    }
    let bounds = move_to_area(desktop.window_bounds(window), from.work_area, to.work_area);
    let moved = desktop.move_window(window, bounds);
    // Maximized again even when it couldn't move: as it was.
    if maximized {
        desktop.set_maximized(window, true);
    }
    moved
}

const NO_WINDOW: &str = "There's no window to move.";

fn active_window_zones<D: Desktop>(desktop: &D, layouts: &Layouts) -> Result<(D::Window, Vec<Rect>), String> {
    let window = desktop.active_window().ok_or(NO_WINDOW)?;
    let monitor = desktop.monitor_of(window);
    Ok((window, place(layouts.for_monitor(monitor.bounds), monitor.work_area)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::desktop::testing::{FakeDesktop, LEFT, RIGHT, WINDOW};
    use crate::zones::layout::{Layout, columns, equal, rows};

    #[test]
    fn a_window_moves_to_that_zone_of_its_monitor() {
        let mut desktop = FakeDesktop { window_bounds: Rect::new(1200, 100, 300, 200), ..FakeDesktop::default() };
        assert_eq!(move_to(&mut desktop, &FakeDesktop::layouts(), 2), Ok(()));
        assert_eq!(desktop.moves, [(WINDOW, Rect::new(1500, 0, 500, 600))]);
    }

    #[test]
    fn a_window_moves_to_a_zone_of_its_monitors_own_layout() {
        let mut desktop = FakeDesktop { window_bounds: Rect::new(1200, 100, 300, 200), ..FakeDesktop::default() };
        let mut layouts = FakeDesktop::layouts();
        layouts.monitors.push((RIGHT, Layout { zones: rows(&equal(2)), gap: 0 }));
        move_to(&mut desktop, &layouts, 2).unwrap();
        assert_eq!(desktop.moves, [(WINDOW, Rect::new(1000, 300, 1000, 300))]);
    }

    #[test]
    fn a_zone_that_isnt_there_is_named() {
        for number in [0, 3] {
            let mut desktop = FakeDesktop::default();
            assert_eq!(
                move_to(&mut desktop, &FakeDesktop::layouts(), number),
                Err(format!("There's no zone {number}: there are 2, from 1 to 2."))
            );
            assert!(desktop.moves.is_empty());
        }
    }

    #[test]
    fn without_a_window_nothing_moves() {
        let mut desktop = FakeDesktop { active: None, ..FakeDesktop::default() };
        assert_eq!(move_to(&mut desktop, &FakeDesktop::layouts(), 1), Err(NO_WINDOW.into()));
        assert_eq!(move_to_pointer(&mut desktop), Err(NO_WINDOW.into()));
    }

    #[test]
    fn a_window_that_cant_be_moved_says_why() {
        let mut desktop = FakeDesktop { move_problem: Some("It runs as administrator.".into()), ..FakeDesktop::default() };
        assert_eq!(move_to(&mut desktop, &FakeDesktop::layouts(), 1), Err("It runs as administrator.".into()));
    }

    #[test]
    fn next_goes_from_zone_to_zone_around_the_end() {
        let mut desktop = FakeDesktop::default();
        let layouts = FakeDesktop::layouts();
        // From no zone to the first, then the second, then the first again.
        for expected in [Rect::new(0, 0, 500, 600), Rect::new(500, 0, 500, 600), Rect::new(0, 0, 500, 600)] {
            move_by(&mut desktop, &layouts, true).unwrap();
            assert_eq!(desktop.window_bounds, expected);
        }
    }

    #[test]
    fn back_from_no_zone_goes_to_the_last_and_from_the_first_to_the_last() {
        let mut desktop = FakeDesktop::default();
        let layouts = FakeDesktop::layouts();
        move_by(&mut desktop, &layouts, false).unwrap();
        assert_eq!(desktop.window_bounds, Rect::new(500, 0, 500, 600));
        move_by(&mut desktop, &layouts, false).unwrap();
        move_by(&mut desktop, &layouts, false).unwrap();
        assert_eq!(desktop.window_bounds, Rect::new(500, 0, 500, 600));
    }

    #[test]
    fn a_window_wider_than_the_zones_still_goes_through_every_zone() {
        // Four columns of 250 pixels, and a program that keeps its window at least 400 wide.
        let layouts = Layouts { default: Layout { zones: columns(&equal(4)), gap: 0 }, monitors: vec![] };
        for (forward, lefts) in [(true, [0, 250, 500, 750, 0]), (false, [750, 500, 250, 0, 750])] {
            let mut desktop = FakeDesktop { minimum_width: 400, ..FakeDesktop::default() };
            let visited: Vec<i32> = lefts
                .iter()
                .map(|_| {
                    move_by(&mut desktop, &layouts, forward).unwrap();
                    desktop.window_bounds.left
                })
                .collect();
            assert_eq!(visited, lefts);
        }
    }

    #[test]
    fn to_the_pointer_moves_a_window_to_the_pointers_monitor_at_the_same_place() {
        let mut desktop = FakeDesktop { pointer: (1500, 300), ..FakeDesktop::default() };
        assert_eq!(move_to_pointer(&mut desktop), Ok(()));
        assert_eq!(desktop.moves, [(WINDOW, Rect::new(1100, 100, 300, 200))]);
        assert!(desktop.changes.is_empty());
    }

    #[test]
    fn to_the_pointer_on_its_monitor_already_does_nothing() {
        let mut desktop = FakeDesktop::default();
        assert_eq!(move_to_pointer(&mut desktop), Ok(()));
        assert!(desktop.moves.is_empty());
        assert_eq!(desktop.monitor_of(WINDOW).bounds, LEFT);
    }

    #[test]
    fn to_the_pointer_a_maximized_window_is_maximized_there() {
        let mut desktop = FakeDesktop { pointer: (1500, 300), maximized: true, ..FakeDesktop::default() };
        move_to_pointer(&mut desktop).unwrap();
        assert_eq!(desktop.changes, ["restore", "maximize"]);
        assert_eq!(desktop.moves, [(WINDOW, Rect::new(1100, 100, 300, 200))]);
    }

    #[test]
    fn to_the_pointer_a_maximized_window_that_cant_move_stays_maximized() {
        let mut desktop = FakeDesktop {
            pointer: (1500, 300),
            maximized: true,
            move_problem: Some("Could not move the window.".into()),
            ..FakeDesktop::default()
        };
        assert_eq!(move_to_pointer(&mut desktop), Err("Could not move the window.".into()));
        assert!(desktop.maximized);
    }
}
