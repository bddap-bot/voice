use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::placement::{length, sub, Pose, Vec3};

const POINTS: usize = 32;
const BAND: usize = 8;
const MOVING: f32 = 0.30;
const STILL: f64 = 0.35;
const SHORTEST: f64 = 0.4;
const LONGEST: f64 = 10.0;
const SMOOTHING: f64 = 0.1;
const NEAR: f32 = 2.0;
pub const AWAKE_FOR: Duration = Duration::from_secs(30);

pub type Point = [f32; 6];

#[derive(Deserialize)]
pub struct Templates {
    threshold: f32,
    peak_speed_min: f32,
    templates: Vec<Vec<Point>>,
}

impl Templates {
    pub fn parse(bytes: &[u8]) -> Result<Templates, String> {
        let templates: Templates = serde_json::from_slice(bytes).map_err(|error| format!("invalid gesture templates: {error}"))?;
        if templates.templates.is_empty() || templates.templates.iter().any(|template| template.len() != POINTS) {
            return Err(format!("gesture templates must be one or more traces of {POINTS} points"));
        }
        Ok(templates)
    }
}

/// Both hands relative to the head, rotated into the head's yaw frame: x right, y up, z back.
pub fn head_relative(head: &Pose, left: Vec3, right: Vec3) -> Point {
    let back = head.axis(2);
    let yaw = back[0].atan2(back[2]);
    let (sin, cos) = yaw.sin_cos();
    let turn = |hand: Vec3| {
        let d = sub(hand, head.t);
        [cos * d[0] - sin * d[2], d[1], sin * d[0] + cos * d[2]]
    };
    let [l, r] = [turn(left), turn(right)];
    [l[0], l[1], l[2], r[0], r[1], r[2]]
}

pub fn resample(trace: &[Point]) -> Vec<Point> {
    let last = trace.len() - 1;
    (0..POINTS)
        .map(|k| {
            let at = k as f32 * last as f32 / (POINTS - 1) as f32;
            let i = at.floor() as usize;
            let j = (i + 1).min(last);
            let w = at - i as f32;
            std::array::from_fn(|c| trace[i][c] * (1.0 - w) + trace[j][c] * w)
        })
        .collect()
}

fn distance(a: &Point, b: &Point) -> f32 {
    a.iter().zip(b).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt()
}

pub fn dtw(a: &[Point], b: &[Point]) -> f32 {
    let (n, m) = (a.len(), b.len());
    let mut cost = vec![f32::INFINITY; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    cost[0] = 0.0;
    for i in 1..=n {
        for j in i.saturating_sub(BAND).max(1)..=(i + BAND).min(m) {
            cost[at(i, j)] = distance(&a[i - 1], &b[j - 1]) + cost[at(i - 1, j)].min(cost[at(i, j - 1)]).min(cost[at(i - 1, j - 1)]);
        }
    }
    cost[at(n, m)] / (n + m) as f32
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    pub distance: f32,
    pub peak: f32,
    pub duration: f64,
    pub matched: bool,
    pub near: bool,
}

/// Until the wrist board's Dismiss lands, waking lasts `AWAKE_FOR`; a match while awake changes nothing.
pub fn wake(awake: Option<Instant>, matched: bool, now: Instant) -> Option<Instant> {
    awake.filter(|since| now.duration_since(*since) < AWAKE_FOR).or(matched.then_some(now))
}

enum Segment {
    Still,
    Moving { trace: Vec<Point>, end: usize, start: f64, peak: f32 },
    Overlong,
}

pub struct Recognizer {
    templates: Templates,
    recent: VecDeque<(f64, [Vec3; 2])>,
    segment: Segment,
    last_moving: f64,
}

impl Recognizer {
    pub fn new(templates: Templates) -> Recognizer {
        Recognizer { templates, recent: VecDeque::new(), segment: Segment::Still, last_moving: f64::NEG_INFINITY }
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        self.recent.len() + if let Segment::Moving { trace, .. } = &self.segment { trace.len() } else { 0 }
    }

    pub fn push(&mut self, t: f64, head: &Pose, left: Vec3, right: Vec3) -> Option<Verdict> {
        let mut verdict = None;
        match self.recent.back() {
            Some(&(last, _)) if t <= last => return None,
            Some(&(last, _)) if t - last > STILL => {
                self.recent.clear();
                verdict = self.judge();
            }
            _ => {}
        }
        self.recent.push_back((t, [left, right]));
        while self.recent.len() > 2 && self.recent[1].0 <= t - SMOOTHING {
            self.recent.pop_front();
        }
        let path: f32 = self.recent.iter().zip(self.recent.iter().skip(1)).map(|((_, a), (_, b))| length(sub(b[0], a[0])).max(length(sub(b[1], a[1])))).sum();
        let speed = path / (t - self.recent[0].0).max(1e-3) as f32;
        let moving = speed > MOVING;
        if moving {
            self.last_moving = t;
        }
        let settled = t - self.last_moving > STILL;
        let point = head_relative(head, left, right);
        let finished = match &mut self.segment {
            Segment::Still if moving => {
                self.segment = Segment::Moving { trace: vec![point], end: 1, start: t, peak: speed };
                false
            }
            Segment::Overlong if settled => {
                self.segment = Segment::Still;
                false
            }
            Segment::Moving { trace, end, start, peak } => {
                trace.push(point);
                if moving {
                    *end = trace.len();
                    *peak = peak.max(speed);
                    if t - *start > LONGEST {
                        self.segment = Segment::Overlong;
                    }
                }
                settled
            }
            _ => false,
        };
        if finished {
            verdict = self.judge();
        }
        verdict
    }

    fn judge(&mut self) -> Option<Verdict> {
        let Segment::Moving { trace, end, start, peak } = std::mem::replace(&mut self.segment, Segment::Still) else {
            return None;
        };
        let duration = self.last_moving - start;
        if duration < SHORTEST {
            return None;
        }
        let trace = resample(&trace[..end]);
        let distance = self.templates.templates.iter().map(|template| dtw(&trace, template)).fold(f32::INFINITY, f32::min);
        let Templates { threshold, peak_speed_min, .. } = self.templates;
        let matched = distance <= threshold && peak >= peak_speed_min;
        Some(Verdict { distance, peak, duration, matched, near: !matched && distance <= NEAR * threshold })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const IDENTITY: Pose = Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0, 1.6, 0.0] };

    fn yawed(angle: f32, t: Vec3) -> Pose {
        let (sin, cos) = angle.sin_cos();
        Pose { r: [[cos, 0.0, sin], [0.0, 1.0, 0.0], [-sin, 0.0, cos]], t }
    }

    /// Both hands sweep one synchronized loop in the sagittal plane, eased in and out, in the head's frame.
    fn sweep(phase: f32, radius: f32) -> [Vec3; 2] {
        let angle = TAU * phase;
        let (y, z) = (-0.3 + radius * (1.0 - angle.cos()), -0.35 - radius * angle.sin());
        [[-0.15, y, z], [0.15, y, z]]
    }

    fn one_handed(phase: f32, radius: f32) -> [Vec3; 2] {
        [[-0.15, -0.3, -0.35], sweep(phase, radius)[1]]
    }

    fn punch(phase: f32, _: f32) -> [Vec3; 2] {
        let reach = 0.6 * (TAU * phase).sin().abs();
        [[-0.15, -0.3, -0.35 - reach], [0.15, -0.3, -0.35 - reach]]
    }

    struct Motion {
        shape: fn(f32, f32) -> [Vec3; 2],
        radius: f32,
        seconds: f64,
        pause: Option<(f32, f64)>,
    }

    const LOOP: Motion = Motion { shape: sweep, radius: 0.55, seconds: 1.8, pause: None };

    fn ease(s: f64) -> f32 {
        let s = s.clamp(0.0, 1.0);
        (s * s * (3.0 - 2.0 * s)) as f32
    }

    /// (time, head, left, right) at `rate` Hz: one second still, the motion, then one second still.
    fn trace(motion: &Motion, rate: f64, head: Pose) -> Vec<(f64, Pose, Vec3, Vec3)> {
        let (at, hold) = motion.pause.unwrap_or((1.0, 0.0));
        let length = 2.0 + motion.seconds + hold;
        let split = 1.0 + motion.seconds * at as f64;
        (0..(length * rate) as usize)
            .map(|k| {
                let t = k as f64 / rate;
                let s = if t < split { t - 1.0 } else if t < split + hold { split - 1.0 } else { t - 1.0 - hold };
                let [left, right] = (motion.shape)(ease(s / motion.seconds), motion.radius).map(|hand| head.apply(hand));
                (t, head, left, right)
            })
            .collect()
    }

    fn template(motion: &Motion) -> Vec<Point> {
        let points: Vec<Point> = (0..=100).map(|k| ease(k as f64 / 100.0)).map(|phase| {
            let [l, r] = (motion.shape)(phase, motion.radius);
            [l[0], l[1], l[2], r[0], r[1], r[2]]
        }).collect();
        resample(&points)
    }

    fn file(traces: Vec<Vec<Point>>) -> String {
        serde_json::json!({ "threshold": 0.14, "peak_speed_min": 2.51, "templates": traces }).to_string()
    }

    fn templates(traces: Vec<Vec<Point>>) -> Templates {
        Templates::parse(file(traces).as_bytes()).unwrap()
    }

    fn verdicts(motion: &Motion, rate: f64, head: Pose) -> Vec<Verdict> {
        let mut recognizer = Recognizer::new(templates(vec![template(&LOOP)]));
        trace(motion, rate, head).into_iter().filter_map(|(t, head, left, right)| recognizer.push(t, &head, left, right)).collect()
    }

    #[test]
    fn the_recorded_loop_matches_at_any_sampling_rate() {
        for rate in [90.0, 72.0, 30.0] {
            let found = verdicts(&LOOP, rate, IDENTITY);
            assert_eq!(found.len(), 1, "{rate} Hz: {found:?}");
            assert!(found[0].matched, "{rate} Hz: {found:?}");
            assert!(found[0].distance < 0.05 && found[0].peak > 2.51, "{rate} Hz: {found:?}");
            assert!((found[0].duration - LOOP.seconds).abs() < 0.3, "{rate} Hz: {found:?}");
        }
    }

    #[test]
    fn the_loop_matches_wherever_the_head_stands_and_turns() {
        for (angle, at) in [(1.2, [2.0, 1.7, -3.0]), (-2.5, [-0.5, 1.4, 4.0]), (3.1, [0.0, 1.6, 0.0])] {
            let found = verdicts(&LOOP, 90.0, yawed(angle, at));
            assert!(found.len() == 1 && found[0].matched, "yaw {angle}: {found:?}");
            assert!(found[0].distance < 0.05, "yaw {angle}: {found:?}");
        }
    }

    #[test]
    fn a_turned_head_rotates_the_hands_into_its_yaw_frame() {
        let head = yawed(std::f32::consts::FRAC_PI_2, [1.0, 1.5, 2.0]);
        let ahead = head.apply([0.0, -0.2, -0.5]);
        let point = head_relative(&head, ahead, ahead);
        for (got, want) in point.iter().zip([0.0, -0.2, -0.5, 0.0, -0.2, -0.5]) {
            assert!((got - want).abs() < 1e-5, "{point:?}");
        }
    }

    #[test]
    fn a_slow_loop_is_a_near_miss_not_a_match() {
        let found = verdicts(&Motion { seconds: 5.0, ..LOOP }, 90.0, IDENTITY);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(!found[0].matched && found[0].near && found[0].peak < 2.51, "{found:?}");
    }

    #[test]
    fn a_fast_motion_of_another_shape_does_not_match() {
        let found = verdicts(&Motion { shape: punch, ..LOOP }, 90.0, IDENTITY);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(!found[0].matched && found[0].distance > 0.14 && found[0].peak >= 2.51, "{found:?}");
    }

    #[test]
    fn a_pause_shorter_than_the_stillness_keeps_one_gesture() {
        assert_eq!(verdicts(&Motion { pause: Some((0.5, 0.2)), ..LOOP }, 90.0, IDENTITY).len(), 1);
        assert_eq!(verdicts(&Motion { pause: Some((0.5, 0.6)), ..LOOP }, 90.0, IDENTITY).len(), 2);
    }

    #[test]
    fn a_gesture_is_judged_only_after_the_hands_settle() {
        let mut recognizer = Recognizer::new(templates(vec![template(&LOOP)]));
        let samples = trace(&LOOP, 90.0, IDENTITY);
        let stopped = samples.iter().rposition(|sample| (sample.0 - 1.0 - LOOP.seconds).abs() < 0.006).unwrap();
        let judged = samples.iter().position(|&(t, head, left, right)| recognizer.push(t, &head, left, right).is_some()).unwrap();
        let waited = samples[judged].0 - samples[stopped].0;
        assert!(waited > STILL && waited < STILL + 0.2, "judged {waited} s after the hands stopped");
    }

    #[test]
    fn endless_motion_is_never_judged_and_stays_bounded() {
        let mut recognizer = Recognizer::new(templates(vec![template(&LOOP)]));
        for k in 0..90 * 60 {
            let t = k as f64 / 90.0;
            let [left, right] = sweep((t / 1.8).fract() as f32, 0.55);
            assert_eq!(recognizer.push(t, &IDENTITY, IDENTITY.apply(left), IDENTITY.apply(right)), None, "at {t} s");
            assert!(recognizer.held() < 90 * 11, "{} samples at {t} s", recognizer.held());
        }
    }

    #[test]
    fn stillness_keeps_only_the_smoothing_window() {
        let mut recognizer = Recognizer::new(templates(vec![template(&LOOP)]));
        let hand = IDENTITY.apply([0.0, -0.3, -0.35]);
        for k in 0..900 {
            recognizer.push(k as f64 / 90.0, &IDENTITY, hand, hand);
        }
        assert!(recognizer.held() < 20, "{}", recognizer.held());
    }

    #[test]
    fn templates_must_be_traces_of_the_resampled_length() {
        assert!(Templates::parse(file(vec![vec![[0.0; 6]; POINTS]]).as_bytes()).is_ok());
        assert!(Templates::parse(file(vec![vec![[0.0; 6]; 16]]).as_bytes()).is_err());
        assert!(Templates::parse(file(vec![]).as_bytes()).is_err());
    }

    #[test]
    fn a_match_wakes_only_the_dormant_and_waking_lasts_its_time() {
        let woken = Instant::now();
        assert_eq!(wake(None, false, woken), None);
        assert_eq!(wake(None, true, woken), Some(woken));
        let later = woken + AWAKE_FOR - Duration::from_millis(1);
        assert_eq!(wake(Some(woken), true, later), Some(woken));
        assert_eq!(wake(Some(woken), false, woken + AWAKE_FOR), None);
    }

    #[test]
    fn a_tracking_gap_ends_the_motion_without_a_verdict() {
        let mut recognizer = Recognizer::new(templates(vec![template(&LOOP)]));
        let samples = trace(&LOOP, 90.0, IDENTITY);
        let middle = 1.0 + LOOP.seconds / 2.0;
        let found: Vec<Verdict> = samples.iter().filter(|sample| (sample.0 - middle).abs() > 0.2).filter_map(|&(t, head, left, right)| recognizer.push(t, &head, left, right)).collect();
        assert!(found.iter().all(|verdict| !verdict.matched), "{found:?}");
    }

    #[test]
    fn either_hand_alone_carries_the_speed() {
        let found = verdicts(&Motion { shape: one_handed, ..LOOP }, 90.0, IDENTITY);
        assert!(found.len() == 1 && found[0].peak > 2.51, "{found:?}");
    }

    #[test]
    fn a_flick_shorter_than_a_gesture_is_not_judged() {
        assert_eq!(verdicts(&Motion { seconds: 0.25, ..LOOP }, 90.0, IDENTITY), vec![]);
    }

    #[test]
    fn resampling_interpolates_evenly() {
        let line = resample(&[[0.0; 6], [1.0; 6]]);
        for (k, point) in line.iter().enumerate() {
            assert!((point[0] - k as f32 / 31.0).abs() < 1e-6, "{k}: {point:?}");
        }
        assert_eq!(resample(&[[0.0; 6], [2.0; 6], [4.0; 6]])[0], [0.0; 6]);
        assert!((resample(&[[0.0; 6], [1.0; 6], [5.0; 6]])[POINTS - 1][0] - 5.0).abs() < 1e-6);
    }

    #[test]
    fn dtw_averages_over_both_lengths_within_its_band() {
        let at = |x: f32| [x, 0.0, 0.0, 0.0, 0.0, 0.0];
        let a: Vec<Point> = (0..POINTS).map(|k| at(k as f32)).collect();
        let lifted: Vec<Point> = a.iter().map(|p| [p[0], 3.0, 0.0, 0.0, 0.0, 0.0]).collect();
        assert!((dtw(&a, &lifted) - 3.0 * POINTS as f32 / (2 * POINTS) as f32).abs() < 1e-5);
        let ramp = |delay: usize| -> Vec<Point> { (0..POINTS).map(|k| at(k.saturating_sub(delay).min(16) as f32)).collect() };
        assert_eq!(dtw(&ramp(0), &ramp(BAND - 2)), 0.0);
        assert!(dtw(&ramp(0), &ramp(BAND + 4)) > 0.0);
    }

    #[test]
    fn a_gesture_whose_hands_then_drop_out_of_tracking_is_still_judged() {
        let mut recognizer = Recognizer::new(templates(vec![template(&LOOP)]));
        let stop = 1.0 + LOOP.seconds + 0.1;
        let found: Vec<Verdict> = trace(&LOOP, 90.0, IDENTITY).into_iter().filter(|sample| sample.0 < stop || sample.0 > stop + 0.6).filter_map(|(t, head, left, right)| recognizer.push(t, &head, left, right)).collect();
        assert!(found.len() == 1 && found[0].matched, "{found:?}");
    }

    #[test]
    fn dtw_is_zero_against_itself_and_warps_a_shifted_copy() {
        let a = template(&LOOP);
        assert_eq!(dtw(&a, &a), 0.0);
        let mut shifted = a[2..].to_vec();
        shifted.extend([a[POINTS - 1]; 2]);
        let plain: f32 = a.iter().zip(&shifted).map(|(a, b)| distance(a, b)).sum::<f32>() / POINTS as f32;
        assert!(dtw(&a, &shifted) < plain / 2.0, "{} vs {plain}", dtw(&a, &shifted));
    }
}
