use std::time::{Duration, Instant};

use crate::motion::Random;

pub type Vec3 = [f32; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
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
    pub fn from_axes(x: Vec3, y: Vec3, z: Vec3, t: Vec3) -> Pose {
        Pose { r: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]], t }
    }
    pub fn mat4(&self) -> glam::Mat4 {
        let [x, y, z] = [0, 1, 2].map(|column| self.axis(column));
        glam::Mat4::from_cols_array_2d(&[[x[0], x[1], x[2], 0.0], [y[0], y[1], y[2], 0.0], [z[0], z[1], z[2], 0.0], [self.t[0], self.t[1], self.t[2], 1.0]])
    }
}

/// Where the avatar's feet stand on the controller: its top, in the controller's frame (x right, y up, z toward the elbow).
const STAND: Vec3 = [0.0, 0.03, 0.02];
/// The board's face plane below the controller, in the controller's frame.
const BELOW: f32 = 0.07;
/// The board's edge nearest the elbow, in the controller's frame.
const NEAR_EDGE: f32 = 0.08;
/// Between the board and the display panel beside it.
const SIDE_GAP: f32 = 0.01;
/// The right controller's touch point ahead of its origin, in its frame, for a controller whose render model has no tip.
pub const TIP: Vec3 = [0.0, 0.0, -0.05];

/// The avatar's frame (feet at the origin, y up, z its facing) standing on the controller and turning with it: its head toward the controller's tip and its face toward the controller's top, upright in a grip that holds the tip up.
pub fn on_controller(hand: &Pose) -> Pose {
    hand.then(&Pose { r: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]], t: STAND })
}

/// How long a set-down avatar keeps riding the controller before it stays where it is.
pub const SETTLE: Duration = Duration::from_secs(5);
/// A set-down avatar brought this close to the floor (y = 0 in the standing universe) stands on it at full scale.
pub const GROUND: f32 = 0.3;
/// How long the avatar takes to grow on the floor or shrink back onto the controller.
pub const GLIDE: Duration = Duration::from_millis(600);

/// Where the avatar stands: on the controller, riding it a moment longer after a set-down press, left in the world at hand size, or full size on the floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    Riding,
    Settling(Instant),
    Stuck(Pose),
    Grounded(Floor),
}

impl Anchor {
    /// Riding, a press sets it down after `SETTLE`; otherwise it returns to the controller.
    pub fn toggle(&mut self, now: Instant) {
        *self = match self {
            Anchor::Riding => Anchor::Settling(now + SETTLE),
            _ => Anchor::Riding,
        };
    }

    pub fn grounded(&self) -> bool {
        matches!(self, Anchor::Grounded(_))
    }

    /// The avatar's frame this frame: settling, it stands on the floor facing `viewer` once brought within `GROUND` of it, else stays at the pose it rode to once `SETTLE` has passed.
    pub fn stand(&mut self, now: Instant, hand: &Pose, viewer: Vec3) -> Pose {
        match *self {
            Anchor::Stuck(pose) => pose,
            Anchor::Grounded(floor) => floor.pose,
            Anchor::Settling(until) => {
                let pose = on_controller(hand);
                if pose.t[1] < GROUND {
                    let floor = upright([pose.t[0], 0.0, pose.t[2]], viewer, pose.axis(2));
                    *self = Anchor::Grounded(Floor::new(floor, Random::seeded()));
                    floor
                } else {
                    if now >= until {
                        *self = Anchor::Stuck(pose);
                    }
                    pose
                }
            }
            Anchor::Riding => on_controller(hand),
        }
    }
}

/// The band a floor puppet keeps around the headset, measured across the floor: two to six feet.
pub const NEAREST: f32 = 0.61;
pub const FARTHEST: f32 = 1.83;
/// Half the cone ahead of the headset that counts as in view.
pub const IN_VIEW: f32 = 0.75;
/// Where it walks to: this far off the line of sight, so it stands in view without blocking it.
const ASIDE: [f32; 2] = [0.2, 0.55];
const KEEP: [f32; 2] = [0.9, 1.5];
/// Close enough to its spot to stop walking.
const ARRIVED: f32 = 0.2;
/// The most it heads around the headset at once, so its path circles him instead of crossing him.
const ARC: f32 = 0.6;
/// Radians per second it turns, and the most it may face away from where it is heading and still step.
const TURN: f32 = 2.0;
const STEER: f32 = 1.0;
/// Seconds it stands between wanders inside the band.
const REST: [f32; 2] = [12.0, 24.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Floor {
    pub pose: Pose,
    feet: Option<[Vec3; 2]>,
    goal: Option<Vec3>,
    heading: f32,
    rest: f32,
    random: Random,
}

impl Floor {
    pub fn new(pose: Pose, mut random: Random) -> Self {
        Self { pose, feet: None, goal: None, heading: 0.0, rest: within(REST, random.next()), random }
    }

    /// Whether it wants to step: it has somewhere to go and faces near enough toward it.
    pub fn going(&self) -> bool {
        self.goal.is_some() && self.heading.abs() < STEER
    }

    /// Moves it by the stride of `feet` while `walking`, then turns it toward where it is going, or toward `head` once there.
    pub fn step(&mut self, feet: Option<[Vec3; 2]>, walking: bool, head: &Pose, delta: f32) -> Pose {
        let previous = self.feet;
        self.feet = feet;
        if let (true, Some(previous), Some(feet)) = (walking, previous, feet) {
            let support = usize::from(previous[1][2] - feet[1][2] > previous[0][2] - feet[0][2]);
            self.pose.t = add(self.pose.t, self.pose.rotate(sub(previous[support], feet[support])));
            self.pose.t[1] = 0.0;
        }
        let at = self.pose.t;
        if self.goal.is_some_and(|goal| !welcome(head, goal) || length(level(sub(goal, at))) < ARRIVED) {
            self.goal = None;
        }
        if self.goal.is_none() {
            self.rest -= delta;
            if !welcome(head, at) {
                self.goal = Some(spot(head, at, side(head, at), 0.5, 0.5));
            } else if self.rest <= 0.0 {
                self.rest = within(REST, self.random.next());
                let side = if self.random.next() < 0.5 { -1.0 } else { 1.0 };
                self.goal = Some(spot(head, at, side, self.random.next(), self.random.next()));
            }
        }
        let toward = level(sub(self.goal.map_or(head.t, |goal| aim(head.t, at, goal)), at));
        if length(toward) > 1e-3 {
            let forward = level(self.pose.axis(2));
            self.heading = cross(forward, toward)[1].atan2(dot(forward, toward));
            let turn = self.heading.clamp(-TURN * delta, TURN * delta);
            if turn != 0.0 {
                self.pose = upright(at, add(at, yawed(forward, turn)), forward);
            }
        }
        self.pose
    }
}

fn level(v: Vec3) -> Vec3 {
    [v[0], 0.0, v[2]]
}

fn within([low, high]: [f32; 2], t: f32) -> f32 {
    low + (high - low) * t
}

fn yawed(v: Vec3, angle: f32) -> Vec3 {
    let (sin, cos) = angle.sin_cos();
    [v[0] * cos + v[2] * sin, v[1], v[2] * cos - v[0] * sin]
}

/// Where the headset looks across the floor, if it looks anywhere but straight down or up.
fn ahead(head: &Pose) -> Option<Vec3> {
    let forward = level(scale(head.axis(2), -1.0));
    (length(forward) > 1e-3).then(|| normalize(forward))
}

/// Across the floor from the headset, `at` is in the band and in view.
pub fn welcome(head: &Pose, at: Vec3) -> bool {
    let offset = level(sub(at, head.t));
    let distance = length(offset);
    (NEAREST..=FARTHEST).contains(&distance) && ahead(head).is_none_or(|ahead| dot(ahead, offset) >= distance * IN_VIEW.cos())
}

/// The side of the line of sight `at` stands on, as the sign of a turn about the vertical.
fn side(head: &Pose, at: Vec3) -> f32 {
    let offset = level(sub(at, head.t));
    if ahead(head).is_some_and(|ahead| cross(ahead, offset)[1] < 0.0) { -1.0 } else { 1.0 }
}

/// A spot in view off the line of sight on `side`, `aside` and `keep` choosing how far off it and how far away within their ranges.
fn spot(head: &Pose, at: Vec3, side: f32, aside: f32, keep: f32) -> Vec3 {
    let ahead = ahead(head).unwrap_or_else(|| normalize(level(sub(at, head.t))));
    let mut spot = add(head.t, scale(yawed(ahead, side * within(ASIDE, aside)), within(KEEP, keep)));
    spot[1] = 0.0;
    spot
}

/// The next place to head for on the way from `at` to `goal`: around the headset at the goal's distance, at most `ARC` at a time.
fn aim(head: Vec3, at: Vec3, goal: Vec3) -> Vec3 {
    let [from, to] = [at, goal].map(|point| level(sub(point, head)));
    let bearing = |v: Vec3| v[0].atan2(v[2]);
    let mut turn = bearing(to) - bearing(from);
    turn -= std::f32::consts::TAU * (turn / std::f32::consts::TAU).round();
    if turn.abs() <= ARC || length(from) < 1e-3 {
        return goal;
    }
    add(level(head), scale(yawed(normalize(from), turn.clamp(-ARC, ARC)), length(to)))
}

/// Upright at `at`, facing `viewer` across the floor, or along `otherwise` when the viewer is straight overhead.
fn upright(at: Vec3, viewer: Vec3, otherwise: Vec3) -> Pose {
    let toward = level(sub(viewer, at));
    let z = if length(toward) > 1e-3 { normalize(toward) } else if length(level(otherwise)) > 1e-3 { normalize(level(otherwise)) } else { [0.0, 0.0, 1.0] };
    Pose::from_axes(cross([0.0, 1.0, 0.0], z), [0.0, 1.0, 0.0], z, at)
}

/// Eases the avatar between its places and sizes: a change of size glides from wherever it was shown over `GLIDE`; otherwise it follows its stand exactly.
#[derive(Default)]
pub struct Glide {
    shown: Option<(Pose, f32)>,
    aim: f32,
    from: Option<(Pose, f32, Instant)>,
}

impl Glide {
    /// The pose and size to show for a stand of `pose` at `size` times hand size.
    pub fn step(&mut self, now: Instant, pose: Pose, size: f32) -> (Pose, f32) {
        if let Some((shown, at)) = self.shown.filter(|_| size != self.aim) {
            self.from = Some((shown, at, now));
        }
        self.aim = size;
        let shown = match self.from {
            Some((from, at, start)) if now < start + GLIDE => {
                let t = (now - start).as_secs_f32() / GLIDE.as_secs_f32();
                let t = t * t * (3.0 - 2.0 * t);
                (between(&from, &pose, t), at * (size / at).powf(t))
            }
            _ => {
                self.from = None;
                (pose, size)
            }
        };
        self.shown = Some(shown);
        shown
    }
}

fn between(from: &Pose, to: &Pose, t: f32) -> Pose {
    let rotation = |pose: &Pose| glam::Quat::from_mat4(&pose.mat4());
    let turned = glam::Mat3::from_quat(rotation(from).slerp(rotation(to), t));
    let [x, y, z] = [turned.x_axis, turned.y_axis, turned.z_axis].map(|axis| axis.to_array());
    Pose::from_axes(x, y, z, add(from.t, scale(sub(to.t, from.t), t)))
}

/// The board under the controller, its face toward the floor while the controller is held upright: turned over, it reads with its top away from the elbow.
pub fn under_controller(height: f32) -> Pose {
    Pose::from_axes([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, -1.0, 0.0], [0.0, -BELOW, NEAR_EDGE - height / 2.0])
}

/// A panel of `size` metres to the right of a board of `board_size`, in its plane, their near edges level.
pub fn beside(board: &Pose, [board_width, board_height]: [f32; 2], [width, height]: [f32; 2]) -> Pose {
    board.then(&Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [(board_width + width) / 2.0 + SIDE_GAP, (height - board_height) / 2.0, 0.0] })
}

/// A window centred on `at` square to the line from `viewer`.
pub fn facing(at: Vec3, viewer: Vec3) -> Pose {
    let toward = sub(viewer, at);
    let z = if length(toward) < 1e-3 { [0.0, 0.0, 1.0] } else { normalize(toward) };
    let side = cross([0.0, 1.0, 0.0], z);
    let x = if length(side) < 1e-3 { [1.0, 0.0, 0.0] } else { normalize(side) };
    Pose::from_axes(x, cross(z, x), z, at)
}

/// Nearer than this a window square to the view stands off from the head, so the eyes stay in front of it.
const CLOSEST: f32 = 0.1;

/// A window through the middle of the posed bounds, square to the head: the least
/// square that shows their corners to both eyes, clipped to the headset's view.
/// Bounds crossing an eye fill its view, since their surface can reach any edge.
pub fn window(points: &[Vec3], head: &Pose, eyes: &[Vec3], view: f32) -> (Pose, f32) {
    let (low, high) = points.iter().fold(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |(low, high), point| (std::array::from_fn(|axis| low[axis].min(point[axis])), std::array::from_fn(|axis| high[axis].max(point[axis]))));
    let depth = dot(sub(scale(add(low, high), 0.5), head.t), scale(head.axis(2), -1.0)).max(CLOSEST);
    let plane = Pose { r: head.r, t: head.apply([0.0, 0.0, -depth]) };
    let local = plane.inverse();
    let (mut low, mut high) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for eye in eyes.iter().map(|&eye| local.apply(eye)) {
        let (mut near, mut far) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for &center in points {
            let center = local.apply(center);
            let ahead = eye[2] - center[2];
            if ahead <= CLOSEST / 10.0 {
                (near, far) = ([f32::NEG_INFINITY; 2], [f32::INFINITY; 2]);
                break;
            }
            let t = eye[2] / ahead;
            for axis in 0..2 {
                let at = eye[axis] + (center[axis] - eye[axis]) * t;
                near[axis] = near[axis].min(at);
                far[axis] = far[axis].max(at);
            }
        }
        let seen: [[f32; 2]; 2] = std::array::from_fn(|axis| [near[axis].max(eye[axis] - view * eye[2]), far[axis].min(eye[axis] + view * eye[2])]);
        if seen.iter().all(|[from, to]| from <= to) {
            for axis in 0..2 {
                low[axis] = low[axis].min(seen[axis][0]);
                high[axis] = high[axis].max(seen[axis][1]);
            }
        }
    }
    if low[0] > high[0] {
        (low, high) = ([-view * depth; 2], [view * depth; 2]);
    }
    let half = (0..2).map(|axis| (high[axis] - low[axis]) / 2.0).fold(0.0, f32::max);
    (Pose { t: plane.apply([(low[0] + high[0]) / 2.0, (low[1] + high[1]) / 2.0, 0.0]), ..plane }, half)
}

/// Where the right controller's touch point is in the board's frame.
pub fn local_tip(board: &Pose, right: &Pose, tip: Vec3) -> Vec3 {
    board.inverse().apply(right.apply(tip))
}

pub fn marker(board: &Pose, right: &Pose, at: Vec3) -> Pose {
    right.inverse().then(&Pose { r: board.r, t: board.apply(at) })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub struct HandPose {
    pub hand: Hand,
    pub pose: Pose,
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

    fn rolled(roll: f32, t: Vec3) -> Pose {
        let (s, c) = roll.sin_cos();
        Pose { r: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]], t }
    }

    #[test]
    fn the_avatar_stands_on_the_controller_and_turns_with_it() {
        let hand = turned(0.7, [-0.2, 1.0, -0.3]).then(&Pose { r: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]], t: [0.0; 3] });
        assert!(close(hand.rotate([0.0, 0.0, -1.0]), [0.0, 1.0, 0.0]), "tip up");
        let avatar = on_controller(&hand);
        assert!(close(avatar.axis(1), [0.0, 1.0, 0.0]), "upright on a controller held tip up");
        assert!(close(avatar.axis(2), hand.axis(1)), "faces the controller's top");
        let tilted = hand.then(&rolled(0.5, [0.0; 3]));
        let avatar = on_controller(&tilted);
        assert!(close(avatar.axis(1), tilted.rotate([0.0, 0.0, -1.0])), "leans as the controller rolls");
    }

    #[test]
    fn the_avatar_is_near_upright_in_the_held_grip() {
        let held = Pose::from_m34(&[[0.5984, -0.7703, -0.2204, -0.1249], [0.1491, 0.3774, -0.9140, 0.7643], [0.7872, 0.5141, 0.3407, 0.0554]]);
        assert!(on_controller(&held).axis(1)[1] > 0.9, "within 25 degrees of world up");
    }

    #[test]
    fn a_set_down_avatar_rides_a_moment_then_stays_while_the_controller_moves_until_pressed_back() {
        let start = Instant::now();
        let at = |seconds: f32| start + Duration::from_secs_f32(seconds);
        let hand = |seconds: f32| turned(seconds, [seconds, 1.0, -0.3]);
        let mut anchor = Anchor::Riding;
        assert_eq!(anchor.stand(at(0.0), &hand(0.0), VIEWER), on_controller(&hand(0.0)));
        anchor.toggle(at(0.0));
        let late = SETTLE.as_secs_f32() - 0.01;
        assert_eq!(anchor.stand(at(late), &hand(late), VIEWER), on_controller(&hand(late)), "still riding just before it settles");
        let settled = SETTLE.as_secs_f32();
        let left = anchor.stand(at(settled), &hand(settled), VIEWER);
        assert_eq!(left, on_controller(&hand(settled)), "stays where it rode to");
        for seconds in [0.5, 2.0, 8.0].map(|after| settled + after) {
            assert_eq!(anchor.stand(at(seconds), &hand(seconds), VIEWER), left, "held while the controller moves");
        }
        let back = settled + 9.0;
        anchor.toggle(at(back));
        assert_eq!(anchor, Anchor::Riding);
        assert_eq!(anchor.stand(at(back), &hand(back), VIEWER), on_controller(&hand(back)), "back on the controller");
        anchor.toggle(at(back + 1.0));
        anchor.toggle(at(back + 1.5));
        let after = back + 1.0 + settled + 1.0;
        assert_eq!(anchor.stand(at(after), &hand(after), VIEWER), on_controller(&hand(after)), "a second press while settling keeps it riding");
    }

    const VIEWER: Vec3 = [0.0, 1.6, 0.0];
    /// With no time passing the floor turns nowhere, so these isolate the stride.
    const STILL: Pose = Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: VIEWER };

    fn walking_feet(time: f32) -> [Vec3; 2] {
        let phase = time.rem_euclid(1.0);
        let (support, phase) = if phase < 0.5 { (0, phase * 2.0) } else { (1, (phase - 0.5) * 2.0) };
        let mut feet = [[-0.1, 0.0, 0.0], [0.1, 0.0, 0.0]];
        feet[support][2] = 0.25 - 0.5 * phase;
        feet[1 - support][2] = -feet[support][2];
        feet[1 - support][1] = 0.15 * (phase * std::f32::consts::PI).sin();
        feet
    }

    #[test]
    fn walking_cancels_the_authored_support_foot_travel_at_any_scale_or_frame_rate() {
        for rate in [30, 90, 144] {
            for size in [0.6, 1.0, 1.5] {
                let mut floor = Floor::new(turned(0.0, [0.0; 3]), Random(7));
                let mut previous = walking_feet(0.0).map(|f| scale(f, size));
                floor.step(Some(previous), true, &STILL, 0.0);
                for frame in 1..=rate {
                    let feet = walking_feet(frame as f32 / rate as f32).map(|f| scale(f, size));
                    let support = usize::from(previous[1][2] - feet[1][2] > previous[0][2] - feet[0][2]);
                    let planted = floor.pose.apply(previous[support]);
                    let next = floor.step(Some(feet), true, &STILL, 0.0);
                    let moved = sub(next.apply(feet[support]), planted);
                    assert!(length([moved[0], 0.0, moved[2]]) < 1e-5);
                    previous = feet;
                }
                assert!((floor.pose.t[2] - size).abs() < 0.04, "stride comes from foot travel, scaled with the model: {:?}", floor.pose.t);
            }
        }
    }

    #[test]
    fn placement_and_resuming_a_step_preserve_any_chosen_position() {
        for position in [[0.0; 3], [3.0, 0.0, -4.0]] {
            let initial = turned(0.7, position);
            let mut floor = Floor::new(initial, Random(7));
            for frame in 0..900 {
                assert_eq!(floor.step(Some(walking_feet(frame as f32 / 90.0)), false, &STILL, 0.0), initial);
            }
            floor.step(Some(walking_feet(0.7)), false, &STILL, 0.0);
            assert_eq!(floor.step(Some(walking_feet(0.7)), true, &STILL, 0.0), initial);
            for frame in 1..=900 {
                let pose = floor.step(Some(walking_feet(0.7 + frame as f32 / 90.0)), true, &STILL, 0.0);
                assert_eq!(pose.r, initial.r);
            }
            assert!(length(sub(floor.pose.t, initial.t)) > 9.0);
            let stopped = floor.step(None, false, &STILL, 0.0);
            assert_eq!(floor.step(Some(walking_feet(0.2)), true, &STILL, 0.0), stopped);
        }
    }

    #[test]
    fn the_bundled_stride_moves_the_hips_forward_across_support_changes_and_cycle_seams() {
        use crate::{motion::Clip, vrm::{Model, Fit}};
        let model = Model::parse(include_bytes!("../golden/figure.vrm")).unwrap();
        let fit = Fit::new(&model, &model.skinned().unwrap(), 1.67);
        let clip = Clip::parse(include_bytes!("../motions/walk.json"), model.version, model.rest_hips()).unwrap();
        for rate in [30, 90, 144] {
            let mut floor = Floor::new(turned(0.0, [0.0; 3]), Random(7));
            let mut previous = None;
            for frame in 0..rate * 10 {
                let worlds = model.worlds(&model.pose(&clip.sample((frame as f32 / rate as f32) % clip.duration())));
                let fitted = fit.placement(&model, &worlds, 0.0);
                let feet = model.feet(&worlds).unwrap().map(|foot| fitted.transform_point3(foot).to_array());
                let stand = floor.step(Some(feet), true, &STILL, 0.0);
                let hips = (stand.mat4() * fitted * worlds[1]).w_axis.z;
                if let Some(before) = previous { assert!(hips >= before - 1e-6, "rate {rate} frame {frame}: {before} -> {hips}"); }
                previous = Some(hips);
            }
        }
    }

    #[test]
    fn an_idle_to_stride_transition_keeps_the_support_foot_planted() {
        let mut floor = Floor::new(turned(0.0, [2.0, 0.0, 3.0]), Random(7));
        let standing = [[-0.2, 0.08, 0.018], [0.2, 0.078, -0.018]];
        let stepping = [[-0.197, 0.081, 0.023], [0.197, 0.078, -0.023]];
        floor.step(Some(standing), false, &STILL, 0.0);
        let planted = floor.pose.apply(standing[1]);
        let next = floor.step(Some(stepping), true, &STILL, 0.0);
        assert!(close(next.apply(stepping[1]), planted));
    }

    /// A headset at standing height turned `yaw` about the vertical, looking down `pitch`.
    fn headset(yaw: f32, pitch: f32, at: Vec3) -> Pose {
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let z = [sy * cp, sp, cy * cp];
        let x = normalize(cross([0.0, 1.0, 0.0], z));
        Pose::from_axes(x, cross(z, x), z, at)
    }

    /// Steps a floor puppet at 90 Hz, its feet striding whenever it asks to walk, returning every pose it stood in.
    fn walk_for(floor: &mut Floor, head: impl Fn(f32) -> Pose, seconds: f32) -> Vec<Pose> {
        let mut gait = 0.0;
        (0..(seconds * 90.0) as usize).map(|frame| {
            let going = floor.going();
            if going { gait += 1.0 / 90.0; }
            floor.step(Some(walking_feet(gait)), going, &head(frame as f32 / 90.0), 1.0 / 90.0)
        }).collect()
    }

    fn across(head: &Pose, at: Vec3) -> f32 {
        length([at[0] - head.t[0], 0.0, at[2] - head.t[2]])
    }

    /// How far it faces away from the headset, and how far off the line of sight it stands, across the floor.
    fn angles(head: &Pose, pose: &Pose) -> (f32, f32) {
        let toward = normalize([head.t[0] - pose.t[0], 0.0, head.t[2] - pose.t[2]]);
        let facing = normalize([pose.axis(2)[0], 0.0, pose.axis(2)[2]]);
        let ahead = normalize([-head.axis(2)[0], 0.0, -head.axis(2)[2]]);
        (dot(facing, toward).clamp(-1.0, 1.0).acos(), dot(ahead, scale(toward, -1.0)).clamp(-1.0, 1.0).acos())
    }

    #[test]
    fn a_floor_puppet_walks_back_into_the_band_in_view_and_settles_facing_the_headset() {
        for yaw in [0.0, 2.0, -2.6] {
            let head = headset(yaw, -0.3, [0.4, 1.6, -0.2]);
            for (start, facing) in [([0.4, 0.0, -4.0], 0.0), ([0.4, 0.0, 3.5], 0.0), ([3.0, 0.0, 2.5], 1.0), ([-5.0, 0.0, -0.2], 3.0), ([0.5, 0.0, -0.1], 2.0)] {
                let mut floor = Floor::new(turned(facing, start), Random(7));
                let poses = walk_for(&mut floor, |_| head, 40.0);
                let end = poses.last().unwrap();
                let distance = across(&head, end.t);
                let (away, aside) = angles(&head, end);
                let label = format!("headset yaw {yaw}, start {start:?}: ended {distance:.2} m off, facing {away:.2} rad away, {aside:.2} rad off the line of sight");
                assert!((NEAREST..=FARTHEST).contains(&distance), "{label}");
                assert!(away < 0.05, "settles facing the headset: {label}");
                assert!((0.1..IN_VIEW).contains(&aside), "in view, not dead centre: {label}");
                assert!(!floor.going(), "settled: {label}");
                let entered = poses.iter().position(|pose| (NEAREST..=FARTHEST).contains(&across(&head, pose.t))).unwrap();
                assert!(poses[entered..].iter().all(|pose| across(&head, pose.t) >= NEAREST * 0.9), "once in the band it circles him rather than crossing him: {label}");
                assert!(poses.iter().all(|pose| pose.t[1] == 0.0 && (pose.axis(1)[1] - 1.0).abs() < 1e-5), "upright on the floor: {label}");
            }
        }
    }

    #[test]
    fn a_settled_puppet_wanders_only_within_the_band_and_follows_the_view_round() {
        let head = |time: f32| headset(if time < 120.0 { 0.0 } else { std::f32::consts::PI }, 0.0, [0.0, 1.6, 0.0]);
        let mut floor = Floor::new(turned(0.0, [0.3, 0.0, -1.2]), Random(7));
        let poses = walk_for(&mut floor, head, 180.0);
        let wandered = poses[..120 * 90].iter().map(|pose| length(sub(pose.t, poses[0].t))).fold(0.0, f32::max);
        assert!(wandered > 0.3, "it wanders: {wandered}");
        assert!(poses[..120 * 90].iter().all(|pose| welcome(&head(0.0), pose.t)), "every wander stays in the band and in view");
        assert!(poses[120 * 90..].iter().all(|pose| across(&head(150.0), pose.t) >= NEAREST * 0.9), "it walks round him, not through him");
        assert!(poses[150 * 90..].iter().all(|pose| welcome(&head(180.0), pose.t)), "in view again within half a minute of his turning round");
        assert!(poses[150 * 90..].iter().any(|pose| angles(&head(180.0), pose).0 < 0.05), "and stands facing him");
    }

    #[test]
    fn a_puppet_set_down_near_the_floor_stands_on_it_facing_the_viewer_until_picked_up() {
        let start = Instant::now();
        let mut anchor = Anchor::Riding;
        anchor.toggle(start);
        let high = turned(0.4, [0.5, 1.0, -0.5]);
        assert_eq!(anchor.stand(start, &high, VIEWER), on_controller(&high), "rides while held high");
        let low = turned(0.4, [0.5, GROUND - 0.1, -0.5]);
        let floor = anchor.stand(start + Duration::from_secs(1), &low, VIEWER);
        assert!(anchor.grounded());
        assert!(close(floor.t, [on_controller(&low).t[0], 0.0, on_controller(&low).t[2]]), "feet on the floor below the controller: {:?}", floor.t);
        assert!(close(floor.axis(1), [0.0, 1.0, 0.0]), "upright");
        assert!(close(floor.axis(2), normalize([-floor.t[0], 0.0, -floor.t[2]])), "facing the viewer across the floor");
        assert_eq!(anchor.stand(start + SETTLE * 2, &high, VIEWER), floor, "stays as the controller rises");
        anchor.toggle(start + SETTLE * 3);
        assert_eq!(anchor.stand(start + SETTLE * 3, &high, VIEWER), on_controller(&high), "picked up");
        anchor.toggle(start);
        assert_eq!(anchor.stand(start + SETTLE, &high, VIEWER), on_controller(&high));
        assert!(!anchor.grounded(), "set down above the floor it stays hand size");
        assert_eq!(anchor.stand(start + SETTLE * 2, &low, VIEWER), on_controller(&high), "and stays put when the controller drops afterwards");
    }

    #[test]
    fn a_change_of_size_glides_from_where_it_was_shown_then_follows_exactly() {
        let start = Instant::now();
        let at = |seconds: f32| start + Duration::from_secs_f32(seconds);
        let (hand, floor) = (turned(0.0, [0.0, 1.0, -0.3]), turned(1.0, [0.4, 0.0, -0.8]));
        let mut glide = Glide::default();
        assert_eq!(glide.step(at(0.0), hand, 1.0), (hand, 1.0));
        let same = |a: &Pose, b: &Pose| close(a.t, b.t) && (0..3).all(|column| close(a.axis(column), b.axis(column)));
        let (pose, size) = glide.step(at(0.0), floor, 6.0);
        assert!(same(&pose, &hand) && size == 1.0, "starts where it was");
        let half = GLIDE.as_secs_f32() / 2.0;
        let (pose, size) = glide.step(at(half), floor, 6.0);
        assert!(close(pose.t, [0.2, 0.5, -0.55]), "halfway there: {:?}", pose.t);
        assert!((size - 6.0f32.sqrt()).abs() < 1e-4, "halfway in proportion: {size}");
        assert!(close(pose.axis(1), [0.0, 1.0, 0.0]) && close(pose.axis(0), turned(0.5, [0.0; 3]).axis(0)), "turned halfway");
        assert_eq!(glide.step(start + GLIDE, floor, 6.0), (floor, 6.0));
        let moved = turned(0.2, [0.1, 1.1, -0.3]);
        let (pose, size) = glide.step(at(5.0), moved, 1.0);
        assert!(same(&pose, &floor) && size == 6.0, "picking up glides back from the floor");
        assert_eq!(glide.step(at(5.0) + GLIDE, moved, 1.0), (moved, 1.0), "then rides exactly");
    }

    #[test]
    fn the_board_faces_the_floor_until_the_controller_is_turned_over() {
        let hand = turned(0.0, [0.0, 1.0, -0.3]);
        let height = 0.2;
        let board = hand.then(&under_controller(height));
        assert!(board.t[1] < hand.t[1] - 0.05, "below the controller");
        assert!(close(board.axis(2), [0.0, -1.0, 0.0]), "face down");
        let over = hand.then(&rolled(std::f32::consts::PI, [0.0; 3]));
        let board = over.then(&under_controller(height));
        assert!(board.t[1] > over.t[1] + 0.05, "above the turned-over controller");
        assert!(close(board.axis(2), [0.0, 1.0, 0.0]), "face up");
        assert!(close(board.axis(0), [1.0, 0.0, 0.0]), "its rows run to the right");
        assert!(close(board.axis(1), [0.0, 0.0, -1.0]), "its top away from the elbow");
        assert!(close(board.apply([0.0, -height / 2.0, 0.0]), over.apply([0.0, -BELOW, NEAR_EDGE])), "a taller board grows away from the elbow");
    }

    #[test]
    fn the_panel_lies_right_of_the_turned_over_board_in_its_plane_and_near_edge() {
        let over = rolled(std::f32::consts::PI, [0.0, 1.0, -0.3]);
        let (board_size, size) = ([0.2, 0.15], [0.3, 0.25]);
        let mount = under_controller(board_size[1]);
        let board = over.then(&mount);
        let panel = over.then(&beside(&mount, board_size, size));
        assert_eq!(panel.r, board.r, "turns with the board");
        let near = |pose: &Pose, [width, height]: [f32; 2]| pose.apply([-width / 2.0, -height / 2.0, 0.0]);
        let gap = sub(near(&panel, size), board.apply([board_size[0] / 2.0, -board_size[1] / 2.0, 0.0]));
        assert!(close(gap, [SIDE_GAP, 0.0, 0.0]), "a gap to the right, near edges level: {gap:?}");
    }

    fn looking(at: Vec3, toward: Vec3) -> Pose {
        let z = normalize(sub(at, toward));
        let x = normalize(cross([0.0, 1.0, 0.0], z));
        Pose::from_axes(x, cross(z, x), z, at)
    }

    #[test]
    fn a_window_holds_every_point_of_a_box_either_eye_can_see() {
        let (low, high) = ([-0.8, 0.0, -0.3], [0.8, 2.5, 0.2]);
        let corners: Vec<Vec3> = (0..8).map(|k| [if k & 1 == 0 { low[0] } else { high[0] }, if k & 2 == 0 { low[1] } else { high[1] }, if k & 4 == 0 { low[2] } else { high[2] }]).collect();
        let view = 1.4;
        for head in [looking([0.0, 1.6, 2.0], [0.0, 1.2, 0.0]), looking([0.0, 1.6, 0.5], [0.0, 2.6, 0.0]), looking([0.0, 1.6, 0.5], [0.0, 0.0, 0.0]), looking([1.2, 1.6, 0.4], [0.0, 1.0, 0.0]), looking([0.3, 0.8, 1.1], [1.5, 0.8, 0.0])] {
            let eyes = [head.apply([-0.032, 0.0, 0.0]), head.apply([0.032, 0.0, 0.0])];
            let (window, half) = window(&corners, &head, &eyes, view);
            assert!(close(window.axis(2), head.axis(2)) && close(window.axis(0), head.axis(0)), "square to the head's view");
            let local = window.inverse();
            let mut inside = 0;
            for eye in eyes {
                let (from_head, from_window) = (head.inverse(), local.apply(eye));
                for step in 0..2000 {
                    let mix = |axis: usize, k: f32| low[axis] + (high[axis] - low[axis]) * (k * (step as f32 + 1.0)).fract();
                    let point = [mix(0, 0.618), mix(1, 0.414), mix(2, 0.732)];
                    let seen = sub(from_head.apply(point), from_head.apply(eye));
                    if seen[2] >= 0.0 || seen[0].abs() > -view * seen[2] || seen[1].abs() > -view * seen[2] {
                        continue;
                    }
                    inside += 1;
                    let point = local.apply(point);
                    let t = from_window[2] / (from_window[2] - point[2]);
                    let [x, y] = [0, 1].map(|axis| from_window[axis] + (point[axis] - from_window[axis]) * t);
                    assert!(x.abs() <= half + 1e-4 && y.abs() <= half + 1e-4, "a point the eye sees lands inside: ({x}, {y}) against {half}");
                }
            }
            assert!(inside > 0);
            let depth = -local.apply(head.t)[2];
            assert!(half <= (view * depth + 0.032) * 1.0001 || depth < CLOSEST * 1.0001, "no wider than the view: {half} at {depth}");
        }
        let head = looking([0.0, 1.0, 3.0], [0.0, 1.0, 0.0]);
        let (_, bare) = window(&[[0.0, 1.0, 0.0]], &head, &[head.t], view);
        assert!(bare.abs() < 1e-6);
        let (window, around) = window(&[[0.0, 1.0, 2.9], [0.0, 1.0, 3.1]], &head, &[head.t], view);
        assert!((around - view * CLOSEST).abs() < 1e-5 && close(window.t, head.apply([0.0, 0.0, -CLOSEST])), "bounds around the eye fill the view: {around}");
    }

    #[test]
    fn a_window_squares_to_the_viewer_with_level_rows() {
        let window = facing([0.2, 1.0, -0.3], [0.0, 1.6, 0.2]);
        assert!(close(window.axis(2), normalize(sub([0.0, 1.6, 0.2], window.t))));
        assert!(window.axis(0)[1].abs() < 1e-5);
        assert!(window.axis(1)[1] > 0.0);
    }
}
