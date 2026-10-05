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
}

const UP: Vec3 = [0.0, 1.0, 0.0];

fn upright_facing(at: Vec3, viewer: Vec3) -> Pose {
    let mut toward = sub(viewer, at);
    toward[1] = 0.0;
    let z = if length(toward) < 1e-3 { [0.0, 0.0, 1.0] } else { normalize(toward) };
    Pose::from_axes(cross(UP, z), UP, z, at)
}

const LIFT: f32 = 0.05;

pub fn above_hand(hand: &Pose, head: &Pose, above_feet: f32) -> Pose {
    upright_facing(add(hand.t, [0.0, LIFT + above_feet, 0.0]), head.t)
}

/// Toward the elbow from the controller's origin, in the controller's frame.
const WRIST: Vec3 = [0.0, 0.0, 0.08];
/// The right controller's touch point ahead of its origin, in its frame.
pub const TIP: Vec3 = [0.0, 0.0, -0.05];

/// Hanging below the left wrist, its face turned to the eyes.
pub fn below_wrist(hand: &Pose, head: &Pose, height: f32) -> Pose {
    let at = sub(hand.apply(WRIST), [0.0, height / 2.0 + LIFT, 0.0]);
    let toward = sub(head.t, at);
    let z = if length(toward) < 1e-3 { [0.0, 0.0, 1.0] } else { normalize(toward) };
    let side = cross(UP, z);
    let x = if length(side) < 1e-3 { [1.0, 0.0, 0.0] } else { normalize(side) };
    Pose::from_axes(x, cross(z, x), z, at)
}

/// Where the right controller's touch point is in the board's frame.
pub fn local_tip(board: &Pose, right: &Pose) -> Vec3 {
    board.inverse().apply(right.apply(TIP))
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

    #[test]
    fn below_the_wrist_the_board_hangs_level_and_faces_the_eyes() {
        let hand = turned(0.7, [0.2, 1.0, -0.3]);
        let head = turned(0.0, [0.0, 1.6, 0.2]);
        let board = below_wrist(&hand, &head, 0.2);
        assert!(board.t[1] < hand.t[1] - 0.1, "below the hand");
        assert!(close(board.axis(2), normalize(sub(head.t, board.t))), "faces the eyes");
        assert!(board.axis(0)[1].abs() < 1e-5, "its rows stay level");
        assert!(board.axis(1)[1] > 0.0, "upright");
    }
}
