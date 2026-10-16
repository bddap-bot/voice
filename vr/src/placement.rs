use std::time::{Duration, Instant};

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
    Grounded(Pose),
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
            Anchor::Stuck(pose) | Anchor::Grounded(pose) => pose,
            Anchor::Settling(until) => {
                let pose = on_controller(hand);
                if pose.t[1] < GROUND {
                    let floor = upright([pose.t[0], 0.0, pose.t[2]], viewer, pose.axis(2));
                    *self = Anchor::Grounded(floor);
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

/// Upright at `at`, facing `viewer` across the floor, or along `otherwise` when the viewer is straight overhead.
fn upright(at: Vec3, viewer: Vec3, otherwise: Vec3) -> Pose {
    let level = |v: Vec3| [v[0], 0.0, v[2]];
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

/// A window square to the head's view through the middle of `balls`, and its half width: the least that shows every ball to each of `eyes`, cut to what each eye can see, `view` being the tangent of the widest the headset shows off its axis.
/// A ball reaching back past an eye fills that eye's view: the surface between it and the balls ahead can reach any edge.
pub fn window(balls: &[(Vec3, f32)], head: &Pose, eyes: &[Vec3], view: f32) -> (Pose, f32) {
    let (low, high) = balls.iter().fold(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |(low, high), (point, _)| (std::array::from_fn(|axis| low[axis].min(point[axis])), std::array::from_fn(|axis| high[axis].max(point[axis]))));
    let depth = dot(sub(scale(add(low, high), 0.5), head.t), scale(head.axis(2), -1.0)).max(CLOSEST);
    let plane = Pose { r: head.r, t: head.apply([0.0, 0.0, -depth]) };
    let local = plane.inverse();
    let (mut low, mut high) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for eye in eyes.iter().map(|&eye| local.apply(eye)) {
        let (mut near, mut far) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for &(center, radius) in balls {
            let center = local.apply(center);
            let ahead = eye[2] - center[2];
            if ahead - radius <= CLOSEST / 10.0 {
                (near, far) = ([f32::NEG_INFINITY; 2], [f32::INFINITY; 2]);
                break;
            }
            let (t, spread) = (eye[2] / ahead, radius * eye[2] / (ahead - radius));
            for axis in 0..2 {
                let at = eye[axis] + (center[axis] - eye[axis]) * t;
                near[axis] = near[axis].min(at - spread);
                far[axis] = far[axis].max(at + spread);
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
        let corners: Vec<(Vec3, f32)> = (0..8).map(|k| ([if k & 1 == 0 { low[0] } else { high[0] }, if k & 2 == 0 { low[1] } else { high[1] }, if k & 4 == 0 { low[2] } else { high[2] }], 0.0)).collect();
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
        let (_, bare) = window(&[([0.0, 1.0, 0.0], 0.0)], &head, &[head.t], view);
        let (_, padded) = window(&[([0.0, 1.0, 0.0], 0.2)], &head, &[head.t], view);
        assert!(bare.abs() < 1e-6 && padded > 0.2 && padded < 0.22, "a ball's outline grows a little nearer: {padded}");
        let (window, around) = window(&[([0.0, 1.0, 2.9], 0.2)], &head, &[head.t], view);
        assert!((around - view * CLOSEST).abs() < 1e-5 && close(window.t, head.apply([0.0, 0.0, -CLOSEST])), "a ball around the eye fills the view: {around}");
    }

    #[test]
    fn a_window_squares_to_the_viewer_with_level_rows() {
        let window = facing([0.2, 1.0, -0.3], [0.0, 1.6, 0.2]);
        assert!(close(window.axis(2), normalize(sub([0.0, 1.6, 0.2], window.t))));
        assert!(window.axis(0)[1].abs() < 1e-5);
        assert!(window.axis(1)[1] > 0.0);
    }
}
