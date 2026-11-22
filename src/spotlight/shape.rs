//! Drawing the spotlight pixel by pixel, for the OSes where we fill a pixel buffer ourselves
//! (Windows and X11; macOS lets Core Animation draw instead).

// macOS doesn't use this file.
#![cfg_attr(target_os = "macos", allow(dead_code))]

use super::Point;

/// A rectangle in pixels: `left..right`, `top..bottom` (right/bottom excluded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Area {
    #[cfg_attr(not(windows), allow(dead_code))] // only Windows uses it
    pub fn new(left: i32, top: i32, width: i32, height: i32) -> Self {
        Self { left, top, right: left + width, bottom: top + height }
    }
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn union(&self, other: &Area) -> Area {
        Area {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
    /// The part of this area inside a `width` x `height` image.
    pub fn clamp_to(&self, width: i32, height: i32) -> Area {
        Area {
            left: self.left.clamp(0, width),
            top: self.top.clamp(0, height),
            right: self.right.clamp(0, width),
            bottom: self.bottom.clamp(0, height),
        }
    }
}

/// Fills `area` with one colour. `buffer` holds the pixels of `buffer_area`, row after row.
#[cfg_attr(not(windows), allow(dead_code))] // only Windows uses it
pub fn fill(buffer: &mut [u32], buffer_area: Area, area: Area, color: u32) {
    let stride = buffer_area.width();
    for y in area.top..area.bottom {
        let row = ((y - buffer_area.top) * stride - buffer_area.left) as isize;
        let (start, end) = ((row + area.left as isize) as usize, (row + area.right as isize) as usize);
        buffer[start..end].fill(color);
    }
}

/// The colours to draw with, in whatever pixel format the OS wants.
#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub dim: u32,
    pub ring: u32,
    pub hole: u32,
}

/// The see-through circle and the ring around it, in window pixels.
#[derive(Debug, Clone, Copy)]
pub struct Circle {
    center: Point,
    inner2: f64,
    outer2: f64,
    reach: i32,
}

impl Circle {
    /// A hole of `radius` around `center`, with a ring `ring` pixels wide on its edge.
    pub fn new(center: Point, radius: f64, ring: f64) -> Self {
        let inner = radius - ring / 2.0;
        let outer = radius + ring / 2.0;
        Self {
            center,
            inner2: inner * inner,
            outer2: outer * outer,
            reach: outer.ceil() as i32 + 1,
        }
    }

    /// The area the circle and its ring cover, within a `width` x `height` window.
    pub fn bounds(&self, width: i32, height: i32) -> Area {
        let (cx, cy) = self.center;
        Area {
            left: cx - self.reach,
            top: cy - self.reach,
            right: cx + self.reach + 1,
            bottom: cy + self.reach + 1,
        }
        .clamp_to(width, height)
    }

    /// Draws `area` (window pixels): the hole and ring where the circle is, dim elsewhere.
    /// `buffer` holds the pixels of `buffer_area`, one row of `buffer_area.width()` after another.
    /// No anti-aliasing on purpose: Windows' see-through key colour must match exactly.
    pub fn paint(&self, buffer: &mut [u32], buffer_area: Area, area: Area, colors: Colors) {
        let stride = buffer_area.width();
        for y in area.top..area.bottom {
            let dy = f64::from(y - self.center.1);
            let row = ((y - buffer_area.top) * stride - buffer_area.left) as isize;
            for x in area.left..area.right {
                let dx = f64::from(x - self.center.0);
                let distance2 = dx * dx + dy * dy;
                buffer[(row + x as isize) as usize] = if distance2 <= self.inner2 {
                    colors.hole
                } else if distance2 <= self.outer2 {
                    colors.ring
                } else {
                    colors.dim
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLORS: Colors = Colors { dim: 1, ring: 2, hole: 3 };

    #[test]
    fn paints_hole_ring_and_dim() {
        let circle = Circle::new((10, 10), 5.0, 2.0); // hole up to 4, ring 4..6
        let area = Area::new(0, 0, 21, 21);
        let mut buffer = vec![0; 21 * 21];
        circle.paint(&mut buffer, area, area, COLORS);
        let at = |x: usize, y: usize| buffer[y * 21 + x];
        assert_eq!(at(10, 10), 3); // centre: hole
        assert_eq!(at(15, 10), 2); // 5 px right: ring
        assert_eq!(at(20, 10), 1); // 10 px right: dim
    }

    #[test]
    fn paints_into_a_buffer_holding_only_part_of_the_window() {
        let circle = Circle::new((100, 100), 5.0, 2.0);
        let area = Area::new(90, 90, 21, 21); // the buffer covers just this part
        let mut buffer = vec![0; 21 * 21];
        circle.paint(&mut buffer, area, area, COLORS);
        assert_eq!(buffer[10 * 21 + 10], 3); // (100, 100) is the buffer's middle
        assert_eq!(buffer[0], 1);
    }

    #[test]
    fn bounds_stay_inside_the_window() {
        let circle = Circle::new((2, 2), 50.0, 4.0);
        assert_eq!(circle.bounds(800, 600), Area { left: 0, top: 0, right: 56, bottom: 56 });
    }
}
