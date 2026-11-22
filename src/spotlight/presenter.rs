//! Where the spotlight is and how big, over time.

use std::time::{Duration, Instant};

use super::Point;

/// The circle's radius when it shows, in pixels at 100% display scale.
pub const START_RADIUS: f64 = 600.0;
/// Its radius once it has shrunk.
pub const RADIUS: f64 = 100.0;
/// How long it takes to shrink.
pub const SHRINK_TIME: Duration = Duration::from_millis(400);
/// It hides once the pointer has stayed still this long.
pub const STILL_TIME: Duration = Duration::from_secs(3);

/// Follows one showing of the spotlight.
pub struct Presenter {
    shown_at: Instant,
    moved_at: Instant,
    cursor: Point,
}

impl Presenter {
    pub fn new(now: Instant, cursor: Point) -> Self {
        Self {
            shown_at: now,
            moved_at: now,
            cursor,
        }
    }

    /// The radius at `now`, at 100% scale.
    pub fn radius(&self, now: Instant) -> i32 {
        radius_at(now.saturating_duration_since(self.shown_at))
    }

    /// Where to draw the circle (centre, radius) at `now`, or `None` once it should hide
    /// because the pointer hasn't moved for `STILL_TIME`.
    pub fn tick(&mut self, now: Instant, cursor: Point) -> Option<(Point, i32)> {
        if cursor != self.cursor {
            self.cursor = cursor;
            self.moved_at = now;
        } else if now.saturating_duration_since(self.moved_at) >= STILL_TIME {
            return None;
        }
        Some((self.cursor, self.radius(now)))
    }
}

/// The radius this long after showing: fast at first, slowing down onto `RADIUS`.
pub fn radius_at(elapsed: Duration) -> i32 {
    let progress = (elapsed.as_secs_f64() / SHRINK_TIME.as_secs_f64()).clamp(0.0, 1.0);
    let left = 1.0 - progress;
    (RADIUS + (START_RADIUS - RADIUS) * left * left).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn shrinks_from_start_to_final_radius() {
        assert_eq!(radius_at(MS(0)), 600);
        assert_eq!(radius_at(MS(200)), 225); // half way: 100 + 500 * 0.5²
        assert_eq!(radius_at(MS(400)), 100);
        assert_eq!(radius_at(MS(5000)), 100);
    }

    #[test]
    fn follows_the_pointer() {
        let start = Instant::now();
        let mut presenter = Presenter::new(start, (10, 10));
        assert_eq!(presenter.tick(start + MS(500), (50, 60)), Some(((50, 60), 100)));
    }

    #[test]
    fn hides_once_the_pointer_stays_still() {
        let start = Instant::now();
        let mut presenter = Presenter::new(start, (10, 10));
        assert!(presenter.tick(start + MS(2900), (10, 10)).is_some());
        assert!(presenter.tick(start + MS(3000), (10, 10)).is_none());
    }

    #[test]
    fn moving_restarts_the_still_timer() {
        let start = Instant::now();
        let mut presenter = Presenter::new(start, (10, 10));
        presenter.tick(start + MS(2000), (20, 20));
        assert!(presenter.tick(start + MS(4000), (20, 20)).is_some());
        assert!(presenter.tick(start + MS(5000), (20, 20)).is_none());
    }
}
