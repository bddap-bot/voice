use serde::{Deserialize, Serialize};

pub type Vec3 = [f32; 3];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub r: [[f32; 3]; 3],
    pub t: Vec3,
}

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
pub fn normalize(a: Vec3) -> Vec3 {
    scale(a, 1.0 / length(a))
}

impl Pose {

    pub fn from_m34(m: &[[f32; 4]; 3]) -> Pose {
        Pose {
            r: [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]],
            t: [m[0][3], m[1][3], m[2][3]],
        }
    }
    pub fn to_m34(&self) -> [[f32; 4]; 3] {
        let r = &self.r;
        [[r[0][0], r[0][1], r[0][2], self.t[0]], [r[1][0], r[1][1], r[1][2], self.t[1]], [r[2][0], r[2][1], r[2][2], self.t[2]]]
    }
    pub fn rotate(&self, v: Vec3) -> Vec3 {
        let r = &self.r;
        [dot(r[0], v), dot(r[1], v), dot(r[2], v)]
    }
    pub fn apply(&self, v: Vec3) -> Vec3 {
        add(self.rotate(v), self.t)
    }
    pub fn axis(&self, column: usize) -> Vec3 {
        [self.r[0][column], self.r[1][column], self.r[2][column]]
    }
    pub fn then(&self, child: &Pose) -> Pose {
        let mut r = [[0.0; 3]; 3];
        for (row, out) in r.iter_mut().enumerate() {
            for (column, value) in out.iter_mut().enumerate() {
                *value = dot(self.r[row], child.axis(column));
            }
        }
        Pose { r, t: self.apply(child.t) }
    }
    pub fn inverse(&self) -> Pose {
        let mut r = [[0.0; 3]; 3];
        for (row, out) in r.iter_mut().enumerate() {
            *out = self.axis(row);
        }
        let inverse = Pose { r, t: [0.0; 3] };
        Pose { t: scale(inverse.rotate(self.t), -1.0), ..inverse }
    }
    fn from_axes(x: Vec3, y: Vec3, z: Vec3, t: Vec3) -> Pose {
        Pose { r: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]], t }
    }
}

const UP: Vec3 = [0.0, 1.0, 0.0];

pub fn upright_facing(at: Vec3, viewer: Vec3) -> Pose {
    let mut toward = sub(viewer, at);
    toward[1] = 0.0;
    let z = if length(toward) < 1e-3 { [0.0, 0.0, 1.0] } else { normalize(toward) };
    Pose::from_axes(cross(UP, z), UP, z, at)
}

pub fn desk_spot(head: &Pose) -> Pose {
    let mut forward = scale(head.axis(2), -1.0);
    forward[1] = 0.0;
    let forward = if length(forward) < 1e-3 { [0.0, 0.0, -1.0] } else { normalize(forward) };
    let left = cross(UP, forward);
    let at = add(add(head.t, scale(forward, 0.55)), add(scale(left, 0.25), [0.0, -0.4, 0.0]));
    upright_facing(at, head.t)
}

const LIFT: f32 = 0.05;

pub fn above_hand(hand: &Pose, head: &Pose, above_feet: f32) -> Pose {
    upright_facing(add(hand.t, [0.0, LIFT + above_feet, 0.0]), head.t)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hand {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Anchor {
    World(Pose),
    Wrist { hand: Hand, offset: Pose },
}

#[derive(Clone, Copy, Debug)]
pub struct HandPose {
    pub hand: Hand,
    pub pose: Pose,
    pub speed: f32,
}

#[derive(Debug, PartialEq)]
pub enum Event {
    Tap,
    Moved(Anchor),
}

pub struct Rules {
    pub reach: f32,
    pub tap_max: f32,
    pub grab_after: f32,
    pub still_speed: f32,
    pub still_for: f32,
    pub wrist_reach: f32,
}

pub const RULES: Rules = Rules { reach: 0.09, tap_max: 0.6, grab_after: 1.0, still_speed: 0.04, still_for: 0.8, wrist_reach: 0.14 };

enum State {
    Idle,
    Touching { hand: Hand, since: f32 },
    Carrying { hand: Hand, offset: Pose, still_since: Option<f32> },
    Away { hand: Hand },
}

pub struct Interaction {
    state: State,
    pub hot: Vec3,
}

impl Interaction {
    pub fn new(hot: Vec3) -> Interaction {
        Interaction { state: State::Idle, hot }
    }

    pub fn carrying(&self) -> Option<(Hand, Pose)> {
        match self.state {
            State::Carrying { hand, offset, .. } => Some((hand, offset)),
            _ => None,
        }
    }

    pub fn step(&mut self, now: f32, anchor: &Anchor, placed: &Pose, hands: &[HandPose], head: &Pose) -> Option<Event> {
        let worn = match anchor {
            Anchor::Wrist { hand, .. } => Some(*hand),
            Anchor::World(_) => None,
        };
        let hot = placed.apply(self.hot);
        let near = |hand: &HandPose| length(sub(hand.pose.t, hot)) < RULES.reach;
        let find = |which: Hand| hands.iter().find(|candidate| candidate.hand == which);
        match self.state {
            State::Idle => {
                if let Some(hand) = hands.iter().find(|hand| Some(hand.hand) != worn && near(hand)) {
                    self.state = State::Touching { hand: hand.hand, since: now };
                }
                None
            }
            State::Away { hand } => {
                if !find(hand).is_some_and(near) {
                    self.state = State::Idle;
                }
                None
            }
            State::Touching { hand, since } => match find(hand) {
                Some(pose) if near(pose) => {
                    if now - since >= RULES.grab_after {
                        self.state = State::Carrying { hand, offset: pose.pose.inverse().then(placed), still_since: None };
                    }
                    None
                }
                _ => {
                    self.state = State::Idle;
                    (now - since <= RULES.tap_max).then_some(Event::Tap)
                }
            },
            State::Carrying { hand, offset, still_since } => {
                let Some(pose) = find(hand) else {
                    self.state = State::Idle;
                    return Some(Event::Moved(Anchor::World(upright_facing(placed.t, head.t))));
                };
                if pose.speed > RULES.still_speed {
                    self.state = State::Carrying { hand, offset, still_since: None };
                    return None;
                }
                let since = still_since.unwrap_or(now);
                if now - since < RULES.still_for {
                    self.state = State::Carrying { hand, offset, still_since: Some(since) };
                    return None;
                }
                self.state = State::Away { hand };
                let carried = pose.pose.then(&offset);
                let other = hands.iter().find(|candidate| candidate.hand != hand && length(sub(candidate.pose.t, carried.apply(self.hot))) < RULES.wrist_reach);
                Some(Event::Moved(match other {
                    Some(wrist) => Anchor::Wrist { hand: wrist.hand, offset: wrist.pose.inverse().then(&carried) },
                    None => Anchor::World(upright_facing(carried.t, head.t)),
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        length(sub(a, b)) < 1e-4
    }

    fn turned(yaw: f32, t: Vec3) -> Pose {
        let (s, c) = yaw.sin_cos();
        Pose { r: [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]], t }
    }

    #[test]
    fn inverse_undoes_a_pose() {
        let pose = turned(0.7, [1.0, 2.0, 3.0]);
        let point = [0.3, -0.2, 0.9];
        assert!(close(pose.inverse().apply(pose.apply(point)), point));
        assert!(close(pose.then(&pose.inverse()).apply(point), point));
        assert_eq!(Pose::from_m34(&pose.to_m34()), pose);
    }

    #[test]
    fn the_desk_spot_is_ahead_left_and_below_and_faces_the_head() {
        let head = turned(0.0, [0.0, 1.6, 0.0]);
        let spot = desk_spot(&head);
        assert!(close(spot.t, [-0.25, 1.2, -0.55]));
        let normal = spot.axis(2);
        let toward = normalize(sub([0.0, 1.2, 0.0], spot.t));
        assert!(dot(normal, toward) > 0.99);
        assert!(close(spot.axis(1), UP));
    }

    #[test]
    fn above_a_hand_it_stands_upright_above_it_and_faces_the_head() {
        let hand = Pose { r: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]], t: [-0.2, 1.0, -0.3] };
        let head = turned(0.0, [0.0, 1.6, 0.2]);
        let quad = above_hand(&hand, &head, 0.17);
        assert!(close(quad.t, [-0.2, 1.22, -0.3]));
        assert!(close(quad.axis(1), UP), "the hand's tilt does not tip the avatar over");
        let toward = normalize([0.2, 0.0, 0.5]);
        assert!(dot(quad.axis(2), toward) > 0.999);
    }

    fn hand(hand: Hand, t: Vec3, speed: f32) -> HandPose {
        HandPose { hand, pose: turned(0.0, t), speed }
    }

    #[test]
    fn a_quick_touch_taps_and_a_pass_far_away_does_nothing() {
        let placed = turned(0.0, [0.0, 1.0, -0.5]);
        let anchor = Anchor::World(placed);
        let head = turned(0.0, [0.0, 1.6, 0.0]);
        let mut interaction = Interaction::new([0.0, 0.0, 0.0]);
        assert_eq!(interaction.step(0.0, &anchor, &placed, &[hand(Hand::Right, [0.5, 1.0, -0.5], 1.0)], &head), None);
        assert_eq!(interaction.step(0.1, &anchor, &placed, &[hand(Hand::Right, [0.0, 1.02, -0.5], 1.0)], &head), None);
        assert_eq!(interaction.step(0.4, &anchor, &placed, &[hand(Hand::Right, [0.3, 1.0, -0.5], 1.0)], &head), Some(Event::Tap));
        interaction.step(1.0, &anchor, &placed, &[hand(Hand::Right, [0.0, 1.0, -0.5], 0.0)], &head);
        assert_eq!(interaction.step(2.0, &anchor, &placed, &[hand(Hand::Right, [0.3, 1.0, -0.5], 1.0)], &head), None, "a long touch is not a tap");
    }

    #[test]
    fn a_held_touch_carries_it_and_holding_still_puts_it_down_upright() {
        let placed = turned(0.0, [0.0, 1.0, -0.5]);
        let anchor = Anchor::World(placed);
        let head = turned(0.0, [0.0, 1.6, 0.0]);
        let mut interaction = Interaction::new([0.0, 0.0, 0.0]);
        interaction.step(0.0, &anchor, &placed, &[hand(Hand::Right, [0.0, 1.0, -0.5], 0.0)], &head);
        interaction.step(1.1, &anchor, &placed, &[hand(Hand::Right, [0.0, 1.0, -0.5], 0.0)], &head);
        let (_, offset) = interaction.carrying().expect("carrying after the dwell");
        let tilted = Pose { r: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]], t: [0.4, 1.1, -0.3] };
        let moving = HandPose { hand: Hand::Right, pose: tilted, speed: 0.5 };
        assert_eq!(interaction.step(1.5, &anchor, &tilted.then(&offset), &[moving], &head), None);
        let still = HandPose { speed: 0.0, ..moving };
        assert_eq!(interaction.step(2.0, &anchor, &tilted.then(&offset), &[still], &head), None);
        let Some(Event::Moved(Anchor::World(dropped))) = interaction.step(2.9, &anchor, &tilted.then(&offset), &[still], &head) else { panic!("not dropped") };
        assert!(close(dropped.t, [0.4, 1.1, -0.3]));
        assert!(close(dropped.axis(1), UP));
        assert_eq!(interaction.step(3.5, &Anchor::World(dropped), &dropped, &[still], &head), None, "the releasing hand does not grab it again");
    }

    #[test]
    fn dropped_beside_the_other_hand_it_rides_that_wrist() {
        let placed = turned(0.0, [0.0, 1.0, -0.5]);
        let anchor = Anchor::World(placed);
        let head = turned(0.0, [0.0, 1.6, 0.0]);
        let mut interaction = Interaction::new([0.0, 0.0, 0.0]);
        interaction.step(0.0, &anchor, &placed, &[hand(Hand::Right, [0.0, 1.0, -0.5], 0.0)], &head);
        interaction.step(1.1, &anchor, &placed, &[hand(Hand::Right, [0.0, 1.0, -0.5], 0.0)], &head);
        let left = hand(Hand::Left, [-0.2, 1.0, -0.3], 0.0);
        let right = hand(Hand::Right, [-0.15, 1.0, -0.3], 0.0);
        interaction.step(2.0, &anchor, &placed, &[left, right], &head);
        let Some(Event::Moved(Anchor::Wrist { hand: Hand::Left, offset })) = interaction.step(3.0, &anchor, &placed, &[left, right], &head) else { panic!("not on the wrist") };
        assert!(close(left.pose.then(&offset).t, [-0.15, 1.0, -0.3]));
    }
}
