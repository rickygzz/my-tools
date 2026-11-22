//! Tells when a key is tapped twice quickly.

use std::time::{Duration, Instant};

/// Tells when a key is tapped twice quickly: pressed and let go of, twice, with no other key
/// or click in between. Holding the key isn't a tap, so its repeats never count, nor do
/// shortcuts that use it, such as Ctrl+C. It's told of every key and click, with the time
/// each happened.
#[derive(Debug, Default)]
pub struct DoubleTapDetector {
    /// When the key went down, while it's held.
    pressed_at: Option<Instant>,
    /// When the first tap let go of the key, while waiting for a second one.
    tapped_at: Option<Instant>,
}

impl DoubleTapDetector {
    /// The longest a tap holds the key down.
    pub const MAX_TAP_LENGTH: Duration = Duration::from_millis(300);
    /// The longest from letting go of the first tap to pressing the second.
    pub const MAX_GAP: Duration = Duration::from_millis(400);

    /// A key went down. `is_the_key` says whether it's the one being tapped.
    pub fn key_down(&mut self, is_the_key: bool, now: Instant) {
        if !is_the_key {
            self.interrupt();
            return;
        }
        // Already down: the OS repeats a held key.
        if self.pressed_at.is_some() {
            return;
        }
        if self
            .tapped_at
            .is_some_and(|tapped| now.duration_since(tapped) > Self::MAX_GAP)
        {
            self.tapped_at = None;
        }
        self.pressed_at = Some(now);
    }

    /// A key went up. Returns true when this ended the second tap.
    pub fn key_up(&mut self, is_the_key: bool, now: Instant) -> bool {
        if !is_the_key {
            return false;
        }
        let Some(pressed) = self.pressed_at.take() else {
            return false;
        };
        if now.duration_since(pressed) > Self::MAX_TAP_LENGTH {
            self.tapped_at = None;
            return false;
        }
        if self.tapped_at.take().is_some() {
            return true;
        }
        self.tapped_at = Some(now);
        false
    }

    /// Something else happened, such as a click: the taps so far no longer count.
    pub fn interrupt(&mut self) {
        self.pressed_at = None;
        self.tapped_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays key events at the given millisecond times, returning what each key-up returned.
    struct Keys {
        detector: DoubleTapDetector,
        start: Instant,
    }

    impl Keys {
        fn new() -> Self {
            Self {
                detector: DoubleTapDetector::default(),
                start: Instant::now(),
            }
        }
        fn at(&self, ms: u64) -> Instant {
            self.start + Duration::from_millis(ms)
        }
        fn down(&mut self, ms: u64) {
            let now = self.at(ms);
            self.detector.key_down(true, now);
        }
        fn up(&mut self, ms: u64) -> bool {
            let now = self.at(ms);
            self.detector.key_up(true, now)
        }
        fn other_key(&mut self, ms: u64) {
            let now = self.at(ms);
            self.detector.key_down(false, now);
        }
    }

    #[test]
    fn two_quick_taps_count() {
        let mut keys = Keys::new();
        keys.down(0);
        assert!(!keys.up(100));
        keys.down(200);
        assert!(keys.up(300));
    }

    #[test]
    fn one_tap_does_not_count() {
        let mut keys = Keys::new();
        keys.down(0);
        assert!(!keys.up(100));
    }

    #[test]
    fn holding_too_long_is_not_a_tap() {
        let mut keys = Keys::new();
        keys.down(0);
        assert!(!keys.up(500));
        keys.down(600);
        assert!(!keys.up(700));
    }

    #[test]
    fn too_long_a_gap_starts_over() {
        let mut keys = Keys::new();
        keys.down(0);
        keys.up(100);
        keys.down(600); // 500 ms after letting go
        assert!(!keys.up(700));
        // ...but that tap can be the first of a new pair.
        keys.down(800);
        assert!(keys.up(900));
    }

    #[test]
    fn another_key_in_between_cancels() {
        let mut keys = Keys::new();
        keys.down(0);
        keys.up(100);
        keys.other_key(150);
        keys.down(200);
        assert!(!keys.up(300));
    }

    #[test]
    fn shortcut_like_ctrl_c_is_not_a_tap() {
        let mut keys = Keys::new();
        keys.down(0);
        keys.other_key(50); // C while Ctrl is held
        assert!(!keys.up(100));
        keys.down(200);
        assert!(!keys.up(300));
    }

    #[test]
    fn key_repeat_while_held_is_ignored() {
        let mut keys = Keys::new();
        keys.down(0);
        keys.up(100);
        keys.down(200);
        keys.down(230); // auto-repeat
        keys.down(260);
        assert!(keys.up(300));
    }

    #[test]
    fn third_tap_starts_a_new_pair() {
        let mut keys = Keys::new();
        keys.down(0);
        keys.up(100);
        keys.down(200);
        assert!(keys.up(300));
        keys.down(400);
        assert!(!keys.up(500));
    }

    #[test]
    fn a_click_cancels() {
        let mut keys = Keys::new();
        keys.down(0);
        keys.up(100);
        keys.detector.interrupt();
        keys.down(200);
        assert!(!keys.up(300));
    }
}
