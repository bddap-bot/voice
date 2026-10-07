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
    fn from_axes(x: Vec3, y: Vec3, z: Vec3, t: Vec3) -> Pose {
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
pub const SETTLE: Duration = Duration::from_secs(1);

/// Where the avatar stands: on the controller, riding it a moment longer after a set-down press, or left in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    Riding,
    Settling(Instant),
    Stuck(Pose),
}

impl Anchor {
    /// Riding, a press sets it down after `SETTLE`; otherwise it returns to the controller.
    pub fn toggle(&mut self, now: Instant) {
        *self = match self {
            Anchor::Riding => Anchor::Settling(now + SETTLE),
            _ => Anchor::Riding,
        };
    }

    /// The avatar's frame this frame, staying at the pose it rode to once `SETTLE` has passed.
    pub fn stand(&mut self, now: Instant, hand: &Pose) -> Pose {
        match *self {
            Anchor::Stuck(pose) => pose,
            Anchor::Settling(until) if now >= until => {
                let pose = on_controller(hand);
                *self = Anchor::Stuck(pose);
                pose
            }
            _ => on_controller(hand),
        }
    }
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
        assert_eq!(anchor.stand(at(0.0), &hand(0.0)), on_controller(&hand(0.0)));
        anchor.toggle(at(0.0));
        let late = SETTLE.as_secs_f32() - 0.01;
        assert_eq!(anchor.stand(at(late), &hand(late)), on_controller(&hand(late)), "still riding just before it settles");
        let settled = SETTLE.as_secs_f32();
        let left = anchor.stand(at(settled), &hand(settled));
        assert_eq!(left, on_controller(&hand(settled)), "stays where it rode to");
        for seconds in [1.5, 3.0, 9.0] {
            assert_eq!(anchor.stand(at(seconds), &hand(seconds)), left, "held while the controller moves");
        }
        anchor.toggle(at(10.0));
        assert_eq!(anchor, Anchor::Riding);
        assert_eq!(anchor.stand(at(10.0), &hand(10.0)), on_controller(&hand(10.0)), "back on the controller");
        anchor.toggle(at(11.0));
        anchor.toggle(at(11.5));
        assert_eq!(anchor.stand(at(13.0), &hand(13.0)), on_controller(&hand(13.0)), "a second press while settling keeps it riding");
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

    #[test]
    fn a_window_squares_to_the_viewer_with_level_rows() {
        let window = facing([0.2, 1.0, -0.3], [0.0, 1.6, 0.2]);
        assert!(close(window.axis(2), normalize(sub([0.0, 1.6, 0.2], window.t))));
        assert!(window.axis(0)[1].abs() < 1e-5);
        assert!(window.axis(1)[1] > 0.0);
    }
}
