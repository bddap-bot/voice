use std::time::{Duration, Instant};

use crate::placement::Vec3;

pub const HOLD: Duration = Duration::from_secs(3);

/// The body's radius around the line through its feet, over its height: the catalog's figures reach a median 0.09 of their height from it at mid-body and their hanging arms 0.15, so a tip held beside the figure, as two hands brought together in play hold it, is outside.
const GIRTH: f32 = 0.12;

/// The hidden way to the appearance library: the right controller's tip held inside the puppet for `HOLD`, by position alone.
pub struct Reveal {
    height: f32,
    since: Option<Instant>,
    fired: bool,
}

impl Reveal {
    /// A puppet standing `height` tall on the origin.
    pub fn new(height: f32) -> Reveal {
        Reveal { height, since: None, fired: false }
    }

    /// The tip in the puppet's standing frame, or None while untracked; true on the frame the hold completes, then again only after the tip leaves.
    pub fn hold(&mut self, now: Instant, tip: Option<Vec3>) -> bool {
        let inside = tip.is_some_and(|[x, y, z]| (0.0..=self.height).contains(&y) && x.hypot(z) <= GIRTH * self.height);
        if !inside {
            self.reset();
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if self.fired || now.duration_since(since) < HOLD {
            return false;
        }
        self.fired = true;
        true
    }

    pub fn reset(&mut self) {
        self.since = None;
        self.fired = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEIGHT: f32 = 0.3;

    fn run(reveal: &mut Reveal, start: Instant, frames: impl IntoIterator<Item = (f32, Option<Vec3>)>) -> Vec<f32> {
        frames.into_iter().filter(|&(at, tip)| reveal.hold(start + Duration::from_secs_f32(at), tip)).map(|(at, _)| at).collect()
    }

    fn steady(from: f32, to: f32, tip: Option<Vec3>) -> impl Iterator<Item = (f32, Option<Vec3>)> {
        (0..).map(move |frame| from + frame as f32 / 90.0).take_while(move |&at| at < to).map(move |at| (at, tip))
    }

    #[test]
    fn three_seconds_inside_the_puppet_reveals_once() {
        let mut reveal = Reveal::new(HEIGHT);
        let fired = run(&mut reveal, Instant::now(), steady(0.0, 10.0, Some([0.0, HEIGHT / 2.0, 0.0])));
        assert_eq!(fired.len(), 1);
        assert!((HOLD.as_secs_f32()..HOLD.as_secs_f32() + 0.02).contains(&fired[0]), "{fired:?}");
    }

    #[test]
    fn leaving_restarts_the_hold_and_a_second_hold_reveals_again() {
        let mut reveal = Reveal::new(HEIGHT);
        let inside = Some([0.02, 0.05, -0.02]);
        let frames = steady(0.0, 2.5, inside).chain(steady(2.5, 2.6, Some([0.3, 0.0, 0.0]))).chain(steady(2.6, 5.0, inside)).chain(steady(5.0, 5.5, None)).chain(steady(5.5, 9.0, inside));
        let fired = run(&mut reveal, Instant::now(), frames);
        assert_eq!(fired.len(), 1, "{fired:?}");
        assert!(fired[0] >= 8.49, "only the last hold lasted three seconds: {fired:?}");
    }

    #[test]
    fn beside_above_below_or_through_the_puppet_reveals_nothing() {
        let mut reveal = Reveal::new(HEIGHT);
        let start = Instant::now();
        for tip in [[0.15, HEIGHT / 2.0, 0.0], [-0.15, HEIGHT / 2.0, 0.0], [0.0, HEIGHT + 0.02, 0.0], [0.0, -0.02, 0.0], [0.0, HEIGHT / 2.0, 0.15], [0.0, HEIGHT / 2.0, -0.15]] {
            assert!(run(&mut reveal, start, steady(0.0, 6.0, Some(tip))).is_empty(), "{tip:?}");
        }
        let sweep = (0..90).map(|frame| (frame as f32 / 90.0, Some([-0.2 + frame as f32 * 0.4 / 90.0, HEIGHT / 2.0, 0.0])));
        assert!(run(&mut reveal, start, sweep).is_empty(), "a hand passing through");
    }

    #[test]
    fn a_tip_beside_or_before_the_body_but_not_in_it_reveals_nothing() {
        let mut reveal = Reveal::new(HEIGHT);
        let start = Instant::now();
        for tip in [[0.07, HEIGHT / 2.0, 0.0], [-0.07, HEIGHT * 0.7, 0.0], [0.0, HEIGHT / 2.0, 0.07], [0.0, HEIGHT / 3.0, -0.07], [0.05, HEIGHT / 2.0, 0.05]] {
            assert!(run(&mut reveal, start, steady(0.0, 6.0, Some(tip))).is_empty(), "{tip:?}");
        }
    }

    #[test]
    fn a_reset_drops_a_hold_in_progress() {
        let mut reveal = Reveal::new(HEIGHT);
        let start = Instant::now();
        let inside = Some([0.0, HEIGHT / 2.0, 0.0]);
        assert!(run(&mut reveal, start, steady(0.0, 2.9, inside)).is_empty());
        reveal.reset();
        assert!(run(&mut reveal, start, steady(2.9, 5.8, inside)).is_empty());
        assert_eq!(run(&mut reveal, start, steady(5.8, 6.0, inside)).len(), 1);
    }
}
