//! What a layout is: zones in percent of a monitor's work area, and the gap between them.
//! Plain data, the same on every OS.

/// A rectangle in screen pixels. `left` and `top` can be below 0, for a monitor left of or
/// above the main one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, width: i32, height: i32) -> Self {
        Self { left, top, width, height }
    }

    pub fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self::new(left, top, right - left, bottom - top)
    }

    pub fn right(&self) -> i32 {
        self.left + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.top + self.height
    }

    /// Larger by `by` on every side; smaller when it's below 0.
    pub fn inflate(&self, by: i32) -> Self {
        Self::new(self.left - by, self.top - by, self.width + 2 * by, self.height + 2 * by)
    }

    /// Whether the point is inside: the left and top edges are, the right and bottom aren't.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        (self.left..self.right()).contains(&x) && (self.top..self.bottom()).contains(&y)
    }
}

/// Where a zone is, in percent of a monitor's work area: `(0, 0, 100, 100)` is all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl Zone {
    pub const fn new(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self { left, top, width, height }
    }

    pub fn right(&self) -> f64 {
        self.left + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.top + self.height
    }
}

/// A monitor, in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monitor {
    /// All of it, which tells it apart from the other monitors across restarts.
    pub bounds: Rect,
    /// It without the taskbar (or menu bar, or panels), where the zones go.
    pub work_area: Rect,
}

/// The zones of a monitor, numbered from 1 in this order, and the space between them.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub zones: Vec<Zone>,
    /// Pixels between zones, and between them and the edges of the work area.
    pub gap: i32,
}

pub const DEFAULT_GAP: i32 = 8;

impl Default for Layout {
    /// Three columns as wide as each other.
    fn default() -> Self {
        Self { zones: columns(&equal(3)), gap: DEFAULT_GAP }
    }
}

/// Every layout the user set: the default one, and the own layouts of some monitors, which
/// may not all be connected.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layouts {
    /// For every monitor without its own.
    pub default: Layout,
    /// The monitors with their own layout, by their bounds.
    pub monitors: Vec<(Rect, Layout)>,
}

impl Layouts {
    /// The monitor's own layout; the default one when it has none.
    pub fn for_monitor(&self, bounds: Rect) -> &Layout {
        self.monitors.iter().find(|(monitor, _)| *monitor == bounds).map_or(&self.default, |(_, layout)| layout)
    }
}

/// A layout the zone editor can start from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Columns2,
    Columns3,
    Columns4,
    /// Columns of 25, 50 and 25 percent.
    WideCenter,
    Rows2,
    Rows3,
    Grid2x2,
    Grid3x2,
    Grid3x3,
}

impl Template {
    pub const ALL: [Template; 9] = [
        Template::Columns2,
        Template::Columns3,
        Template::Columns4,
        Template::WideCenter,
        Template::Rows2,
        Template::Rows3,
        Template::Grid2x2,
        Template::Grid3x2,
        Template::Grid3x3,
    ];

    pub fn zones(self) -> Vec<Zone> {
        match self {
            Template::Columns2 => columns(&equal(2)),
            Template::Columns3 => columns(&equal(3)),
            Template::Columns4 => columns(&equal(4)),
            Template::WideCenter => columns(&[25.0, 50.0, 25.0]),
            Template::Rows2 => rows(&equal(2)),
            Template::Rows3 => rows(&equal(3)),
            Template::Grid2x2 => grid(&equal(2), &equal(2)),
            Template::Grid3x2 => grid(&equal(3), &equal(2)),
            Template::Grid3x3 => grid(&equal(3), &equal(3)),
        }
    }

    /// Its name in the editor's menu.
    pub fn label(self) -> &'static str {
        match self {
            Template::Columns2 => "2 columns",
            Template::Columns3 => "3 columns",
            Template::Columns4 => "4 columns",
            Template::WideCenter => "Wide center (25 / 50 / 25)",
            Template::Rows2 => "2 rows",
            Template::Rows3 => "3 rows",
            Template::Grid2x2 => "Grid 2 x 2",
            Template::Grid3x2 => "Grid 3 x 2",
            Template::Grid3x3 => "Grid 3 x 3",
        }
    }
}

/// `widths`: each column's width in percent, left to right, adding up to 100.
pub fn columns(widths: &[f64]) -> Vec<Zone> {
    grid(widths, &[100.0])
}

/// `heights`: each row's height in percent, top to bottom, adding up to 100.
pub fn rows(heights: &[f64]) -> Vec<Zone> {
    grid(&[100.0], heights)
}

/// A zone per cell, row by row, each left to right.
pub fn grid(widths: &[f64], heights: &[f64]) -> Vec<Zone> {
    let mut zones = Vec::with_capacity(widths.len() * heights.len());
    let mut top = 0.0;
    for &height in heights {
        let mut left = 0.0;
        for &width in widths {
            zones.push(Zone::new(left, top, width, height));
            left += width;
        }
        top += height;
    }
    zones
}

/// As many percents of the same size as `count`, adding up to 100.
pub fn equal(count: usize) -> Vec<f64> {
    vec![100.0 / count as f64; count]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_goes_row_by_row() {
        assert_eq!(
            grid(&equal(2), &equal(2)),
            [Zone::new(0.0, 0.0, 50.0, 50.0), Zone::new(50.0, 0.0, 50.0, 50.0), Zone::new(0.0, 50.0, 50.0, 50.0), Zone::new(50.0, 50.0, 50.0, 50.0)]
        );
    }

    #[test]
    fn every_template_fills_the_monitor_once() {
        for template in Template::ALL {
            let area: f64 = template.zones().iter().map(|zone| zone.width * zone.height).sum();
            assert!((area - 100.0 * 100.0).abs() < 1e-6, "{}", template.label());
            assert!(template.zones().iter().all(|zone| zone.right() <= 100.0 + 1e-9 && zone.bottom() <= 100.0 + 1e-9));
        }
    }

    #[test]
    fn a_monitor_without_its_own_layout_uses_the_default() {
        let wide = Rect::new(1920, 0, 2560, 1440);
        let own = Layout { zones: rows(&equal(2)), gap: 0 };
        let layouts = Layouts { default: Layout::default(), monitors: vec![(wide, own.clone())] };
        assert_eq!(layouts.for_monitor(wide), &own);
        assert_eq!(layouts.for_monitor(Rect::new(0, 0, 1920, 1080)), &Layout::default());
    }
}
