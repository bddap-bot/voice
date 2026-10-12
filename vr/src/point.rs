use std::time::{Duration, Instant};

use crate::placement::Vec3;

/// How long the puppet keeps pointing after it stops speaking.
const QUIET: Duration = Duration::from_millis(1500);
/// How long it points when the reply that asked is never spoken.
const UNSPOKEN: Duration = Duration::from_secs(10);
const SPEAKING: f32 = 0.05;
/// How quickly the arm moves to and from pointing, per second.
const EASE: f32 = 6.0;

/// The puppet pointing at a named part of the shown object while it speaks the reply that named it.
#[derive(Default)]
pub struct Pointing {
    name: Option<String>,
    since: Option<Instant>,
    quiet: Option<Instant>,
    spoke: bool,
    weight: f32,
    target: Option<Vec3>,
}

impl Pointing {
    pub fn start(&mut self, name: String, now: Instant) {
        eprintln!("pointing at {name:?}");
        *self = Pointing { name: Some(name), since: Some(now), weight: self.weight, target: self.target, ..Pointing::default() };
    }

    pub fn stop(&mut self) {
        *self = Pointing::default();
    }

    /// The target and how fully to point at it this frame; `locate` finds a part of the shown object by name, and finds nothing while the object loads.
    pub fn step(&mut self, now: Instant, delta: f32, mouth: f32, locate: impl FnOnce(&str) -> Option<Vec3>) -> Option<(Vec3, f32)> {
        if let (Some(name), Some(since)) = (&self.name, self.since) {
            if mouth > SPEAKING {
                (self.spoke, self.quiet) = (true, None);
            } else if self.spoke {
                self.quiet.get_or_insert(now);
            }
            let over = match self.quiet {
                Some(quiet) => now - quiet >= QUIET,
                None => !self.spoke && now - since >= UNSPOKEN,
            };
            if over {
                self.name = None;
            } else if let Some(target) = locate(name) {
                self.target = Some(target);
            }
        }
        let wanted = self.name.is_some() as u8 as f32;
        self.weight += (wanted - self.weight) * (delta * EASE).min(1.0);
        Some((self.target?, self.weight)).filter(|&(_, weight)| weight > 1e-3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_points_while_the_reply_is_spoken_and_lowers_the_arm_after_it_falls_quiet() {
        let start = Instant::now();
        let mut pointing = Pointing::default();
        let frame = Duration::from_millis(11);
        let part = [0.3, 1.2, -0.4];
        pointing.start("gear".into(), start);
        let mut now = start;
        let mut weights = Vec::new();
        for step in 0..450 {
            let mouth = if (20..200).contains(&step) { 0.4 } else { 0.0 };
            weights.push(pointing.step(now, 0.011, mouth, |name| (name == "gear").then_some(part)).map_or(0.0, |(target, weight)| {
                assert_eq!(target, part);
                weight
            }));
            now += frame;
        }
        assert!(weights[100] > 0.99, "pointing while speaking: {}", weights[100]);
        assert!(weights[300] > 0.99, "still pointing within {QUIET:?} of the last word: {}", weights[300]);
        assert!(weights[449] < 0.01, "the arm is down {QUIET:?} after the last word: {}", weights[449]);
    }

    #[test]
    fn it_waits_for_a_part_still_loading_and_stops_after_an_unspoken_reply() {
        let start = Instant::now();
        let mut pointing = Pointing::default();
        pointing.start("gear".into(), start);
        assert_eq!(pointing.step(start, 0.011, 0.5, |_| None), None, "nothing to point at while the object loads");
        assert!(pointing.step(start, 0.5, 0.5, |_| Some([0.0; 3])).is_some(), "it points once the part appears");
        pointing.start("gear".into(), start);
        assert!(pointing.step(start + Duration::from_secs(1), 1.0, 0.0, |_| Some([0.0; 3])).is_some());
        assert!(pointing.step(start + UNSPOKEN, 1.0, 0.0, |_| Some([0.0; 3])).is_none(), "an unspoken reply's pointing ends after {UNSPOKEN:?}");
    }
}
