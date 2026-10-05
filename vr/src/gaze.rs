use glam::{Mat4, Quat, Vec3};

use crate::vrm::{Model, Pose};

/// VRM's look-at settings describe only the eyes, so the neck's comfortable turn is ours.
const NECK: f32 = 0.55;
const HEAD_SHARE: f32 = 0.6;
/// Past this fraction of the reach the turn approaches the reach without meeting it.
const KNEE: f32 = 0.75;
const EASE: f32 = 0.2;

fn settle(angle: f32, reach: f32) -> f32 {
    let knee = KNEE * reach;
    if angle <= knee {
        return angle;
    }
    let soft = reach - knee;
    knee + soft * ((angle - knee) / soft).tanh()
}

/// The eyes' reach toward `wanted` (face frame): the range maps limit yaw and pitch each on its own.
fn eye_reach([sideways, up, down]: [f32; 3], wanted: Vec3) -> f32 {
    let vertical = if wanted.y >= 0.0 { up } else { down };
    let tilt = wanted.x.hypot(wanted.y);
    if tilt < 1e-9 {
        return sideways.min(vertical);
    }
    (sideways * tilt / wanted.x.abs()).min(vertical * tilt / wanted.y.abs())
}

/// Splits the turn toward `wanted` (face frame) into the head's turn and the gaze direction, both in the face frame,
/// along the shortest arc so the turn adds no roll, settling short of what head and eyes together cannot reach.
fn split(wanted: Vec3, eyes: [f32; 3]) -> (Quat, Vec3) {
    let angle = wanted.angle_between(Vec3::Z);
    if angle < 1e-6 {
        return (Quat::IDENTITY, Vec3::Z);
    }
    let eyes = eye_reach(eyes, wanted);
    let reached = settle(angle, NECK + eyes);
    let head = (reached * HEAD_SHARE).max(reached - eyes).min(NECK);
    let arc = Quat::from_rotation_arc(Vec3::Z, wanted);
    let along = |turn: f32| Quat::IDENTITY.slerp(arc, turn / angle);
    (along(head), along(reached) * Vec3::Z)
}

/// The direction the avatar looks, in model space, eased toward the viewer; the head and eyes follow it at once,
/// so the gaze holds steady while the motion sways the head.
#[derive(Default)]
pub struct Gaze {
    toward: Option<Vec3>,
}

impl Gaze {
    /// Turns the head and eyes of `pose`, whose node transforms are `worlds`, toward `target` in model space.
    pub fn look(&mut self, model: &Model, pose: &mut Pose, worlds: &[Mat4], target: Vec3, delta: f32) {
        let Some((origin, face)) = model.face(worlds) else { return };
        let wanted = (target - origin).normalize_or(face * Vec3::Z);
        let amount = 1.0 - (-delta / EASE).exp();
        let toward = self.toward.unwrap_or(face * Vec3::Z).lerp(wanted, amount).normalize_or(wanted);
        self.toward = Some(toward);
        let (head, gaze) = split(face.inverse() * toward, model.eye_reach());
        model.turn_head(pose, worlds, face * head * face.inverse());
        let eyes = head.inverse() * gaze;
        model.turn_eyes(pose, eyes.x.atan2(eyes.z).to_degrees(), eyes.y.atan2(eyes.x.hypot(eyes.z)).to_degrees());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::{tests::Builder, Version};

    const WIDE: [f32; 3] = [1.571; 3];

    fn toward(yaw: f32, pitch: f32) -> Vec3 {
        Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos())
    }

    #[test]
    fn inside_the_reach_the_gaze_meets_the_direction_and_the_head_stays_in_its_range() {
        for (yaw, pitch) in [(0.0, 0.0), (0.4, 0.3), (-0.9, 1.2), (1.5, -0.8), (-1.6, -1.2)] {
            let wanted = toward(yaw, pitch);
            let (head, gaze) = split(wanted, WIDE);
            assert!(gaze.angle_between(wanted) < 1e-4, "{yaw} {pitch}");
            assert!(head.to_axis_angle().1 <= NECK + 1e-5);
        }
    }

    #[test]
    fn past_the_reach_the_turn_settles_short_and_never_strains() {
        let reach = NECK + 0.35;
        let mut last = 0.0;
        for step in 0..=180 {
            let wanted = step as f32 * std::f32::consts::PI / 180.0;
            let (head, gaze) = split(Vec3::new(0.0, wanted.sin(), wanted.cos()), [0.35; 3]);
            let turned = gaze.angle_between(Vec3::Z);
            assert!(turned <= reach + 1e-6 && turned <= wanted + 1e-5, "within the reach: {wanted} → {turned}");
            assert!(turned >= last - 1e-5, "a farther target never turns it less");
            assert!(head.to_axis_angle().1 <= NECK + 1e-5 && turned - head.to_axis_angle().1 <= 0.35 + 1e-5, "neither head nor eyes pass their range");
            if wanted <= KNEE * reach {
                assert!((turned - wanted).abs() < 1e-5, "followed exactly up to the knee");
            }
            last = turned;
        }
        assert!(last > 0.99 * reach, "the farthest target turns it nearly to the reach: {last} of {reach}");
    }

    #[test]
    fn just_past_the_reach_a_farther_target_still_turns_it_farther() {
        let reach = NECK + 0.35;
        let turned = |wanted: f32| split(Vec3::new(0.0, wanted.sin(), wanted.cos()), [0.35; 3]).1.angle_between(Vec3::Z);
        assert!(turned(reach) < reach - 0.03, "the reach is approached, not met: {}", turned(reach));
        assert!(turned(reach + 0.3) > turned(reach) + 0.01, "the turn keeps easing toward the reach past it");
    }

    #[test]
    fn eyes_that_barely_move_leave_the_rest_to_the_head() {
        let (head, gaze) = split(toward(0.0, 0.4), [1.0, 0.05, 1.0]);
        assert!(gaze.angle_between(toward(0.0, 0.4)) < 1e-4, "the target is in reach");
        assert!((head.to_axis_angle().1 - 0.35).abs() < 1e-4, "the head takes all but the eyes' 0.05");
    }

    #[test]
    fn the_eye_reach_is_where_yaw_or_pitch_meets_its_map() {
        let reach = [0.2, 0.6, 0.4];
        assert!((eye_reach(reach, toward(0.3, 0.0)) - 0.2).abs() < 1e-6);
        assert!((eye_reach(reach, toward(0.0, 0.3)) - 0.6).abs() < 1e-6);
        assert!((eye_reach(reach, toward(0.0, -0.3)) - 0.4).abs() < 1e-6);
        let diagonal = eye_reach(reach, Vec3::new(1.0, 1.0, 1.0));
        assert!((diagonal - 0.2 * std::f32::consts::SQRT_2).abs() < 1e-6, "diagonally the sideways map binds first: {diagonal}");
    }

    /// Hips, neck, a head turned in its rest pose, and two eyes; each look-at range map turns an eye bone through
    /// as many degrees as the gaze angle, up to `inputs` = [inner, outer, down, up].
    fn head(version: Version, inputs: [f32; 4], offset_z: f32) -> Model {
        let [inner, outer, down, up] = inputs;
        let look = match version {
            Version::One => serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 }, "neck": { "node": 2 }, "head": { "node": 3 }, "leftEye": { "node": 4 }, "rightEye": { "node": 5 } } },
                "lookAt": { "type": "bone", "offsetFromHeadBone": [0.0, 0.05, offset_z], "rangeMapHorizontalInner": { "inputMaxValue": inner, "outputScale": inner }, "rangeMapHorizontalOuter": { "inputMaxValue": outer, "outputScale": outer }, "rangeMapVerticalDown": { "inputMaxValue": down, "outputScale": down }, "rangeMapVerticalUp": { "inputMaxValue": up, "outputScale": up } } } }),
            Version::Zero => serde_json::json!({ "VRM": { "humanoid": { "humanBones": [{ "bone": "hips", "node": 1 }, { "bone": "neck", "node": 2 }, { "bone": "head", "node": 3 }, { "bone": "leftEye", "node": 4 }, { "bone": "rightEye", "node": 5 }] },
                "firstPerson": { "lookAtTypeName": "Bone", "firstPersonBoneOffset": { "x": 0.0, "y": 0.05, "z": offset_z }, "lookAtHorizontalInner": { "xRange": inner, "yRange": inner }, "lookAtHorizontalOuter": { "xRange": outer, "yRange": outer }, "lookAtVerticalDown": { "xRange": down, "yRange": down }, "lookAtVerticalUp": { "xRange": up, "yRange": up } } } }),
        };
        let mut builder = Builder::new(look);
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 1.5, 0.0]);
        builder.set("nodes", serde_json::json!([
            { "children": [1], "mesh": 0 },
            { "translation": [0.0, 1.0, 0.0], "children": [2] },
            { "translation": [0.0, 0.4, 0.0], "rotation": Quat::from_rotation_z(0.3).to_array(), "children": [3] },
            { "translation": [0.0, 0.1, 0.0], "rotation": Quat::from_rotation_z(-0.1).to_array(), "children": [4, 5] },
            { "translation": [0.03, 0.05, 0.0] },
            { "translation": [-0.03, 0.05, 0.0] },
        ]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        Model::parse(&builder.glb()).unwrap()
    }

    fn rotation(world: &Mat4) -> Quat {
        world.to_scale_rotation_translation().1
    }

    /// Runs the gaze for three seconds at 90 Hz, each frame from the pose `start` gives; returns the last pose.
    fn settled(model: &Model, start: impl Fn() -> Pose, target: Vec3) -> Pose {
        let mut gaze = Gaze::default();
        let mut pose = start();
        for _ in 0..270 {
            pose = start();
            let worlds = model.worlds(&pose);
            gaze.look(model, &mut pose, &worlds, target, 1.0 / 90.0);
        }
        pose
    }

    /// Where a posed bone points, in model space: its turn from the rest pose applied to the figure's facing.
    fn points(model: &Model, pose: &Pose, node: usize) -> Vec3 {
        let (worlds, rest) = (model.worlds(pose), model.worlds(&model.rest()));
        rotation(&worlds[node]) * rotation(&rest[node]).inverse() * model.facing() * Vec3::Z
    }

    #[test]
    fn a_sweep_around_the_head_is_followed_inside_the_reach_and_settles_short_outside() {
        for version in [Version::Zero, Version::One] {
            let model = head(version, [20.0; 4], 0.0);
            let (origin, face) = model.face(&model.worlds(&model.rest())).unwrap();
            let reach = NECK + 20f32.to_radians();
            for degrees in (-170..=170).step_by(10) {
                let angle = (degrees as f32).to_radians();
                let wanted = face * Vec3::new(0.0, angle.sin(), angle.cos());
                let pose = settled(&model, || model.rest(), origin + wanted * 100.0);
                let ahead = face * Vec3::Z;
                for eye in [4, 5] {
                    let looking = points(&model, &pose, eye);
                    let off = looking.angle_between(ahead);
                    if angle.abs() <= KNEE * reach {
                        assert!(looking.angle_between(wanted) < 0.01, "{version:?} at {degrees}°: eye {eye} meets the target");
                    } else {
                        assert!(off < reach && off > KNEE * reach - 0.01, "{version:?} at {degrees}°: eye {eye} settles between the knee and the reach, at {}°", off.to_degrees());
                    }
                    assert!(degrees == 0 || (looking.dot(face * Vec3::Y) > 0.0) == (degrees > 0), "{version:?} at {degrees}°: toward the target's side");
                }
                assert!((points(&model, &pose, 3) - ahead).dot(face * Vec3::X).abs() < 1e-4, "{version:?} at {degrees}°: an upward turn takes no sideways turn");
            }
        }
    }

    #[test]
    fn the_eye_bones_turn_left_and_up_and_the_neck_shares_the_head_turn() {
        for version in [Version::Zero, Version::One] {
            let model = head(version, [90.0; 4], 0.0);
            let (origin, face) = model.face(&model.worlds(&model.rest())).unwrap();
            let wanted = face * toward(0.3, 0.2);
            let pose = settled(&model, || model.rest(), origin + wanted * 100.0);
            let (head_turn, _) = split(toward(0.3, 0.2), model.eye_reach());
            let (worlds, rest) = (model.worlds(&pose), model.worlds(&model.rest()));
            let angle = |at: usize| (rotation(&worlds[at]) * rotation(&rest[at]).inverse()).to_axis_angle().1;
            let whole = head_turn.to_axis_angle().1;
            assert!((angle(2) - whole / 2.0).abs() < 1e-3 && (angle(3) - whole).abs() < 1e-3, "{version:?}: the neck takes half, the head the whole: {} {}", angle(2), angle(3));
            for eye in [4, 5] {
                assert!(points(&model, &pose, eye).angle_between(wanted) < 1e-3, "{version:?}: eye {eye} meets the target");
            }
        }
    }

    #[test]
    fn each_eye_turns_on_its_own_map() {
        let model = head(Version::One, [10.0, 30.0, 20.0, 40.0], 0.0);
        let turned = |yaw: f32, pitch: f32, eye: usize| {
            let mut pose = model.rest();
            model.turn_eyes(&mut pose, yaw, pitch);
            let points = model.facing().inverse() * points(&model, &pose, eye);
            (points.x.atan2(points.z).to_degrees(), points.y.atan2(points.x.hypot(points.z)).to_degrees())
        };
        let close = |(yaw, pitch): (f32, f32), expected: (f32, f32)| (yaw - expected.0).abs() < 0.05 && (pitch - expected.1).abs() < 0.05;
        assert!(close(turned(50.0, 0.0, 4), (30.0, 0.0)), "leftward, the left eye turns outward to its outer limit: {:?}", turned(50.0, 0.0, 4));
        assert!(close(turned(50.0, 0.0, 5), (10.0, 0.0)), "and the right eye inward to its inner limit: {:?}", turned(50.0, 0.0, 5));
        assert!(close(turned(-5.0, -15.0, 4), (-5.0, -15.0)), "rightward and down within the maps: {:?}", turned(-5.0, -15.0, 4));
        assert!(close(turned(0.0, 60.0, 5), (0.0, 40.0)), "up to the up limit: {:?}", turned(0.0, 60.0, 5));
        assert!(close(turned(0.0, -60.0, 5), (0.0, -20.0)), "down to the down limit: {:?}", turned(0.0, -60.0, 5));
    }

    #[test]
    fn the_eye_reach_reads_the_maps_and_eyes_that_cannot_move_reach_nothing() {
        let model = head(Version::One, [10.0, 30.0, 20.0, 40.0], 0.0);
        let [sideways, up, down] = model.eye_reach();
        assert!((sideways - 10f32.to_radians()).abs() < 1e-6 && (up - 40f32.to_radians()).abs() < 1e-6 && (down - 20f32.to_radians()).abs() < 1e-6);
        let mut builder = Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": { "head": { "node": 0 } } }, "lookAt": { "type": "bone" } } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        assert_eq!(Model::parse(&builder.glb()).unwrap().eye_reach(), [0.0; 3], "no eye bones");
        let mut builder = Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": { "head": { "node": 0 }, "leftEye": { "node": 0 } } } } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        assert_eq!(Model::parse(&builder.glb()).unwrap().eye_reach(), [0.0; 3], "no look-at settings");
    }

    #[test]
    fn the_eyes_look_from_the_offset_in_the_head_bone_frame() {
        for version in [Version::Zero, Version::One] {
            let model = head(version, [90.0; 4], 0.1);
            let worlds = model.worlds(&model.rest());
            let (origin, _) = model.face(&worlds).unwrap();
            let ahead = if version == Version::Zero { -0.1 } else { 0.1 };
            let expected = worlds[3].transform_point3(Vec3::new(0.0, 0.05, ahead));
            assert!(origin.abs_diff_eq(expected, 1e-5), "{version:?}: {origin} vs {expected}");
        }
    }

    #[test]
    fn a_head_the_pose_tilts_looks_up_by_tipping_back_and_its_eyes_meet_the_target() {
        for version in [Version::Zero, Version::One] {
            let model = head(version, [90.0; 4], 0.0);
            let tilted = || {
                let mut pose = model.rest();
                model.turn_head(&mut pose, &model.worlds(&model.rest()), Quat::from_rotation_z(0.4));
                pose
            };
            let tilted_worlds = model.worlds(&tilted());
            let (origin, face) = model.face(&tilted_worlds).unwrap();
            let ahead = face * Vec3::Z;
            let wanted = (ahead + Vec3::Y * 1.2).normalize();
            let pose = settled(&model, tilted, origin + wanted * 100.0);
            let (axis, angle) = (rotation(&model.worlds(&pose)[3]) * rotation(&tilted_worlds[3]).inverse()).to_axis_angle();
            assert!(angle > 0.3 && axis.dot(ahead.cross(Vec3::Y).normalize()).abs() > 0.999, "{version:?}: tips back about the level side axis: {axis} {angle}");
            for eye in [4, 5] {
                assert!(points(&model, &pose, eye).angle_between(wanted) < 1e-3, "{version:?}: eye {eye} meets the target");
            }
        }
    }

    #[test]
    fn the_gaze_eases_in_and_then_holds_while_the_motion_sways_the_head() {
        let model = head(Version::One, [90.0; 4], 0.0);
        let (origin, face) = model.face(&model.worlds(&model.rest())).unwrap();
        let wanted = face * toward(0.0, 0.5);
        let target = origin + wanted * 100.0;
        let mut gaze = Gaze::default();
        let mut pose = model.rest();
        gaze.look(&model, &mut pose, &model.worlds(&model.rest()), target, 1.0 / 90.0);
        let first = points(&model, &pose, 4).angle_between(face * Vec3::Z);
        assert!(first > 0.0 && first < 0.05, "one frame moves only a little: {first}");
        for frame in 0..400 {
            let swayed = || {
                let mut pose = model.rest();
                model.turn_head(&mut pose, &model.worlds(&model.rest()), Quat::from_rotation_y(0.1 * (frame as f32 * 0.05).sin()));
                pose
            };
            pose = swayed();
            let worlds = model.worlds(&pose);
            gaze.look(&model, &mut pose, &worlds, target, 1.0 / 90.0);
            if frame >= 270 {
                assert!(points(&model, &pose, 4).angle_between(wanted) < 1e-3, "frame {frame}: the eyes stay on the target as the head sways");
            }
        }
    }

    #[test]
    fn an_expression_look_at_weighs_the_look_expressions_on_their_maps() {
        let groups: Vec<serde_json::Value> = ["lookup", "lookdown", "lookleft", "lookright"].iter().enumerate().map(|(at, name)| serde_json::json!({ "presetName": name, "binds": [{ "mesh": 0, "index": at, "weight": 100 }] })).collect();
        let mut builder = Builder::new(serde_json::json!({ "VRM": { "humanoid": { "humanBones": [{ "bone": "head", "node": 0 }] },
            "firstPerson": { "lookAtTypeName": "BlendShape", "lookAtHorizontalInner": { "xRange": 10.0, "yRange": 0.2 }, "lookAtHorizontalOuter": { "xRange": 40.0, "yRange": 0.6 }, "lookAtVerticalUp": { "xRange": 40.0, "yRange": 0.8 } },
            "blendShapeMaster": { "blendShapeGroups": groups } } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let shift = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position }, "targets": [{ "POSITION": shift }, { "POSITION": shift }, { "POSITION": shift }, { "POSITION": shift }] }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        assert!((model.eye_reach()[0] - 40f32.to_radians()).abs() < 1e-6, "only the outer map moves expression eyes sideways");
        let mut pose = model.rest();
        model.turn_eyes(&mut pose, 20.0, 20.0);
        assert_eq!(pose.weights[0], vec![0.4, 0.0, 0.3, 0.0], "half the range weighs lookUp and lookLeft at half their scales, lookLeft on the outer map");
        let mut pose = model.rest();
        model.turn_eyes(&mut pose, -20.0, 0.0);
        assert_eq!(pose.weights[0], vec![0.0, 0.0, 0.0, 0.3], "lookRight takes the outer map too, as three-vrm's applier does");
    }
}
