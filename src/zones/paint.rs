//! What the overlay draws, as plain rectangles: each zone filled, with a white border and its
//! number, and the highlighted one in blue, drawn last so all of it shows where zones overlap.
//!
//! Numbers are seven-segment digits made of rectangles, like a digital clock's, so no fonts
//! are needed on any OS. Windows and X11 fill a pixel buffer with [`rasterize`]; macOS makes a
//! Core Animation layer of each rectangle.

use super::drag::ShownZone;
use super::geometry::drawing_order;
use super::layout::Rect;

/// What a rectangle is, which gives its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Zone,
    Highlight,
    Border,
    Number,
}

/// The colours as opaque RGB; the overlay as a whole is 60% opaque.
pub const fn rgb(paint: Paint) -> [u8; 3] {
    match paint {
        Paint::Zone => [32, 32, 32],
        Paint::Highlight => [0, 120, 215],
        Paint::Border | Paint::Number => [255, 255, 255],
    }
}

pub const OPACITY: f64 = 0.6;

/// The rectangles to draw, in order, in the coordinates of `zones` minus `origin` (the
/// overlay's top left corner). `scale` makes the border wider on a high-DPI screen.
pub fn shapes(zones: &[ShownZone], highlighted: Option<usize>, origin: (i32, i32), scale: f64) -> Vec<(Rect, Paint)> {
    let border = (2.0 * scale).round().max(1.0) as i32;
    let mut shapes = Vec::new();
    for index in drawing_order(zones.len(), highlighted) {
        let zone = zones[index].bounds;
        let zone = Rect { left: zone.left - origin.0, top: zone.top - origin.1, ..zone };
        let fill = if highlighted == Some(index) { Paint::Highlight } else { Paint::Zone };
        shapes.push((zone, fill));
        let (left, top, right, bottom) = (zone.left, zone.top, zone.right(), zone.bottom());
        for side in [
            Rect::from_edges(left, top, right, top + border),
            Rect::from_edges(left, bottom - border, right, bottom),
            Rect::from_edges(left, top, left + border, bottom),
            Rect::from_edges(right - border, top, right, bottom),
        ] {
            shapes.push((side, Paint::Border));
        }
        let height = (zone.width.min(zone.height) / 4).clamp(8, 96);
        shapes.extend(number(zones[index].number, zone, height).into_iter().map(|segment| (segment, Paint::Number)));
    }
    shapes
}

/// Which of the seven segments each digit lights: top, top right, bottom right, bottom,
/// bottom left, top left, middle.
const SEGMENTS: [[bool; 7]; 10] = {
    const fn digit(lit: &[u8]) -> [bool; 7] {
        let mut segments = [false; 7];
        let mut index = 0;
        while index < lit.len() {
            segments[lit[index] as usize] = true;
            index += 1;
        }
        segments
    }
    [
        digit(&[0, 1, 2, 3, 4, 5]),
        digit(&[1, 2]),
        digit(&[0, 1, 6, 4, 3]),
        digit(&[0, 1, 6, 2, 3]),
        digit(&[5, 6, 1, 2]),
        digit(&[0, 5, 6, 2, 3]),
        digit(&[0, 5, 4, 3, 2, 6]),
        digit(&[0, 1, 2]),
        digit(&[0, 1, 2, 3, 4, 5, 6]),
        digit(&[0, 1, 2, 3, 5, 6]),
    ]
};

/// The segments of `number`, `height` tall, centred in `area`.
pub fn number(number: usize, area: Rect, height: i32) -> Vec<Rect> {
    let digits: Vec<usize> = number.to_string().bytes().map(|byte| usize::from(byte - b'0')).collect();
    let width = height * 11 / 20;
    let thick = (height / 7).max(2);
    let space = thick * 2;
    // A 1 lights only its right side: drawn as narrow as that, so numbers look centred.
    let width_of = |digit: usize| if digit == 1 { thick } else { width };
    let total = digits.iter().map(|&digit| width_of(digit)).sum::<i32>() + space * (digits.len() as i32 - 1);
    let (mut left, top) = (area.left + (area.width - total) / 2, area.top + (area.height - height) / 2);

    let mut segments = Vec::new();
    for digit in digits {
        let (right, bottom, middle) = (left + width_of(digit), top + height, top + height / 2);
        let all = [
            Rect::from_edges(left, top, right, top + thick),
            Rect::from_edges(right - thick, top, right, middle + thick / 2),
            Rect::from_edges(right - thick, middle - thick / 2, right, bottom),
            Rect::from_edges(left, bottom - thick, right, bottom),
            Rect::from_edges(left, middle - thick / 2, left + thick, bottom),
            Rect::from_edges(left, top, left + thick, middle + thick / 2),
            Rect::from_edges(left, middle - thick / 2, right, middle - thick / 2 + thick),
        ];
        segments.extend(all.into_iter().zip(SEGMENTS[digit]).filter(|(_, lit)| *lit).map(|(segment, _)| segment));
        left = right + space;
    }
    segments
}

/// Fills a `width` x `height` pixel buffer, row by row, with the shapes over `background`.
/// `color` turns each kind of shape into a pixel value.
// macOS draws a layer per shape instead.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn rasterize(shapes: &[(Rect, Paint)], width: i32, height: i32, background: u32, color: impl Fn(Paint) -> u32) -> Vec<u32> {
    let mut pixels = vec![background; (width.max(0) * height.max(0)) as usize];
    for &(shape, paint) in shapes {
        let value = color(paint);
        let (left, right) = (shape.left.clamp(0, width), shape.right().clamp(0, width));
        for y in shape.top.clamp(0, height)..shape.bottom().clamp(0, height) {
            let row = (y * width) as usize;
            pixels[row + left as usize..row + right as usize].fill(value);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(left: i32, number: usize) -> ShownZone {
        ShownZone { bounds: Rect::new(left, 0, 100, 100), number }
    }

    #[test]
    fn the_highlighted_zone_is_drawn_last_in_blue() {
        let shapes = shapes(&[zone(0, 1), zone(50, 2)], Some(0), (0, 0), 1.0);
        let fills: Vec<(Rect, Paint)> =
            shapes.iter().copied().filter(|(_, paint)| matches!(paint, Paint::Zone | Paint::Highlight)).collect();
        assert_eq!(fills, [(Rect::new(50, 0, 100, 100), Paint::Zone), (Rect::new(0, 0, 100, 100), Paint::Highlight)]);
        // Its number is the last thing drawn: 1 is the two segments on the right.
        assert_eq!(shapes.last().unwrap().1, Paint::Number);
    }

    #[test]
    fn shapes_are_moved_to_the_overlays_corner() {
        let shapes = shapes(&[zone(100, 1)], None, (100, 0), 1.0);
        assert_eq!(shapes[0], (Rect::new(0, 0, 100, 100), Paint::Zone));
    }

    #[test]
    fn digits_light_their_segments_and_a_number_is_centred() {
        let area = Rect::new(0, 0, 200, 100);
        assert_eq!(number(8, area, 40).len(), 7);
        assert_eq!(number(1, area, 40).len(), 2);
        let ten = number(10, area, 40);
        assert_eq!(ten.len(), 2 + 6);
        let (left, right) = (ten.iter().map(|r| r.left).min().unwrap(), ten.iter().map(|r| r.right()).max().unwrap());
        assert!((left - (200 - right)).abs() <= 1, "{left}..{right}");
    }

    #[test]
    fn rasterizing_fills_the_shapes_over_the_background_cut_to_the_buffer() {
        let pixels = rasterize(&[(Rect::new(-1, 1, 3, 1), Paint::Zone), (Rect::new(1, 1, 9, 9), Paint::Border)], 3, 3, 0, |paint| {
            if paint == Paint::Zone { 1 } else { 2 }
        });
        assert_eq!(pixels, [0, 0, 0, 1, 2, 2, 0, 2, 2]);
    }
}
