//! Where the zones of a layout are on a monitor, and which one a point or a window is in.

use super::layout::{Layout, Rect};

/// How far a window can be from a zone, in pixels, and still be in it: programs round their
/// sizes.
const SAME_ZONE_TOLERANCE: i32 = 4;

/// Each zone of the layout in screen pixels, in the layout's order, with the gap around it.
/// `work_area` is the monitor's, in screen pixels.
pub fn place(layout: &Layout, work_area: Rect) -> Vec<Rect> {
    // Half the gap inside the work area, and half around each zone: a whole gap between zones
    // and at the edges.
    let half = layout.gap / 2;
    let area = work_area.inflate(-half);
    layout
        .zones
        .iter()
        .map(|zone| {
            let left = area.left + percent(area.width, zone.left);
            let top = area.top + percent(area.height, zone.top);
            let right = area.left + percent(area.width, zone.right());
            let bottom = area.top + percent(area.height, zone.bottom());
            let bounds = Rect::from_edges(left + half, top + half, right - half, bottom - half);
            Rect { width: bounds.width.max(1), height: bounds.height.max(1), ..bounds }
        })
        .collect()
}

/// The index of the zone the point is in; `None` when it's in none.
///
/// A point in the gap between zones is in the zones on either side, so no zone goes off while
/// the mouse crosses it. When it's in several, the one whose center is nearest, so each of
/// zones that overlap is the one chosen around its own middle; of those as near, the smallest,
/// so a zone inside another with the same center can still be chosen.
pub fn zone_at(zones: &[Rect], x: i32, y: i32, gap: i32) -> Option<usize> {
    let reach = gap / 2 + 1;
    let (indexes, found): (Vec<usize>, Vec<[f64; 4]>) = zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| zone.inflate(reach).contains(x, y))
        .map(|(index, zone)| {
            (index, [zone.left, zone.top, zone.width, zone.height].map(f64::from))
        })
        .unzip();
    nearest(&found, f64::from(x), f64::from(y)).map(|index| indexes[index])
}

/// Of zones that a point is in, the one whose center is nearest the point; of those as near,
/// the smallest. Each zone is `[left, top, width, height]`, in any unit, the point's too.
/// `None` for no zones.
pub fn nearest(zones: &[[f64; 4]], x: f64, y: f64) -> Option<usize> {
    const SAME: f64 = 1e-9;
    let mut nearest = None;
    let (mut best_distance, mut best_area) = (f64::MAX, f64::MAX);
    for (index, &[left, top, width, height]) in zones.iter().enumerate() {
        let (dx, dy) = (left + width / 2.0 - x, top + height / 2.0 - y);
        let distance = dx * dx + dy * dy;
        let area = width * height;
        if distance < best_distance - SAME || ((distance - best_distance).abs() <= SAME && area < best_area) {
            nearest = Some(index);
            best_distance = distance;
            best_area = area;
        }
    }
    nearest
}

/// The index of the zone the window was put in: the one whose top left corner it's at, to a
/// few pixels, since a window put in a zone always starts there; of those, the one nearest its
/// size. Not its size alone: a program can keep its window larger than a small zone, or size
/// it in steps, as a terminal does by rows. `None` when it's at no zone's corner.
pub fn zone_of(zones: &[Rect], window: Rect) -> Option<usize> {
    zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| {
            (zone.left - window.left).abs() <= SAME_ZONE_TOLERANCE
                && (zone.top - window.top).abs() <= SAME_ZONE_TOLERANCE
        })
        // `min_by_key` keeps the first of those as near, as the C# version did.
        .min_by_key(|(_, zone)| (zone.width - window.width).abs() + (zone.height - window.height).abs())
        .map(|(index, _)| index)
}

/// The order to draw `count` zones in: as numbered, but `on_top` (such as the one
/// highlighted) last, so where zones overlap all of it shows.
pub fn drawing_order(count: usize, on_top: Option<usize>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..count).collect();
    if let Some(top) = on_top.filter(|&top| top < count) {
        order.remove(top);
        order.push(top);
    }
    order
}

/// Where a window goes on another monitor: as large, but no larger than its work area, and
/// at the same place in it, so a window at the right edge stays at the right edge, and one in
/// the middle in the middle. `window` is as the user sees it; `from` and `to` are the work
/// areas of the monitor it's on and the one it goes to.
pub fn move_to_area(window: Rect, from: Rect, to: Rect) -> Rect {
    let width = window.width.min(to.width);
    let height = window.height.min(to.height);
    let left = to.left + along(window.left - from.left, from.width - window.width, to.width - width);
    let top = to.top + along(window.top - from.top, from.height - window.height, to.height - height);
    Rect::new(left, top, width, height)
}

/// The same part of the room there is on the other monitor; the middle when there was none.
fn along(offset: i32, room: i32, new_room: i32) -> i32 {
    let part = if room <= 0 { 0.5 } else { (f64::from(offset) / f64::from(room)).clamp(0.0, 1.0) };
    (part * f64::from(new_room)).round_ties_even() as i32
}

/// `percent` of `length`, rounded half to even as the C# version did, so the zones land on
/// the same pixels.
fn percent(length: i32, percent: f64) -> i32 {
    (f64::from(length) * percent / 100.0).round_ties_even() as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::layout::{columns, equal, grid};

    const WORK_AREA: Rect = Rect::new(0, 0, 1000, 600);

    fn layout(zones: Vec<crate::zones::layout::Zone>, gap: i32) -> Layout {
        Layout { zones, gap }
    }

    #[test]
    fn without_a_gap_the_zones_share_the_work_area() {
        assert_eq!(
            place(&layout(columns(&[25.0, 50.0, 25.0]), 0), WORK_AREA),
            [Rect::new(0, 0, 250, 600), Rect::new(250, 0, 500, 600), Rect::new(750, 0, 250, 600)]
        );
    }

    #[test]
    fn the_gap_is_between_zones_and_at_the_edges() {
        assert_eq!(
            place(&layout(columns(&equal(2)), 10), Rect::new(100, 50, 1010, 610)),
            [Rect::new(110, 60, 490, 590), Rect::new(610, 60, 490, 590)]
        );
    }

    #[test]
    fn a_grid_is_placed_row_by_row() {
        assert_eq!(
            place(&layout(grid(&equal(2), &equal(2)), 0), WORK_AREA),
            [Rect::new(0, 0, 500, 300), Rect::new(500, 0, 500, 300), Rect::new(0, 300, 500, 300), Rect::new(500, 300, 500, 300)]
        );
    }

    #[test]
    fn thirds_of_an_odd_width_leave_no_pixel_out() {
        let zones = place(&layout(columns(&equal(3)), 0), Rect::new(0, 0, 1001, 600));
        assert_eq!(zones[0].left, 0);
        assert_eq!(zones[0].right(), zones[1].left);
        assert_eq!(zones[1].right(), zones[2].left);
        assert_eq!(zones[2].right(), 1001);
    }

    #[test]
    fn the_zone_under_a_point_counts_the_gap() {
        // A gap of 10, from 495 to 505.
        let zones = [Rect::from_edges(5, 5, 495, 595), Rect::from_edges(505, 5, 995, 595)];
        for (x, y, expected) in [(100, 100, Some(0)), (700, 500, Some(1)), (504, 300, Some(1)), (496, 300, Some(0)), (1200, 300, None)] {
            assert_eq!(zone_at(&zones, x, y, 10), expected, "at ({x}, {y})");
        }
    }

    #[test]
    fn in_zones_that_overlap_the_point_is_in_the_one_whose_center_is_nearest() {
        // Four zones that overlap, as a user saved them, on a monitor of 1000 x 1000.
        let zones = [Rect::new(0, 0, 635, 722), Rect::new(366, 0, 634, 722), Rect::new(0, 500, 635, 500), Rect::new(366, 363, 634, 637)];
        for (x, y, expected) in [(800, 200, 1), (800, 500, 1), (800, 600, 3), (300, 200, 0), (300, 700, 2)] {
            assert_eq!(zone_at(&zones, x, y, 0), Some(expected), "at ({x}, {y})");
        }
    }

    #[test]
    fn in_a_zone_inside_another_with_the_same_center_the_point_is_in_the_smaller() {
        let zones = [Rect::new(0, 0, 1000, 600), Rect::new(400, 200, 200, 200)];
        assert_eq!(zone_at(&zones, 500, 300, 0), Some(1));
    }

    #[test]
    fn the_zone_on_top_is_drawn_last() {
        assert_eq!(drawing_order(3, None), [0, 1, 2]);
        assert_eq!(drawing_order(3, Some(0)), [1, 2, 0]);
        assert_eq!(drawing_order(3, Some(1)), [0, 2, 1]);
        assert_eq!(drawing_order(3, Some(5)), [0, 1, 2]);
    }

    #[test]
    fn a_window_is_in_the_zone_it_fills_to_a_few_pixels() {
        let zones = [Rect::new(0, 0, 500, 600), Rect::new(500, 0, 500, 600)];
        assert_eq!(zone_of(&zones, Rect::new(0, 0, 500, 600)), Some(0));
        assert_eq!(zone_of(&zones, Rect::new(502, -3, 497, 604)), Some(1));
        assert_eq!(zone_of(&zones, Rect::new(100, 100, 300, 300)), None);
    }

    #[test]
    fn a_window_not_the_zones_size_is_in_the_zone_whose_corner_it_is_at() {
        // Zones that overlap; the third is too narrow for some programs, which keep their
        // windows wider, and a terminal sizes its window in rows.
        let zones = [Rect::new(0, 0, 938, 745), Rect::new(703, 0, 1217, 745), Rect::new(0, 516, 703, 516), Rect::new(703, 375, 1217, 657)];
        assert_eq!(zone_of(&zones, Rect::new(0, 516, 900, 516)), Some(2));
        assert_eq!(zone_of(&zones, Rect::new(0, 516, 690, 509)), Some(2));
        assert_eq!(zone_of(&zones, Rect::new(0, 0, 940, 745)), Some(0));
    }

    #[test]
    fn of_zones_with_the_same_corner_a_window_is_in_the_one_nearest_its_size() {
        let zones = [Rect::new(0, 0, 1000, 600), Rect::new(0, 0, 500, 300)];
        assert_eq!(zone_of(&zones, Rect::new(0, 0, 520, 310)), Some(1));
        assert_eq!(zone_of(&zones, Rect::new(0, 0, 990, 600)), Some(0));
    }

    #[test]
    fn moved_to_another_monitor_a_window_keeps_its_size_and_place() {
        assert_eq!(
            move_to_area(Rect::new(100, 100, 300, 200), WORK_AREA, Rect::new(1000, 0, 1000, 600)),
            Rect::new(1100, 100, 300, 200)
        );
    }

    #[test]
    fn on_a_larger_monitor_a_window_at_the_edge_stays_there_and_one_as_high_goes_in_the_middle() {
        assert_eq!(
            move_to_area(Rect::new(700, 0, 300, 600), WORK_AREA, Rect::new(1000, 0, 1600, 900)),
            Rect::new(2300, 150, 300, 600)
        );
    }

    #[test]
    fn on_a_smaller_monitor_a_window_is_no_larger_than_it() {
        assert_eq!(
            move_to_area(Rect::new(400, 300, 600, 300), WORK_AREA, Rect::new(2000, 0, 500, 400)),
            Rect::new(2000, 100, 500, 300)
        );
    }
}
