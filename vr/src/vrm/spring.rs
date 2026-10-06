use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

use super::{floats, index, number, Model, Node, Pose, Version};

#[derive(Clone, Copy)]
struct Settings {
    stiffness: f32,
    drag: f32,
    gravity: Vec3,
    radius: f32,
}

struct Joint {
    node: usize,
    tail: Vec3,
    settings: Settings,
    groups: Vec<usize>,
    below: Vec<usize>,
}

struct Collider {
    node: usize,
    offset: Vec3,
    tail: Option<Vec3>,
    radius: f32,
}

#[derive(Default)]
pub struct Rig {
    joints: Vec<Joint>,
    colliders: Vec<Collider>,
    groups: Vec<Vec<usize>>,
}

struct Bone {
    node: usize,
    child: Option<usize>,
    settings: Settings,
    groups: Vec<usize>,
}

fn object(value: &Value) -> Option<Vec3> {
    value.is_object().then(|| Vec3::new(number(&value["x"], 0.0), number(&value["y"], 0.0), number(&value["z"], 0.0)))
}

impl Rig {
    /// Reads VRMC_springBone or VRM 0 secondaryAnimation as three-vrm reads them, skipping what it skips.
    pub(super) fn parse(version: Version, extensions: &Value, nodes: &[Node], children: &[Vec<usize>]) -> Rig {
        let empty = Vec::new();
        let list = |value: &Value| value.as_array().unwrap_or(&empty).clone();
        let node = |value: &Value| index(value).filter(|&node| node < nodes.len());
        let mut rig = Rig::default();
        let mut bones = Vec::new();
        match version {
            Version::One => {
                let extension = &extensions["VRMC_springBone"];
                let mut slots = Vec::new();
                for collider in list(&extension["colliders"]) {
                    let shape = &collider["shape"];
                    let parsed = match (shape.get("sphere"), shape.get("capsule"), node(&collider["node"])) {
                        (Some(sphere), _, Some(at)) => Some(Collider { node: at, offset: Vec3::from(floats(&sphere["offset"], [0.0; 3])), tail: None, radius: number(&sphere["radius"], 0.0) }),
                        (None, Some(capsule), Some(at)) => Some(Collider { node: at, offset: Vec3::from(floats(&capsule["offset"], [0.0; 3])), tail: Some(Vec3::from(floats(&capsule["tail"], [0.0; 3]))), radius: number(&capsule["radius"], 0.0) }),
                        _ => None,
                    };
                    slots.push(parsed.map(|collider| {
                        rig.colliders.push(collider);
                        rig.colliders.len() - 1
                    }));
                }
                for group in list(&extension["colliderGroups"]) {
                    rig.groups.push(list(&group["colliders"]).iter().filter_map(|slot| *slots.get(index(slot)?)?).collect());
                }
                for spring in list(&extension["springs"]) {
                    let groups: Vec<usize> = list(&spring["colliderGroups"]).iter().filter_map(index).filter(|&group| group < rig.groups.len()).collect();
                    for pair in list(&spring["joints"]).windows(2) {
                        let joint = &pair[0];
                        let (Some(at), Some(child)) = (node(&joint["node"]), node(&pair[1]["node"])) else { continue };
                        let settings = Settings {
                            stiffness: number(&joint["stiffness"], 1.0),
                            drag: number(&joint["dragForce"], 0.4),
                            gravity: Vec3::from(floats(&joint["gravityDir"], [0.0, -1.0, 0.0])) * number(&joint["gravityPower"], 0.0),
                            radius: number(&joint["hitRadius"], 0.0),
                        };
                        bones.push(Bone { node: at, child: Some(child), settings, groups: groups.clone() });
                    }
                }
            }
            Version::Zero => {
                let extension = &extensions["VRM"]["secondaryAnimation"];
                for group in list(&extension["colliderGroups"]) {
                    let first = rig.colliders.len();
                    if let Some(at) = node(&group["node"]) {
                        for collider in list(&group["colliders"]) {
                            let offset = object(&collider["offset"]).unwrap_or(Vec3::ZERO) * Vec3::new(1.0, 1.0, -1.0);
                            rig.colliders.push(Collider { node: at, offset, tail: None, radius: number(&collider["radius"], 0.0) });
                        }
                    }
                    rig.groups.push((first..rig.colliders.len()).collect());
                }
                for group in list(&extension["boneGroups"]) {
                    let groups: Vec<usize> = list(&group["colliderGroups"]).iter().filter_map(index).filter(|&group| group < rig.groups.len()).collect();
                    let settings = Settings {
                        stiffness: number(&group["stiffiness"], 1.0),
                        drag: number(&group["dragForce"], 0.4),
                        gravity: object(&group["gravityDir"]).unwrap_or(Vec3::NEG_Y) * number(&group["gravityPower"], 0.0),
                        radius: number(&group["hitRadius"], 0.0),
                    };
                    for root in list(&group["bones"]).iter().filter_map(node) {
                        let mut stack = vec![root];
                        while let Some(at) = stack.pop() {
                            bones.push(Bone { node: at, child: children[at].first().copied(), settings, groups: groups.clone() });
                            stack.extend(children[at].iter().rev());
                        }
                    }
                }
            }
        }
        let depth = |mut at: usize| {
            let mut depth = 0;
            while let Some(parent) = nodes[at].parent {
                at = parent;
                depth += 1;
            }
            depth
        };
        for Bone { node, child, settings, groups } in bones {
            let tail = match child {
                Some(child) => nodes[child].translation,
                None => nodes[node].translation.normalize_or_zero() * 0.07,
            };
            if tail.length_squared() == 0.0 {
                continue;
            }
            let mut below = Vec::new();
            let mut frontier = children[node].clone();
            while !frontier.is_empty() {
                below.extend(&frontier);
                frontier = frontier.iter().flat_map(|&node| children[node].iter().copied()).collect();
            }
            rig.joints.push(Joint { node, tail, settings, groups, below });
        }
        rig.joints.sort_by_key(|joint| depth(joint.node));
        rig
    }
}

#[derive(Clone, Copy)]
struct Tail {
    now: Vec3,
    before: Vec3,
}

#[derive(Default)]
pub struct Springs {
    tails: Vec<Tail>,
}

fn scale_of(matrix: &Mat4) -> f32 {
    matrix.x_axis.truncate().length()
}

impl Springs {
    /// Starts the next step from rest, so a jump of the anchor does not fling the hair.
    pub fn reset(&mut self) {
        self.tails.clear();
    }

    pub fn step(&mut self, model: &Model, pose: &mut Pose, model_to_world: Mat4, delta: f32) {
        let rig = &model.rig;
        if rig.joints.is_empty() {
            return;
        }
        let mut worlds: Vec<Mat4> = model.worlds(pose).into_iter().map(|world| model_to_world * world).collect();
        let colliders: Vec<(Vec3, Option<Vec3>, f32)> = rig
            .colliders
            .iter()
            .map(|collider| {
                let world = worlds[collider.node];
                (world.transform_point3(collider.offset), collider.tail.map(|tail| world.transform_point3(tail)), collider.radius * scale_of(&world))
            })
            .collect();
        let fresh = self.tails.len() != rig.joints.len();
        if fresh {
            self.tails = vec![Tail { now: Vec3::ZERO, before: Vec3::ZERO }; rig.joints.len()];
        }
        for (joint, tail) in rig.joints.iter().zip(&mut self.tails) {
            let node = &model.nodes[joint.node];
            let parent = node.parent.map_or(model_to_world, |parent| worlds[parent]);
            let initial = parent * Mat4::from_scale_rotation_translation(node.scale, node.rotation, pose.translations[joint.node]);
            let origin = initial.w_axis.truncate();
            let rest = initial.transform_point3(joint.tail);
            if fresh {
                *tail = Tail { now: rest, before: rest };
            } else if delta > 0.0 {
                let Settings { stiffness, drag, gravity, radius } = joint.settings;
                let length = (rest - origin).length();
                let axis = initial.transform_vector3(joint.tail).normalize();
                let scale = scale_of(&initial);
                let moved = tail.now + (tail.now - tail.before) * (1.0 - drag) + (axis * stiffness + gravity) * (delta * scale);
                let mut next = origin + (moved - origin).normalize_or(axis) * length;
                for &(center, end, reach) in joint.groups.iter().flat_map(|&group| &rig.groups[group]).map(|&collider| &colliders[collider]) {
                    let nearest = match end {
                        Some(end) => {
                            let span = end - center;
                            center + span * ((next - center).dot(span) / span.length_squared().max(f32::MIN_POSITIVE)).clamp(0.0, 1.0)
                        }
                        None => center,
                    };
                    let away = next - nearest;
                    let overlap = radius * scale + reach - away.length();
                    if overlap > 0.0 {
                        next += away.normalize_or_zero() * overlap;
                        next = origin + (next - origin).normalize_or(axis) * length;
                    }
                }
                *tail = Tail { now: next, before: tail.now };
            }
            let toward = initial.inverse().transform_point3(tail.now).normalize_or(joint.tail.normalize());
            pose.rotations[joint.node] = (node.rotation * Quat::from_rotation_arc(joint.tail.normalize(), toward)).normalize();
            worlds[joint.node] = parent * model.local(pose, joint.node);
            for &below in &joint.below {
                worlds[below] = worlds[model.nodes[below].parent.expect("a node below a joint has a parent")] * model.local(pose, below);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::tests::Builder;

    /// A body with a three-segment strand of hair hanging forward from its top, each segment 0.1 long, in VRM 0 form.
    fn strand(group: Value, colliders: Value) -> Model {
        let mut builder = Builder::new(serde_json::json!({ "VRM": { "humanoid": { "humanBones": [{ "bone": "hips", "node": 1 }] }, "secondaryAnimation": { "boneGroups": [group], "colliderGroups": colliders } } }));
        let position = builder.accessor("VEC3", &[-0.1, 0.0, 0.0, 0.1, 0.0, 0.0, 0.1, 1.0, 0.0, -0.1, 1.0, 0.0]);
        let indices = builder.indices(&[0, 1, 2, 0, 2, 3]);
        builder.set(
            "nodes",
            serde_json::json!([
                { "children": [1], "mesh": 0 },
                { "translation": [0.0, 1.0, 0.0], "children": [2] },
                { "translation": [0.0, 0.0, 0.1], "children": [3] },
                { "translation": [0.0, 0.0, 0.1], "children": [4] },
                { "translation": [0.0, 0.0, 0.1] }
            ]),
        );
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position }, "indices": indices }] }]));
        Model::parse(&builder.glb()).unwrap()
    }

    fn hair(stiffness: f32, gravity: f32) -> Model {
        strand(serde_json::json!({ "bones": [2], "stiffiness": stiffness, "dragForce": 0.1, "gravityPower": gravity, "gravityDir": { "x": 0, "y": -1, "z": 0 }, "hitRadius": 0.0, "center": 1 }), serde_json::json!([]))
    }

    fn tip(model: &Model, pose: &Pose, anchor: Mat4) -> Vec3 {
        (anchor * model.worlds(pose)[4]).w_axis.truncate()
    }

    fn run(model: &Model, springs: &mut Springs, anchor: impl Fn(f32) -> Mat4, from: f32, to: f32) -> Vec<(f32, Vec3)> {
        const STEP: f32 = 1.0 / 90.0;
        let mut trace = Vec::new();
        let mut time = from;
        while time < to {
            let mut pose = model.rest();
            springs.step(model, &mut pose, anchor(time), STEP);
            trace.push((time, anchor(time).inverse().transform_point3(tip(model, &pose, anchor(time)))));
            time += STEP;
        }
        trace
    }

    #[test]
    fn a_vrm_0_group_makes_a_joint_of_every_bone_down_to_a_tip_beyond_the_last() {
        let model = hair(1.0, 0.0);
        let joints: Vec<(usize, Vec3)> = model.rig.joints.iter().map(|joint| (joint.node, joint.tail)).collect();
        assert_eq!(joints, [(2, Vec3::new(0.0, 0.0, 0.1)), (3, Vec3::new(0.0, 0.0, 0.1)), (4, Vec3::new(0.0, 0.0, 0.07))]);
        assert_eq!(model.rig.joints[0].below, [3, 4]);
    }

    #[test]
    fn a_spring_entry_naming_a_missing_node_is_skipped_and_the_rest_still_load() {
        let colliders = serde_json::json!([{ "node": 99, "colliders": [{ "radius": 1.0 }] }, { "node": 1, "colliders": [{ "radius": 0.04 }] }]);
        let model = strand(serde_json::json!({ "bones": [99, -1, 2], "colliderGroups": [1] }), colliders);
        assert_eq!(model.rig.joints.len(), 3);
        assert_eq!(model.rig.groups, [vec![], vec![0]], "a skipped collider group keeps its place, so the bone group's index still names its sphere");
        assert_eq!(model.rig.colliders[0].radius, 0.04);
    }

    #[test]
    fn a_vrm_1_spring_joins_each_joint_to_the_next_and_reads_capsules() {
        let mut builder = Builder::new(serde_json::json!({
            "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 } } } },
            "VRMC_springBone": {
                "colliders": [{ "node": 1, "shape": { "capsule": { "offset": [0.0, 0.1, 0.0], "radius": 0.05, "tail": [0.0, 0.3, 0.0] } } }],
                "colliderGroups": [{ "colliders": [0] }],
                "springs": [{ "joints": [{ "node": 2, "stiffness": 0.5 }, { "node": 3 }, { "node": 4 }], "colliderGroups": [0] }]
            }
        }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "children": [1], "mesh": 0 }, { "children": [2] }, { "translation": [0.0, 1.0, 0.0], "children": [3] }, { "translation": [0.0, 0.0, 0.1], "children": [4] }, { "translation": [0.0, 0.0, 0.2] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let joints: Vec<(usize, Vec3, f32)> = model.rig.joints.iter().map(|joint| (joint.node, joint.tail, joint.settings.stiffness)).collect();
        assert_eq!(joints, [(2, Vec3::new(0.0, 0.0, 0.1), 0.5), (3, Vec3::new(0.0, 0.0, 0.2), 1.0)], "the last joint is only a tail");
        let capsule = &model.rig.colliders[0];
        assert_eq!((capsule.offset, capsule.tail, capsule.radius), (Vec3::new(0.0, 0.1, 0.0), Some(Vec3::new(0.0, 0.3, 0.0)), 0.05));
        assert_eq!(model.rig.joints[0].groups, [0]);
    }

    #[test]
    fn carrying_the_figure_swings_the_hair_behind_it_and_it_settles_back() {
        let model = hair(1.0, 0.0);
        let mut springs = Springs::default();
        let scale = Mat4::from_scale(Vec3::splat(0.2));
        let carried = |time: f32| Mat4::from_translation(Vec3::new(0.4 * (time.clamp(0.0, 0.25) / 0.25), 0.0, 0.0)) * scale;
        let still = run(&model, &mut springs, carried, -1.0, 0.0);
        let rest = still.last().unwrap().1;
        assert!(rest.abs_diff_eq(Vec3::new(0.0, 1.0, 0.3), 1e-4), "at rest the strand hangs where the file put it: {rest}");
        let moving = run(&model, &mut springs, carried, 0.0, 4.0);
        let swing = |trace: &[(f32, Vec3)], from: f32, to: f32| trace.iter().filter(|(time, _)| (from..to).contains(time)).map(|(_, tip)| tip.x - rest.x).fold(0.0f32, |most, x| if x.abs() > most.abs() { x } else { most });
        let during = swing(&moving, 0.0, 0.25);
        assert!(during < -0.03, "while the hand moves the figure to +x, the tip trails toward -x: {during}");
        let after = swing(&moving, 0.25, 0.8);
        assert!(after > 0.01, "when the hand stops, the tip swings on past rest: {after}");
        let settled = swing(&moving, 3.5, 4.0);
        assert!(settled.abs() < 0.005, "and settles back to rest: {settled}");
    }

    #[test]
    fn hair_falls_toward_the_ground_whichever_way_the_figure_is_held() {
        let model = hair(0.2, 2.0);
        for (turn, down) in [(Quat::IDENTITY, Vec3::NEG_Y), (Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), Vec3::NEG_X)] {
            let mut springs = Springs::default();
            let anchor = Mat4::from_rotation_translation(turn, Vec3::new(0.3, 1.2, -0.4)) * Mat4::from_scale(Vec3::splat(0.2));
            let trace = run(&model, &mut springs, |_| anchor, 0.0, 6.0);
            let (first, last) = (trace[0].1, trace.last().unwrap().1);
            let fell = (last - first).dot(down);
            assert!(fell > 0.05, "held turned by {turn}, the tip falls toward the world's ground, figure {down} in its own frame: {fell}");
        }
    }

    #[test]
    fn a_reset_puts_the_hair_back_at_rest_wherever_the_anchor_jumped() {
        let model = hair(1.0, 0.0);
        let mut springs = Springs::default();
        run(&model, &mut springs, |_| Mat4::IDENTITY, 0.0, 0.5);
        springs.reset();
        let far = Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0));
        let trace = run(&model, &mut springs, |_| far, 0.0, 0.2);
        assert!(trace.iter().all(|(_, tip)| tip.abs_diff_eq(Vec3::new(0.0, 1.0, 0.3), 1e-4)), "no swing from the jump");
    }

    #[test]
    fn a_collider_keeps_the_strand_out_of_the_body() {
        let hanging = |colliders: Value| {
            let model = strand(serde_json::json!({ "bones": [2], "stiffiness": 0.0, "dragForce": 0.4, "gravityPower": 1.0, "hitRadius": 0.0, "colliderGroups": [0] }), colliders);
            let mut springs = Springs::default();
            run(&model, &mut springs, |_| Mat4::IDENTITY, 0.0, 3.0);
            let mut pose = model.rest();
            springs.step(&model, &mut pose, Mat4::IDENTITY, 1.0 / 90.0);
            model.worlds(&pose)[3].w_axis.truncate()
        };
        let free = hanging(serde_json::json!([{ "node": 1, "colliders": [] }]));
        assert!(free.abs_diff_eq(Vec3::new(0.0, 0.9, 0.1), 2e-3), "without the sphere the strand hangs straight down: {free}");
        let blocked = hanging(serde_json::json!([{ "node": 1, "colliders": [{ "offset": { "x": 0.0, "y": -0.1, "z": -0.13 }, "radius": 0.04 }] }]));
        let sphere = Vec3::new(0.0, 0.9, 0.13);
        assert!((blocked - sphere).length() >= 0.04 - 1e-4, "with it, the strand's middle rests on the sphere, its offset's z turned into glTF's: {blocked}");
        assert!(blocked.y < 0.99, "having fallen onto it: {blocked}");
    }

    #[test]
    fn the_swing_is_the_same_at_any_size_of_figure() {
        let model = hair(1.0, 0.5);
        let carried = |size: f32| move |time: f32| Mat4::from_translation(Vec3::new(2.0 * size * (time.clamp(0.0, 0.25) / 0.25), 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(size));
        let small = run(&model, &mut Springs::default(), carried(0.2), 0.0, 1.0);
        let full = run(&model, &mut Springs::default(), carried(1.0), 0.0, 1.0);
        for ((_, a), (_, b)) in small.iter().zip(&full) {
            assert!(a.abs_diff_eq(*b, 1e-4), "{a} vs {b}");
        }
    }

    #[test]
    fn a_joint_below_a_plain_node_hangs_from_where_its_swung_ancestor_put_it() {
        let mut builder = Builder::new(serde_json::json!({
            "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 } } } },
            "VRMC_springBone": { "springs": [
                { "joints": [{ "node": 2, "stiffness": 0.0, "gravityPower": 1.0 }, { "node": 3 }] },
                { "joints": [{ "node": 4, "stiffness": 0.0, "gravityPower": 1.0 }, { "node": 5 }] }
            ] }
        }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "children": [1], "mesh": 0 }, { "children": [2] }, { "translation": [0.0, 1.0, 0.0], "children": [3] }, { "translation": [0.0, 0.0, 0.1], "children": [4] }, { "translation": [0.0, 0.0, 0.2], "children": [5] }, { "translation": [0.0, 0.0, 0.1] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let mut springs = Springs::default();
        run(&model, &mut springs, |_| Mat4::IDENTITY, 0.0, 3.0);
        let mut pose = model.rest();
        springs.step(&model, &mut pose, Mat4::IDENTITY, 1.0 / 90.0);
        let end = model.worlds(&pose)[5].w_axis.truncate();
        assert!(end.abs_diff_eq(Vec3::new(0.0, 0.6, 0.0), 2e-3), "{end}");
    }

    #[test]
    fn a_turned_bone_hangs_down_too() {
        let mut builder = Builder::new(serde_json::json!({ "VRM": { "humanoid": { "humanBones": [{ "bone": "hips", "node": 1 }] }, "secondaryAnimation": { "boneGroups": [{ "bones": [2], "stiffiness": 0.0, "dragForce": 0.4, "gravityPower": 1.0 }], "colliderGroups": [] } } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let side = std::f32::consts::FRAC_1_SQRT_2;
        builder.set("nodes", serde_json::json!([{ "children": [1], "mesh": 0 }, { "translation": [0.0, 1.0, 0.0], "children": [2] }, { "translation": [0.0, 0.0, 0.1], "rotation": [0.0, side, 0.0, side], "children": [3] }, { "translation": [0.0, 0.0, 0.1], "children": [4] }, { "translation": [0.0, 0.0, 0.1] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let mut springs = Springs::default();
        run(&model, &mut springs, |_| Mat4::IDENTITY, 0.0, 3.0);
        let mut pose = model.rest();
        springs.step(&model, &mut pose, Mat4::IDENTITY, 1.0 / 90.0);
        let end = model.worlds(&pose)[3].w_axis.truncate();
        assert!(end.abs_diff_eq(Vec3::new(0.0, 0.9, 0.1), 2e-3), "{end}");
    }
}
