use std::collections::HashSet;

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

use super::{floats, index, number, Model, Node, Pose, Version};

#[derive(Clone, Copy)]
struct Settings {
    stiffness: f32,
    drag: f32,
    gravity: Vec3,
    radius: f32,
    limit: Option<Limit>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Bound {
    /// Within `x` of the axis every way round.
    Angle,
    /// Within `x` of the axis, in the plane across the limit's own X.
    Hinge,
    /// Within `x` round from the axis toward the limit's Z, and `z` over toward its X, each on its own as VRChat clamps them.
    Polar,
}

/// How far a joint may swing from its rest direction, as a VRChat PhysBone's limit: angles in radians about the axis `rotation` turns the rest direction and the joint's X to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Limit {
    bound: Bound,
    x: f32,
    z: f32,
    rotation: Quat,
}

impl Limit {
    /// A joint's extras "limit": its "shape" (angle, hinge or polar), "x" and "z" in degrees, and its "rotation" quaternion in the joint's own axes.
    fn read(extras: &Value) -> Option<Limit> {
        let limit = &extras["limit"];
        let bound = match limit["shape"].as_str()? {
            "angle" => Bound::Angle,
            "hinge" => Bound::Hinge,
            "polar" => Bound::Polar,
            _ => return None,
        };
        let [x, y, z, w] = floats(&limit["rotation"], [0.0, 0.0, 0.0, 1.0]);
        let rotation = Quat::from_xyzw(x, y, z, w);
        let rotation = if rotation.length_squared() > 0.0 { rotation.normalize() } else { Quat::IDENTITY };
        Some(Limit { bound, x: number(&limit["x"], 180.0).to_radians(), z: number(&limit["z"], 180.0).to_radians(), rotation })
    }

    /// `toward`, a direction in the joint's rest axes, brought back inside the limit about `rest`.
    fn keep(&self, rest: Vec3, toward: Vec3) -> Vec3 {
        let up = self.rotation * rest.normalize();
        let side = (self.rotation * Vec3::X).reject_from(up).try_normalize().unwrap_or_else(|| up.any_orthonormal_vector());
        let front = side.cross(up);
        let (mut x, y, z) = (toward.dot(side), toward.dot(up), toward.dot(front));
        if self.bound == Bound::Hinge {
            x = 0.0;
        }
        if self.bound == Bound::Polar {
            let toward = Vec3::new(x, y, z).normalize_or(Vec3::Y);
            let around = toward.z.atan2(toward.y).clamp(-self.x, self.x);
            let over = toward.x.clamp(-1.0, 1.0).asin().clamp(-self.z, self.z);
            return up * over.cos() * around.cos() + side * over.sin() + front * over.cos() * around.sin();
        }
        let across = Vec3::new(x, 0.0, z);
        let swing = across.length().atan2(y);
        if swing <= self.x && self.bound != Bound::Hinge {
            return toward;
        }
        let swing = swing.min(self.x);
        let across = across.try_normalize().unwrap_or(Vec3::ZERO);
        up * swing.cos() + (side * across.x + front * across.z) * swing.sin()
    }
}

/// A VRChat PhysBone's settings at one bone, as its joint's extras "physbone" carry them, so the joint swings by the PhysBone's own step.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PhysBone {
    /// Version 1.0's step, which restores the bone's length by pull and gravity's push; 1.1 bends the target toward the ground instead.
    first: bool,
    advanced: bool,
    pull: f32,
    spring: f32,
    stiffness: f32,
    gravity: f32,
    falloff: f32,
    immobile: f32,
    /// Immobile cancels only the avatar's travel, not every motion of the PhysBone root's parent.
    travel: bool,
    /// The shortest and longest the bone may grow, as fractions of its rest length.
    squish: f32,
    stretch: f32,
    stretch_motion: f32,
    /// At the bone's start and its end, in the model's units.
    radius: [f32; 2],
    /// How many transforms down from the PhysBone's root the bone is.
    depth: usize,
    /// Its tail is the PhysBone's endpoint, no transform in VRChat, so its squish and stretch never show.
    endpoint: bool,
}

impl PhysBone {
    fn read(extras: &Value) -> Option<PhysBone> {
        let bone = &extras["physbone"];
        let n = |key: &str, fallback: f32| number(&bone[key], fallback);
        let [begin, end] = floats(&bone["radius"], [0.0; 2]);
        bone.is_object().then(|| PhysBone {
            first: bone["version"] == "1.0",
            advanced: bone["integration"] == "advanced",
            pull: n("pull", 0.2),
            spring: n("spring", 0.2),
            stiffness: n("stiffness", 0.2),
            gravity: n("gravity", 0.0),
            falloff: n("gravityFalloff", 0.0),
            immobile: n("immobile", 0.0),
            travel: bone["immobileType"] == "world",
            squish: 1.0 - n("maxSquish", 0.0),
            stretch: 1.0 + n("maxStretch", 0.0),
            stretch_motion: n("stretchMotion", 0.0),
            radius: [begin, end],
            depth: index(&bone["depth"]).unwrap_or(0),
            endpoint: bone["endpoint"].as_bool().unwrap_or(false),
        })
    }
}

struct Joint {
    node: usize,
    tail: Vec3,
    /// The node `tail` reaches, which a PhysBone moves in or out as the bone squishes and stretches.
    child: Option<usize>,
    settings: Settings,
    physbone: Option<PhysBone>,
    spring: usize,
    groups: Vec<usize>,
    /// Whether another's hand pushes it.
    others: bool,
    below: Vec<usize>,
}

/// One PhysBone chain's joints, root first, and the node its PhysBone's root hangs from.
struct Chain {
    joints: Vec<usize>,
    anchor: usize,
}

/// What a step swings in turn, parents before children.
enum Unit {
    Joint(usize),
    Chain(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// A sphere, or with a tail a capsule, keeping joints outside it or, `inside`, within it.
    Round { tail: Option<Vec3>, radius: f32, inside: bool },
    /// Joints stay on the side its normal points to.
    Plane { normal: Vec3 },
}

struct Collider {
    /// Its place in the file's list, which the expressions' switched colliders name.
    source: usize,
    node: usize,
    offset: Vec3,
    shape: Shape,
}

/// What the avatar's menu leaves off: the joints of its switched-off springs, held at the pose's rotation, and the file indices of its switched-off colliders.
#[derive(Debug, Default, PartialEq)]
pub struct Idle {
    pub joints: HashSet<usize>,
    pub colliders: HashSet<usize>,
}

enum Placed {
    Round { center: Vec3, end: Option<Vec3>, reach: f32, inside: bool },
    Plane { point: Vec3, normal: Vec3 },
}

impl Placed {
    fn push(&self, at: Vec3, radius: f32) -> Vec3 {
        match *self {
            Placed::Round { center, end, reach, inside } => {
                let nearest = match end {
                    Some(end) => {
                        let span = end - center;
                        center + span * ((at - center).dot(span) / span.length_squared().max(f32::MIN_POSITIVE)).clamp(0.0, 1.0)
                    }
                    None => center,
                };
                let away = at - nearest;
                let distance = away.length();
                let depth = if inside { distance + radius - reach } else { reach + radius - distance };
                if depth <= 0.0 {
                    return at;
                }
                at + away.normalize_or_zero() * if inside { -depth } else { depth }
            }
            Placed::Plane { point, normal } => at - normal * ((at - point).dot(normal) - radius).min(0.0),
        }
    }
}

#[derive(Default)]
pub struct Rig {
    joints: Vec<Joint>,
    colliders: Vec<Collider>,
    groups: Vec<Vec<usize>>,
    chains: Vec<Chain>,
    units: Vec<Unit>,
}

struct Bone {
    node: usize,
    child: Option<usize>,
    settings: Settings,
    physbone: Option<PhysBone>,
    spring: usize,
    groups: Vec<usize>,
    others: bool,
}

/// A hand that is not the avatar's own, as a sphere in world space.
#[derive(Clone, Copy, Debug)]
pub struct Hand {
    pub center: Vec3,
    pub radius: f32,
}

/// Whether a spring's (or VRM 0 bone group's) extras let others' hands push it: its "collide" lists "wearer" and "others", as a VRChat PhysBone's allowCollision does, and a spring without the list is everyone's to touch.
fn pushed_by_others(source: &Value) -> bool {
    source["extras"]["collide"].as_array().is_none_or(|who| who.iter().any(|hand| hand == "others"))
}

fn object(value: &Value) -> Option<Vec3> {
    value.is_object().then(|| Vec3::new(number(&value["x"], 0.0), number(&value["y"], 0.0), number(&value["z"], 0.0)))
}

impl Rig {
    /// Reads VRMC_springBone, with VRMC_springBone_extended_collider's inside and plane colliders, or VRM 0 secondaryAnimation as three-vrm reads them, skipping what it skips.
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
                for (source, collider) in list(&extension["colliders"]).iter().enumerate() {
                    let extended = &collider["extensions"]["VRMC_springBone_extended_collider"]["shape"];
                    let shape = if extended.is_object() { extended } else { &collider["shape"] };
                    let offset = |geometry: &Value| Vec3::from(floats(&geometry["offset"], [0.0; 3]));
                    let round = |geometry: &Value, tail: Option<Vec3>| Shape::Round { tail, radius: number(&geometry["radius"], 0.0), inside: geometry["inside"].as_bool().unwrap_or(false) };
                    let parsed = node(&collider["node"]).and_then(|at| match (shape.get("sphere"), shape.get("capsule"), shape.get("plane")) {
                        (Some(sphere), _, _) => Some(Collider { source, node: at, offset: offset(sphere), shape: round(sphere, None) }),
                        (None, Some(capsule), _) => Some(Collider { source, node: at, offset: offset(capsule), shape: round(capsule, Some(Vec3::from(floats(&capsule["tail"], [0.0; 3])))) }),
                        (None, None, Some(plane)) => Some(Collider { source, node: at, offset: offset(plane), shape: Shape::Plane { normal: Vec3::from(floats(&plane["normal"], [0.0, 0.0, 1.0])).normalize_or(Vec3::Z) } }),
                        _ => None,
                    });
                    slots.push(parsed.map(|collider| {
                        rig.colliders.push(collider);
                        rig.colliders.len() - 1
                    }));
                }
                for group in list(&extension["colliderGroups"]) {
                    rig.groups.push(list(&group["colliders"]).iter().filter_map(|slot| *slots.get(index(slot)?)?).collect());
                }
                for (index, spring) in list(&extension["springs"]).iter().enumerate() {
                    let groups: Vec<usize> = list(&spring["colliderGroups"]).iter().filter_map(super::index).filter(|&group| group < rig.groups.len()).collect();
                    let others = pushed_by_others(spring);
                    for pair in list(&spring["joints"]).windows(2) {
                        let joint = &pair[0];
                        let (Some(at), Some(child)) = (node(&joint["node"]), node(&pair[1]["node"])) else { continue };
                        let settings = Settings {
                            stiffness: number(&joint["stiffness"], 1.0),
                            drag: number(&joint["dragForce"], 0.4),
                            gravity: Vec3::from(floats(&joint["gravityDir"], [0.0, -1.0, 0.0])) * number(&joint["gravityPower"], 0.0),
                            radius: number(&joint["hitRadius"], 0.0),
                            limit: Limit::read(&joint["extras"]),
                        };
                        bones.push(Bone { node: at, child: Some(child), settings, physbone: PhysBone::read(&joint["extras"]), spring: index, groups: groups.clone(), others });
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
                            rig.colliders.push(Collider { source: rig.colliders.len(), node: at, offset, shape: Shape::Round { tail: None, radius: number(&collider["radius"], 0.0), inside: false } });
                        }
                    }
                    rig.groups.push((first..rig.colliders.len()).collect());
                }
                for group in list(&extension["boneGroups"]) {
                    let groups: Vec<usize> = list(&group["colliderGroups"]).iter().filter_map(index).filter(|&group| group < rig.groups.len()).collect();
                    let others = pushed_by_others(&group);
                    let settings = Settings {
                        stiffness: number(&group["stiffiness"], 1.0),
                        drag: number(&group["dragForce"], 0.4),
                        gravity: object(&group["gravityDir"]).unwrap_or(Vec3::NEG_Y) * number(&group["gravityPower"], 0.0),
                        radius: number(&group["hitRadius"], 0.0),
                        limit: None,
                    };
                    for root in list(&group["bones"]).iter().filter_map(node) {
                        let mut stack = vec![root];
                        while let Some(at) = stack.pop() {
                            bones.push(Bone { node: at, child: children[at].first().copied(), settings, physbone: None, spring: 0, groups: groups.clone(), others });
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
        for Bone { node, child, settings, physbone, spring, groups, others } in bones {
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
            rig.joints.push(Joint { node, tail, child, settings, physbone, spring, groups, others, below });
        }
        rig.joints.sort_by_key(|joint| depth(joint.node));
        for (at, joint) in rig.joints.iter().enumerate() {
            let Some(physbone) = joint.physbone else {
                rig.units.push(Unit::Joint(at));
                continue;
            };
            if let Some(chain) = rig.chains.iter_mut().find(|chain| rig.joints[chain.joints[0]].spring == joint.spring) {
                chain.joints.push(at);
                continue;
            }
            let mut root = joint.node;
            for _ in 0..physbone.depth {
                root = nodes[root].parent.unwrap_or(root);
            }
            let Some(anchor) = nodes[root].parent else { continue };
            rig.units.push(Unit::Chain(rig.chains.len()));
            rig.chains.push(Chain { joints: vec![at], anchor });
        }
        rig
    }
}

impl Model {
    pub fn spring_motion(&self, worlds: &[Mat4]) -> Vec<(usize, String, Quat, Vec3)> {
        self.rig.joints.iter().map(|joint| {
            let node = &self.nodes[joint.node];
            let parent = node.parent.map_or(Mat4::IDENTITY, |parent| worlds[parent]);
            let local = parent.inverse() * worlds[joint.node];
            (joint.node, node.name.clone().unwrap_or_default(), local.to_scale_rotation_translation().1, worlds[joint.node].w_axis.truncate())
        }).collect()
    }

    pub fn spring_joints(&self) -> usize {
        self.rig.joints.len()
    }
}

#[derive(Clone, Copy, Default)]
struct Tail {
    rotation: Quat,
    now: Vec3,
    before: Vec3,
    /// `now` in the joint's rest frame, so a step can tell the tail's own motion from the motion its parent carries it through.
    held: Vec3,
    /// A PhysBone's direction and velocity at its last whole step, beside the motion of its carrying frame it felt then.
    vector: Vec3,
    velocity: Vec3,
    felt: Vec3,
    /// A PhysBone's tail in the immobile frame at its last whole step.
    anchored: Vec3,
    length: f32,
    /// The world's down in the bone's rest axes, which gravity falloff weighs the bone's turn against.
    down: Vec3,
}

#[derive(Default)]
pub struct Springs {
    tails: Vec<Tail>,
    /// Time not yet stepped: PhysBones step at 60 Hz whatever the frame rate.
    owed: f32,
    /// Each chain's anchor at its last whole step, and the figure's place then.
    anchors: Vec<(Vec3, Quat)>,
    place: Vec3,
}

const PHYSBONE_STEP: f32 = 1.0 / 60.0;

fn scale_of(matrix: &Mat4) -> f32 {
    matrix.x_axis.truncate().length()
}

fn placement(matrix: &Mat4) -> (Vec3, Quat) {
    let (_, rotation, translation) = matrix.to_scale_rotation_translation();
    (translation, rotation)
}

/// `vector` lengthened or shortened into `shortest..=longest`, or `fallback` at the shortest if it has none.
fn clamp_length(vector: Vec3, shortest: f32, longest: f32, fallback: Vec3) -> Vec3 {
    let shortest = shortest.max(1e-5);
    let length = vector.length();
    if length <= f32::EPSILON {
        return fallback.normalize_or_zero() * shortest;
    }
    vector * (length.clamp(shortest, longest.max(shortest)) / length)
}

/// A PhysBone bone being stepped: its start, its tail, its radius at each, and the lengths it may take.
struct Pressed {
    begin: Vec3,
    end: Vec3,
    radius: [f32; 2],
    shortest: f32,
    longest: f32,
    fallback: Vec3,
}

impl Placed {
    /// Moves the bone's tail out of the shape, or inside it, as VRChat does: a bone with a radius at its start is a tapered capsule from there.
    fn press(&self, bone: &mut Pressed) -> bool {
        let Pressed { begin, end, radius, .. } = *bone;
        let moved = match *self {
            Placed::Round { center, end: far, reach, inside } => {
                let nearest = match far {
                    Some(far) => {
                        let span = far - center;
                        center + span * ((end - center).dot(span) / span.length_squared().max(f32::MIN_POSITIVE)).clamp(0.0, 1.0)
                    }
                    None => center,
                };
                if inside {
                    let depth = (end - nearest).length() + radius[1] - reach;
                    (depth > 0.0).then(|| end + (nearest - end).normalize_or_zero() * depth)
                } else if radius[1] > 0.0 {
                    let (mut at, mut round) = (end, radius[1]);
                    let span = end - begin;
                    if radius[0] > 0.0 && span.length_squared() > 0.0 {
                        let clear = 1.0 - ((span.length() - radius[0]) / span.length()).max(0.0);
                        let along = ((nearest - begin).dot(span) / span.length_squared()).clamp(0.0, 1.0).max(clear);
                        at = begin.lerp(end, along);
                        round = radius[0] + (radius[1] - radius[0]) * along;
                    }
                    let gap = (at - nearest).length() - (round + reach);
                    (gap < 0.0).then(|| end + (nearest - at).normalize_or_zero() * gap)
                } else {
                    None
                }
            }
            Placed::Plane { point, normal } => {
                let depth = normal.dot(point - end) + radius[1];
                (depth > 0.0).then(|| end + normal * depth)
            }
        };
        let Some(moved) = moved else { return false };
        bone.end = begin + clamp_length(moved - begin, bone.shortest, bone.longest, bone.fallback);
        true
    }
}

impl Springs {
    /// Starts the next step from rest, so a jump of the anchor does not fling the hair.
    pub fn reset(&mut self) {
        self.tails.clear();
    }

    pub fn restore(&self, model: &Model, pose: &mut Pose, idle: &Idle) {
        if !model.constraints.is_empty() {
            for (joint, tail) in model.rig.joints.iter().zip(&self.tails).filter(|(joint, _)| !idle.joints.contains(&joint.node)) {
                pose.rotations[joint.node] = tail.rotation;
                if let (Some(bone), Some(child)) = (joint.physbone, joint.child) {
                    if !bone.endpoint {
                        pose.translations[child] = joint.tail.normalize() * tail.length;
                    }
                }
            }
        }
    }

    /// Swings each joint but the `idle` ones, which keep the pose's rotation and carry their tails along so they set off from it, against every collider but the `idle` ones.
    pub fn step(&mut self, model: &Model, pose: &mut Pose, model_to_world: Mat4, hands: &[Hand], delta: f32, idle: &Idle) {
        let rig = &model.rig;
        if rig.joints.is_empty() {
            return;
        }
        let mut worlds: Vec<Mat4> = model.worlds(pose).into_iter().map(|world| model_to_world * world).collect();
        let colliders: Vec<Placed> = rig
            .colliders
            .iter()
            .map(|collider| {
                let world = worlds[collider.node];
                let at = world.transform_point3(collider.offset);
                match collider.shape {
                    Shape::Round { tail, radius, inside } => Placed::Round { center: at, end: tail.map(|tail| world.transform_point3(tail)), reach: radius * scale_of(&world), inside },
                    Shape::Plane { normal } => Placed::Plane { point: at, normal: world.transform_vector3(normal).normalize_or(Vec3::Y) },
                }
            })
            .collect();
        let hands: Vec<Placed> = hands.iter().map(|hand| Placed::Round { center: hand.center, end: None, reach: hand.radius, inside: false }).collect();
        let fresh = self.tails.len() != rig.joints.len();
        let place = model_to_world.w_axis.truncate();
        if fresh {
            let rest = model.worlds(&model.rest());
            self.tails = rig.joints.iter().map(|joint| Tail { down: rest[joint.node].inverse().transform_vector3(Vec3::NEG_Y).normalize_or(Vec3::NEG_Y), length: joint.tail.length(), ..Tail::default() }).collect();
            self.anchors = rig.chains.iter().map(|chain| placement(&worlds[chain.anchor])).collect();
            self.place = place;
            self.owed = 0.0;
        }
        self.owed += delta.max(0.0);
        // Divided, not multiplied, as VRChat does: a frame of exactly 1/60 s must make exactly one step, not one and a sliver.
        let executions = (self.owed / PHYSBONE_STEP).min(6.0);
        if executions >= 1.0 {
            self.owed = executions.fract() * PHYSBONE_STEP;
        }
        for unit in &rig.units {
            let at = match *unit {
                Unit::Joint(at) => at,
                Unit::Chain(chain) => {
                    self.chain(model, pose, &mut worlds, chain, (place, executions), [&colliders, &hands], idle, fresh);
                    continue;
                }
            };
            let joint = &rig.joints[at];
            let tail = &mut self.tails[at];
            let node = &model.nodes[joint.node];
            let parent = node.parent.map_or(model_to_world, |parent| worlds[parent]);
            let initial = parent * Mat4::from_scale_rotation_translation(node.scale, node.rotation, pose.translations[joint.node]);
            let origin = initial.w_axis.truncate();
            let rest = initial.transform_point3(joint.tail);
            let still = idle.joints.contains(&joint.node);
            if fresh {
                *tail = Tail { now: rest, before: rest, held: joint.tail, rotation: node.rotation, ..*tail };
            }
            if still {
                let now = (parent * model.local(pose, joint.node)).transform_point3(joint.tail);
                *tail = Tail { now, before: tail.now, rotation: pose.rotations[joint.node], ..*tail };
            } else if !fresh && delta > 0.0 {
                let Settings { stiffness, drag, gravity, radius, limit } = joint.settings;
                let length = (rest - origin).length();
                let axis = initial.transform_vector3(joint.tail).normalize();
                let scale = scale_of(&initial);
                // Drag slows the tail only relative to its carrying frame: steady motion of the figure leaves it at rest, and only acceleration swings it.
                let carried = initial.transform_point3(tail.held);
                let moved = carried + (tail.now - tail.before - (carried - tail.now)) * (1.0 - drag) + (axis * stiffness + gravity) * (delta * scale);
                let mut next = origin + (moved - origin).normalize_or(axis) * length;
                let touching = if joint.others { &hands[..] } else { &[] };
                let running = joint.groups.iter().flat_map(|&group| &rig.groups[group]).filter(|&&collider| !idle.colliders.contains(&rig.colliders[collider].source));
                for placed in running.map(|&collider| &colliders[collider]).chain(touching) {
                    let pushed = placed.push(next, radius * scale);
                    if pushed != next {
                        next = origin + (pushed - origin).normalize_or(axis) * length;
                    }
                }
                if let Some(limit) = limit {
                    let rest = initial.inverse();
                    next = origin + initial.transform_vector3(limit.keep(joint.tail, rest.transform_vector3(next - origin).normalize_or(joint.tail.normalize()))).normalize_or(axis) * length;
                }
                *tail = Tail { now: next, before: tail.now, ..*tail };
            }
            tail.held = initial.inverse().transform_point3(tail.now);
            if !still {
                let toward = tail.held.normalize_or(joint.tail.normalize());
                pose.rotations[joint.node] = (node.rotation * Quat::from_rotation_arc(joint.tail.normalize(), toward)).normalize();
                tail.rotation = pose.rotations[joint.node];
            }
            settle(model, pose, &mut worlds, joint, parent);
        }
        if executions >= 1.0 {
            self.place = place;
        }
    }

    /// Steps one PhysBone chain as VRChat does: whole 60 Hz steps with its anchor moved part way for each, and a part step for time left over that only shows.
    #[allow(clippy::too_many_arguments)]
    fn chain(&mut self, model: &Model, pose: &mut Pose, worlds: &mut [Mat4], at: usize, (place, executions): (Vec3, f32), [colliders, hands]: [&[Placed]; 2], idle: &Idle, fresh: bool) {
        let rig = &model.rig;
        let chain = &rig.chains[at];
        let now = worlds[chain.anchor];
        let (position, rotation) = placement(&now);
        let shape = Mat4::from_rotation_translation(rotation, position).inverse() * now;
        let (last, last_place) = (self.anchors[at], self.place);
        let travel = chain.joints.first().and_then(|&joint| rig.joints[joint].physbone).is_some_and(|bone| bone.travel);
        let steps = (executions.ceil() as usize).max(1);
        let mut before = Mat4::from_rotation_translation(last.1, last.0) * shape;
        for step in 0..steps {
            let (through, ratio) = if steps == 1 { (1.0, executions) } else { ((step as f32 + 1.0).min(executions) / executions, (executions - step as f32).min(1.0)) };
            let held = (last.0.lerp(position, through), last.1.slerp(rotation, through));
            let anchor = Mat4::from_rotation_translation(held.1, held.0) * shape;
            let immobile = if travel { Mat4::from_translation(last_place.lerp(place, through)) } else { anchor };
            let carrier = anchor * before.inverse();
            let shift = anchor * now.inverse();
            let whole = ratio >= 1.0;
            for (index, &at) in chain.joints.iter().enumerate() {
                let joint = &rig.joints[at];
                let tail = &mut self.tails[at];
                let node = &model.nodes[joint.node];
                let parent = node.parent.map_or(Mat4::IDENTITY, |parent| worlds[parent]);
                let parent = if index == 0 { shift * parent } else { parent };
                let local = |rotation: Quat| parent * Mat4::from_scale_rotation_translation(node.scale, rotation, pose.translations[joint.node]);
                let along = joint.tail.normalize();
                if fresh || idle.joints.contains(&joint.node) {
                    let rotation = if fresh { node.rotation } else { pose.rotations[joint.node] };
                    let world = local(rotation);
                    let end = world.transform_point3(joint.tail);
                    *tail = Tail { rotation, now: end, vector: end - world.w_axis.truncate(), velocity: Vec3::ZERO, felt: Vec3::ZERO, anchored: immobile.inverse().transform_point3(end), length: joint.tail.length(), ..*tail };
                } else if let Some(bone) = joint.physbone {
                    tail.rotation = swing(joint, bone, node.rotation, tail, &local, Frames { immobile, carrier }, (ratio, whole), joint.groups.iter().flat_map(|&group| &rig.groups[group]).filter(|&&collider| !idle.colliders.contains(&rig.colliders[collider].source)).map(|&collider| &colliders[collider]).chain(if joint.others { hands } else { &[] }));
                }
                pose.rotations[joint.node] = tail.rotation;
                if let (Some(child), false) = (joint.child, joint.physbone.is_some_and(|bone| bone.endpoint)) {
                    pose.translations[child] = along * tail.length;
                }
                settle(model, pose, worlds, joint, parent);
            }
            if whole {
                self.anchors[at] = held;
            }
            before = anchor;
        }
    }
}

/// The shortest turn from one direction to another, however small: glam's `from_rotation_arc` rounds turns under about 7e-4 radians to none, which holds a bone that gravity or a slow pull moves in smaller steps at rest forever.
fn turn(from: Vec3, to: Vec3) -> Quat {
    let axis = from.cross(to);
    let sine = axis.length();
    if sine <= f32::EPSILON * from.dot(to).abs() {
        return Quat::from_rotation_arc(from, to);
    }
    Quat::from_axis_angle(axis / sine, sine.atan2(from.dot(to)))
}

/// Puts the joint's node where `parent` and the pose now hold it, and everything below it after.
fn settle(model: &Model, pose: &Pose, worlds: &mut [Mat4], joint: &Joint, parent: Mat4) {
    worlds[joint.node] = parent * model.local(pose, joint.node);
    for &below in &joint.below {
        worlds[below] = worlds[model.nodes[below].parent.expect("a node below a joint has a parent")] * model.local(pose, below);
    }
}

/// The frames a PhysBone step weighs a bone's motion against: the one immobile holds it in, and its chain's anchor's motion over the step.
struct Frames {
    immobile: Mat4,
    carrier: Mat4,
}

/// One VRChat PhysBone step of a bone, its tail's state kept in `tail`, returning the bone's turn.
fn swing<'a>(joint: &Joint, bone: PhysBone, resting: Quat, tail: &mut Tail, local: &impl Fn(Quat) -> Mat4, Frames { immobile, carrier }: Frames, (ratio, whole): (f32, bool), touching: impl Iterator<Item = &'a Placed>) -> Quat {
    let along = joint.tail.normalize();
    let current = local(tail.rotation);
    let begin = current.w_axis.truncate();
    let reach = current.transform_vector3(along * tail.length);
    let mut length = reach.length();
    if length < 8e-6 {
        return tail.rotation;
    }
    let scale = current.x_axis.truncate().length().max(current.y_axis.truncate().length()).max(current.z_axis.truncate().length());
    let original = local(resting);
    let mut target = original.transform_point3(joint.tail);
    let rest = (target - begin).length().max(8e-6);
    let start = tail.now;
    let held = (immobile.transform_point3(tail.anchored) - start) * bone.immobile;
    // Steady motion of the chain's anchor carries the tail with it; only a change in that motion swings it, at the share immobile leaves felt.
    let felt = carrier.transform_point3(start) - start - held;
    let mut end = start + held + felt;
    let from = end;
    let velocity = tail.velocity + tail.felt - felt;
    let vector = carrier.transform_vector3(tail.vector);
    let down = Vec3::NEG_Y;
    let weigh = |gravity: f32| {
        if bone.falloff <= 0.0 {
            return gravity;
        }
        let turned = (1.0 - current.transform_vector3(tail.down).normalize_or(down).dot(down)).min(1.0);
        gravity * (1.0 - bone.falloff + bone.falloff * turned)
    };
    let momentum = (0.99 * bone.spring).min(1.0);
    let mut step;
    if bone.first {
        let restore = (rest - length) * bone.pull * 0.5;
        length += restore;
        step = (end - begin).normalize_or_zero() * restore;
        if bone.advanced {
            step += velocity * bone.spring;
            step += (target - (end + step)) * bone.pull;
            if bone.gravity != 0.0 {
                step += down * weigh(bone.gravity * length);
            }
            step += vector * bone.stiffness;
        } else {
            step += (target - end) * bone.pull;
            if bone.gravity != 0.0 {
                let toward = vector.normalize_or_zero();
                let across = Quat::from_axis_angle(toward.cross(down).normalize_or_zero(), std::f32::consts::FRAC_PI_2) * toward;
                step += across * weigh(bone.gravity * length) * across.dot(down);
            }
            step = step.lerp(velocity, momentum);
            if whole {
                tail.velocity = step;
            }
        }
        step = begin + (end + step - begin).normalize_or_zero() * length - end;
    } else {
        if bone.gravity != 0.0 {
            let pulled = down * bone.gravity.signum() * rest;
            target = begin + (target - begin).lerp(pulled, weigh(bone.gravity.abs())).normalize_or_zero() * rest;
        }
        if bone.advanced {
            step = velocity * bone.spring;
            step += (target - (end + step)) * bone.pull;
            if bone.stiffness > 0.0 {
                step += (vector - (end + step - begin)) * bone.stiffness;
            }
        } else {
            step = ((target - end) * bone.pull).lerp(velocity, momentum);
            if whole {
                tail.velocity = step;
            }
        }
        if bone.stretch_motion < 1.0 {
            let toward = end + step - begin;
            let (reached, was) = (toward.length(), vector.length());
            let kept = if was < rest { reached.clamp(was, rest) } else { reached.clamp(rest, was) };
            step = begin + toward.normalize_or_zero() * (kept + (reached - kept) * bone.stretch_motion) - end;
        }
    }
    end += step;
    let mut pressed = Pressed { begin, end, radius: bone.radius.map(|radius| radius * scale), shortest: rest * bone.squish, longest: rest * bone.stretch, fallback: reach };
    pressed.end = begin + clamp_length(end - begin, pressed.shortest, pressed.longest, reach);
    // A collider may squish the bone but, as in VRChat, never stretch it past the length it reached this step.
    pressed.fallback = pressed.end - begin;
    pressed.longest = pressed.fallback.length();
    let mut hit = false;
    for placed in touching {
        hit |= placed.press(&mut pressed);
    }
    if hit && !bone.advanced && whole {
        tail.velocity *= 0.75;
    }
    let length = (pressed.end - begin).length();
    let blend = if ratio > 0.0 { ratio / (1.0 + (ratio - 1.0) * bone.pull) } else { 0.0 };
    let end = begin + (from.lerp(pressed.end, blend) - begin).normalize_or_zero() * length;
    tail.length = joint.tail.length() * (length / rest);
    let axis = original.inverse().transform_point3(end).normalize_or(along);
    let axis = joint.settings.limit.map_or(axis, |limit| limit.keep(joint.tail, axis));
    let rotation = (resting * turn(along, axis)).normalize();
    let end = local(rotation).transform_point3(along * tail.length);
    if whole {
        tail.anchored = immobile.inverse().transform_point3(end);
        tail.now = end;
        tail.vector = end - begin;
        tail.felt = felt;
        if bone.advanced {
            tail.velocity = end - from;
        }
    }
    rotation
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
            springs.step(model, &mut pose, anchor(time), &[], STEP, &Idle::default());
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
        assert_eq!(model.rig.colliders[0].shape, Shape::Round { tail: None, radius: 0.04, inside: false });
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
        assert_eq!((capsule.offset, capsule.shape), (Vec3::new(0.0, 0.1, 0.0), Shape::Round { tail: Some(Vec3::new(0.0, 0.3, 0.0)), radius: 0.05, inside: false }));
        assert_eq!(model.rig.joints[0].groups, [0]);
    }

    #[test]
    fn each_joint_pulls_with_its_own_files_gravity_and_none_where_the_file_names_none() {
        let pulled = strand(serde_json::json!({ "bones": [2], "gravityPower": 0.7, "gravityDir": { "x": 1, "y": 0, "z": 0 } }), serde_json::json!([]));
        assert!(pulled.rig.joints.iter().all(|joint| joint.settings.gravity == Vec3::new(0.7, 0.0, 0.0)));
        let unpulled = strand(serde_json::json!({ "bones": [2] }), serde_json::json!([]));
        assert!(unpulled.rig.joints.iter().all(|joint| joint.settings.gravity == Vec3::ZERO));
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
    fn gliding_steadily_leaves_the_hair_at_rest_and_only_a_change_of_speed_swings_it() {
        let model = hair(1.0, 0.0);
        let mut springs = Springs::default();
        let gliding = |time: f32| Mat4::from_translation(Vec3::new(time.max(0.0), 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(0.2));
        let rest = run(&model, &mut springs, gliding, -1.0, 0.0).last().unwrap().1;
        let moving = run(&model, &mut springs, gliding, 0.0, 3.0);
        let trail = |from: f32, to: f32| moving.iter().filter(|(time, _)| (from..to).contains(time)).map(|(_, tip)| (tip.x - rest.x).abs()).fold(0.0f32, f32::max);
        assert!(trail(0.0, 0.5) > 0.03, "setting off swings the tip back: {}", trail(0.0, 0.5));
        assert!(trail(2.5, 3.0) < 0.002, "at a steady speed it hangs as at rest: {}", trail(2.5, 3.0));
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
    fn a_stilled_strand_holds_the_pose_while_swayed_and_sets_off_from_it_when_freed() {
        let model = hair(1.0, 0.0);
        let swaying = |time: f32| Mat4::from_translation(Vec3::new(0.1 * (6.0 * time).sin(), 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(0.2));
        let posed = || {
            let mut pose = model.rest();
            pose.rotations[2] = model.nodes[2].rotation * Quat::from_rotation_x(0.5);
            pose
        };
        let held = model.worlds(&posed())[4].w_axis.truncate();
        let rest = model.worlds(&model.rest())[4].w_axis.truncate();
        let mut springs = Springs::default();
        let mut tips = Vec::new();
        for frame in 0..270 {
            let idle = Idle { joints: if (90..180).contains(&frame) { HashSet::from([2, 3, 4]) } else { HashSet::new() }, colliders: HashSet::new() };
            let mut pose = posed();
            springs.step(&model, &mut pose, swaying(frame as f32 / 90.0), &[], 1.0 / 90.0, &idle);
            if !idle.joints.is_empty() {
                assert_eq!(pose.rotations, posed().rotations, "frame {frame}");
            }
            tips.push(model.worlds(&pose)[4].w_axis.truncate());
        }
        let swing = tips[..90].iter().map(|tip| tip.distance(rest)).fold(0.0f32, f32::max);
        assert!(swing > 0.01, "free, the strand swings as the figure sways: {swing}");
        assert!(tips[90..180].iter().all(|tip| tip.abs_diff_eq(held, 1e-5)), "stilled, it holds the pose however the figure sways");
        let jump = tips[180].distance(tips[179]);
        assert!(jump < 0.005, "freed, it sets off from the held pose without a fling: {jump}");
        assert!(tips[269].distance(rest) < tips[180].distance(rest), "and swings back toward rest");
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
            springs.step(&model, &mut pose, Mat4::IDENTITY, &[], 1.0 / 90.0, &Idle::default());
            model.worlds(&pose)[3].w_axis.truncate()
        };
        let free = hanging(serde_json::json!([{ "node": 1, "colliders": [] }]));
        assert!(free.abs_diff_eq(Vec3::new(0.0, 0.9, 0.1), 2e-3), "without the sphere the strand hangs straight down: {free}");
        let blocked = hanging(serde_json::json!([{ "node": 1, "colliders": [{ "offset": { "x": 0.0, "y": -0.1, "z": -0.13 }, "radius": 0.04 }] }]));
        let sphere = Vec3::new(0.0, 0.9, 0.13);
        assert!((blocked - sphere).length() >= 0.04 - 1e-4, "with it, the strand's middle rests on the sphere, its offset's z turned into glTF's: {blocked}");
        assert!(blocked.y < 0.99, "having fallen onto it: {blocked}");
    }

    /// A strand of three 0.1 segments reaching forward from 1.0 up, falling under gravity onto the one collider given, in VRM 1 form.
    fn fallen(collider: Value) -> Vec<Vec3> {
        let mut builder = Builder::new(serde_json::json!({
            "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 } } } },
            "VRMC_springBone": {
                "colliders": [collider],
                "colliderGroups": [{ "colliders": [0] }],
                "springs": [{ "joints": [{ "node": 2, "stiffness": 0.0, "gravityPower": 1.0, "dragForce": 0.4 }, { "node": 3, "stiffness": 0.0, "gravityPower": 1.0, "dragForce": 0.4 }, { "node": 4, "stiffness": 0.0, "gravityPower": 1.0, "dragForce": 0.4 }, { "node": 5 }], "colliderGroups": [0] }]
            }
        }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "children": [1], "mesh": 0 }, { "children": [2] }, { "translation": [0.0, 1.0, 0.0], "children": [3] }, { "translation": [0.0, 0.0, 0.1], "children": [4] }, { "translation": [0.0, 0.0, 0.1], "children": [5] }, { "translation": [0.0, 0.0, 0.1] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let mut springs = Springs::default();
        run(&model, &mut springs, |_| Mat4::IDENTITY, 0.0, 3.0);
        let mut pose = model.rest();
        springs.step(&model, &mut pose, Mat4::IDENTITY, &[], 1.0 / 90.0, &Idle::default());
        let worlds = model.worlds(&pose);
        (3..6).map(|node| worlds[node].w_axis.truncate()).collect()
    }

    /// The `fallen` strand, its joints driven by one PhysBone and its sphere both waiting on the menu's one switch, stepped for three seconds at `value`.
    fn switched(value: f32) -> (Vec<Vec3>, Vec<Vec3>) {
        let center = Vec3::new(0.0, 0.8, 0.02);
        let mut builder = Builder::new(serde_json::json!({
            "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 } } } },
            "VRMC_springBone": {
                "colliders": [{ "node": 1, "shape": nowhere() }, { "node": 1, "shape": { "sphere": { "offset": center.to_array(), "radius": 0.04 } } }],
                "colliderGroups": [{ "colliders": [0, 1] }],
                "springs": [{ "joints": [{ "node": 2, "stiffness": 0.0, "gravityPower": 1.0, "dragForce": 0.4 }, { "node": 3, "stiffness": 0.0, "gravityPower": 1.0, "dragForce": 0.4 }, { "node": 4, "stiffness": 0.0, "gravityPower": 1.0, "dragForce": 0.4 }, { "node": 5 }], "colliderGroups": [0] }]
            },
            "VRCHAT_expressions": { "menu": [], "parameters": [], "layers": [], "clips": [], "targets": [{ "effect": "switch" }, { "effect": "switch", "default": 1 }], "springs": [{ "joints": [2, 3, 4, 5], "when": [[1, 1]] }], "colliders": [{ "collider": 1, "when": [[0, 1]] }] }
        }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "children": [1], "mesh": 0 }, { "children": [2] }, { "translation": [0.0, 1.0, 0.0], "children": [3] }, { "translation": [0.0, 0.0, 0.1], "children": [4] }, { "translation": [0.0, 0.0, 0.1], "children": [5] }, { "translation": [0.0, 0.0, 0.1] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let menu = model.menu.as_ref().expect("the menu parses");
        let settle = |values: &[f32]| {
            let idle = menu.idle(values);
            let mut springs = Springs::default();
            let mut pose = model.rest();
            for _ in 0..270 {
                pose = model.rest();
                springs.step(&model, &mut pose, Mat4::IDENTITY, &[], 1.0 / 90.0, &idle);
            }
            let worlds = model.worlds(&pose);
            (3..6).map(|node| worlds[node].w_axis.truncate()).collect::<Vec<_>>()
        };
        (settle(&[value, 1.0]), settle(&[value, 0.0]))
    }

    #[test]
    fn a_collider_and_a_physbone_the_menu_switches_follow_it() {
        let center = Vec3::new(0.0, 0.8, 0.02);
        let (off, _) = switched(0.0);
        assert!(off[2].abs_diff_eq(Vec3::new(0.0, 0.7, 0.0), 2e-3), "its sphere switched off, the strand hangs straight through where it would be: {off:?}");
        let (on, held) = switched(1.0);
        for joint in &on {
            assert!((*joint - center).length() >= 0.04 - SETTLED, "switched on, the sphere holds every joint out: {on:?}");
        }
        assert!(on[1].z > 0.03, "and the strand rests on its front: {on:?}");
        let rest = [Vec3::new(0.0, 1.0, 0.1), Vec3::new(0.0, 1.0, 0.2), Vec3::new(0.0, 1.0, 0.3)];
        assert!(held.iter().zip(rest).all(|(joint, rest)| joint.abs_diff_eq(rest, 1e-5)), "its PhysBone switched off, the strand keeps the pose instead of falling: {held:?}");
    }

    /// One push and the segment's length restored, as three-vrm does, leaves a joint up to this deep.
    const SETTLED: f32 = 0.01;

    fn nowhere() -> Value {
        serde_json::json!({ "sphere": { "offset": [0.0, -10.0, 0.0], "radius": 0.0 } })
    }

    #[test]
    fn a_chain_stays_outside_a_sphere_collider() {
        let free = fallen(serde_json::json!({ "node": 1, "shape": nowhere() }));
        assert!(free[2].abs_diff_eq(Vec3::new(0.0, 0.7, 0.0), 2e-3), "without it the strand hangs straight down: {free:?}");
        let center = Vec3::new(0.0, 0.8, 0.02);
        let held = fallen(serde_json::json!({ "node": 1, "shape": { "sphere": { "offset": center.to_array(), "radius": 0.04 } } }));
        for joint in &held {
            assert!((*joint - center).length() >= 0.04 - SETTLED, "every joint rests outside the sphere: {held:?}");
        }
        assert!(held[1].z > 0.03, "falling from in front, the strand comes to rest on the sphere's front instead of hanging through it: {held:?}");
    }

    #[test]
    fn a_chain_stays_outside_a_capsule_collider() {
        let (start, end) = (Vec3::new(-0.2, 0.8, 0.02), Vec3::new(0.1, 0.8, 0.02));
        let held = fallen(serde_json::json!({ "node": 1, "shape": { "capsule": { "offset": start.to_array(), "tail": end.to_array(), "radius": 0.04 } } }));
        for joint in &held {
            let span = end - start;
            let nearest = start + span * ((*joint - start).dot(span) / span.length_squared()).clamp(0.0, 1.0);
            assert!((*joint - nearest).length() >= 0.04 - SETTLED, "every joint rests outside the capsule, measured from its axis between the ends: {held:?}");
        }
        assert!(held[1].z > 0.03, "falling from in front, the strand comes to rest on the bar's front, between its ends: {held:?}");
    }

    #[test]
    fn an_inside_sphere_keeps_the_chain_within_it() {
        let held = fallen(serde_json::json!({ "node": 1, "shape": nowhere(), "extensions": { "VRMC_springBone_extended_collider": { "specVersion": "1.0", "shape": { "sphere": { "offset": [0.0, 1.0, 0.0], "radius": 0.25, "inside": true } } } } }));
        for joint in &held {
            assert!((*joint - Vec3::new(0.0, 1.0, 0.0)).length() <= 0.25 + SETTLED, "every joint stays within the sphere: {held:?}");
        }
    }

    #[test]
    fn an_inside_capsule_keeps_the_chain_within_it() {
        let held = fallen(serde_json::json!({ "node": 1, "shape": nowhere(), "extensions": { "VRMC_springBone_extended_collider": { "specVersion": "1.0", "shape": { "capsule": { "offset": [0.0, 1.0, 0.0], "tail": [0.0, 1.0, 0.3], "radius": 0.15, "inside": true } } } } }));
        for joint in &held {
            let nearest = Vec3::new(0.0, 1.0, joint.z.clamp(0.0, 0.3));
            assert!((*joint - nearest).length() <= 0.15 + SETTLED, "every joint stays within the capsule: {held:?}");
        }
    }

    #[test]
    fn a_plane_keeps_the_chain_on_its_normals_side() {
        let held = fallen(serde_json::json!({ "node": 1, "shape": nowhere(), "extensions": { "VRMC_springBone_extended_collider": { "specVersion": "1.0", "shape": { "plane": { "offset": [0.0, 0.85, 0.0], "normal": [0.0, 1.0, 0.0] } } } } }));
        assert!(held.iter().all(|joint| joint.y >= 0.85 - SETTLED), "no joint sinks below the plane: {held:?}");
        assert!(held[2].y < 0.9, "the strand still falls to it: {held:?}");
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
        springs.step(&model, &mut pose, Mat4::IDENTITY, &[], 1.0 / 90.0, &Idle::default());
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
        springs.step(&model, &mut pose, Mat4::IDENTITY, &[], 1.0 / 90.0, &Idle::default());
        let end = model.worlds(&pose)[3].w_axis.truncate();
        assert!(end.abs_diff_eq(Vec3::new(0.0, 0.9, 0.1), 2e-3), "{end}");
    }

    /// Where the strand's tip comes to rest with another's hand held just under it, when its bone group's extras list `collide`, or list none.
    fn touched(collide: Option<Value>) -> Vec3 {
        let mut group = serde_json::json!({ "bones": [2], "stiffiness": 1.0, "dragForce": 0.1, "gravityPower": 0.0, "hitRadius": 0.0 });
        if let Some(collide) = collide {
            group["extras"] = serde_json::json!({ "collide": collide });
        }
        let model = strand(group, serde_json::json!([]));
        let mut springs = Springs::default();
        let hand = [Hand { center: Vec3::new(0.0, 0.97, 0.3), radius: 0.05 }];
        let mut pose = model.rest();
        for _ in 0..90 {
            pose = model.rest();
            springs.step(&model, &mut pose, Mat4::IDENTITY, &hand, 1.0 / 90.0, &Idle::default());
        }
        tip(&model, &pose, Mat4::IDENTITY)
    }

    #[test]
    fn another_hand_pushes_a_chain_whose_file_lets_others_collide_and_passes_through_one_it_does_not() {
        let rest = Vec3::new(0.0, 1.0, 0.3);
        for allowed in [Some(serde_json::json!(["wearer", "others"])), Some(serde_json::json!(["others"])), None] {
            let at = touched(allowed.clone());
            assert!(at.y > rest.y + 0.01, "a hand under a strand whose collide is {allowed:?} lifts its tip: {at}");
        }
        for refused in [serde_json::json!(["wearer"]), serde_json::json!([])] {
            let at = touched(Some(refused.clone()));
            assert!(at.abs_diff_eq(rest, 1e-4), "a hand passes through a strand whose collide is {refused}: {at}");
        }
    }

    /// How far, in degrees, a hand rising up through a three-segment strand reaching forward swings its first segment at most, each joint's extras `limit` as given.
    fn swung(limit: Option<Value>) -> f32 {
        let joint = |node: usize| {
            let mut joint = serde_json::json!({ "node": node, "stiffness": 1.0, "dragForce": 0.4 });
            if let Some(limit) = &limit {
                joint["extras"] = serde_json::json!({ "limit": limit });
            }
            joint
        };
        let mut builder = Builder::new(serde_json::json!({
            "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 } } } },
            "VRMC_springBone": { "springs": [{ "joints": [joint(2), joint(3), joint(4), { "node": 5 }] }] }
        }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "children": [1], "mesh": 0 }, { "children": [2] }, { "translation": [0.0, 1.0, 0.0], "children": [3] }, { "translation": [0.0, 0.0, 0.1], "children": [4] }, { "translation": [0.0, 0.0, 0.1], "children": [5] }, { "translation": [0.0, 0.0, 0.1] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let mut springs = Springs::default();
        let mut most = 0.0f32;
        for frame in 0..90 {
            let hand = [Hand { center: Vec3::new(0.0, 0.9 + 0.4 * frame as f32 / 90.0, 0.12), radius: 0.07 }];
            let mut pose = model.rest();
            springs.step(&model, &mut pose, Mat4::IDENTITY, &hand, 1.0 / 90.0, &Idle::default());
            let worlds = model.worlds(&pose);
            most = most.max((worlds[3].w_axis - worlds[2].w_axis).truncate().angle_between(Vec3::Z).to_degrees());
        }
        most
    }

    #[test]
    fn a_hand_swings_a_limited_joint_no_further_than_its_limit() {
        let free = swung(None);
        assert!(free > 40.0, "unlimited, the hand lifts the strand's first segment well past 20 degrees: {free}");
        for shape in ["angle", "hinge", "polar"] {
            let held = swung(Some(serde_json::json!({ "shape": shape, "x": 20.0, "z": 20.0 })));
            assert!(held <= 20.0 + 0.1 && held > 15.0, "a {shape} limit of 20 degrees holds the pushed segment at it: {held}");
        }
        let tilted = swung(Some(serde_json::json!({ "shape": "angle", "x": 20.0, "rotation": [-(7.5f32.to_radians().sin()), 0.0, 0.0, 7.5f32.to_radians().cos()] })));
        assert!(tilted > 25.0 && tilted <= 35.0 + 0.1, "the limit's rotation tilts the cone it keeps the joint in: {tilted}");
    }

    #[test]
    fn a_polar_limit_lets_a_joint_swing_its_own_way_further_each_way() {
        let wide = Limit { bound: Bound::Polar, x: 10f32.to_radians(), z: 40f32.to_radians(), rotation: Quat::IDENTITY };
        let far = |toward: Vec3| wide.keep(Vec3::Y, (Vec3::Y + toward).normalize()).angle_between(Vec3::Y).to_degrees();
        assert!((far(Vec3::Z * 5.0) - 10.0).abs() < 0.1, "toward Z it stops at x: {}", far(Vec3::Z * 5.0));
        assert!((far(Vec3::X * 5.0) - 40.0).abs() < 0.1, "toward X at z: {}", far(Vec3::X * 5.0));
        assert!(far(Vec3::X * 0.3) < 40.0 && (far(Vec3::X * 0.3) - 0.3f32.atan().to_degrees()).abs() < 0.01, "and inside it is left alone");
    }

    /// A Unity point, rotation or pose as glTF holds it: Unity's X mirrored.
    fn from_unity(p: [f32; 3]) -> Vec3 {
        Vec3::new(-p[0], p[1], p[2])
    }

    fn from_unity_turn(q: Quat) -> Quat {
        Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
    }

    fn from_unity_pose(p: [f32; 3], q: [f32; 4]) -> Mat4 {
        Mat4::from_rotation_translation(from_unity_turn(Quat::from_array(q)), from_unity(p))
    }

    /// A chain of nodes hanging from the root, each given in Unity's axes below the last, every one but the last a joint with its extras "physbone" from `bones`, and `limit` if any.
    fn physbones(nodes: &[([f32; 3], Quat)], bones: &[Value], limit: Option<Value>) -> Model {
        let mut listed = vec![serde_json::json!({ "children": [1], "mesh": 0 })];
        for (at, (position, turn)) in nodes.iter().enumerate() {
            let (position, turn) = (from_unity(*position), from_unity_turn(*turn));
            let mut node = serde_json::json!({ "translation": position.to_array(), "rotation": turn.to_array() });
            if at + 1 < nodes.len() {
                node["children"] = serde_json::json!([at + 2]);
            }
            listed.push(node);
        }
        let mut joints: Vec<Value> = bones.iter().enumerate().map(|(at, bone)| {
            let mut extras = serde_json::json!({ "physbone": bone });
            if let Some(limit) = &limit {
                extras["limit"] = limit.clone();
            }
            serde_json::json!({ "node": at + 1, "extras": extras })
        }).collect();
        joints.push(serde_json::json!({ "node": bones.len() + 1 }));
        let mut builder = Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 0 } } } }, "VRMC_springBone": { "springs": [{ "joints": joints }] } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", Value::Array(listed));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        Model::parse(&builder.glb()).unwrap()
    }

    /// A breast-like two-bone PhysBone on a chest, its last bone ending at an endpoint: version 1.1, advanced, immobile, a polar limit and squish.
    fn breast() -> Model {
        let bone = |gravity: f32, radius: [f32; 2], depth: usize, endpoint: bool| serde_json::json!({ "version": "1.1", "integration": "advanced", "pull": 0.066, "spring": 0.765, "stiffness": 0.064, "gravity": gravity, "immobile": 0.38, "immobileType": "allMotion", "maxSquish": 0.37, "radius": radius, "depth": depth, "endpoint": endpoint });
        let tilted = Quat::from_euler(glam::EulerRot::YXZ, 10f32.to_radians(), 80f32.to_radians(), 0.0);
        physbones(&[([-0.08, 0.0, 0.10], tilted), ([0.0, 0.05, 0.01], Quat::IDENTITY), ([0.0, 0.019, 0.0], Quat::IDENTITY)], &[bone(0.003, [0.0288, 0.0324], 0, false), bone(0.01, [0.0324, 0.036], 1, true)], Some(serde_json::json!({ "shape": "polar", "x": 14.0, "z": 16.0 })))
    }

    /// Where the PhysBone's last node sits in the anchor's frame after `frames` 60 Hz steps with the anchor held at `anchor` and the hand, if any, where given.
    fn rested(model: &Model, springs: &mut Springs, anchor: Mat4, hand: Option<Vec3>, frames: usize) -> Vec3 {
        let mut pose = model.rest();
        for _ in 0..frames {
            pose = model.rest();
            let hands: Vec<Hand> = hand.map(|center| Hand { center, radius: 0.04 }).into_iter().collect();
            springs.step(model, &mut pose, anchor, &hands, 1.0 / 60.0, &Idle::default());
        }
        model.worlds(&pose)[model.nodes.len() - 1].w_axis.truncate()
    }

    #[test]
    fn a_physbone_pushed_by_a_hand_comes_to_rest_where_vrchat_puts_it() {
        // VRChat's own PhysBone in Unity (SDK 3.10.5), the same chain on a chest, a 4 cm sphere pushed 1 cm nearer every half second: the tip in the chest's frame at the end of each.
        const UNITY: [[f32; 3]; 15] = [[0.06789, 0.00175, 0.16867], [0.06789, 0.00175, 0.16867], [0.06789, 0.00175, 0.16867], [0.06789, 0.00175, 0.16867], [0.06789, 0.00175, 0.16867], [0.06789, 0.00175, 0.16867], [0.06789, 0.00175, 0.16867], [0.06283, 0.00001, 0.16723], [0.06283, -0.00264, 0.16723], [0.05976, -0.00505, 0.16227], [0.05364, -0.00988, 0.15438], [0.05114, -0.01484, 0.14595], [0.05358, -0.01341, 0.14108], [0.04656, -0.01752, 0.15512], [0.04656, -0.01752, 0.15512]];
        let model = breast();
        let chest = from_unity_pose([0.000945, 1.190331, 0.003663], [0.003841, -0.005742, -0.000175, 0.999976]);
        let mut springs = Springs::default();
        rested(&model, &mut springs, chest, None, 600);
        for (step, unity) in UNITY.iter().enumerate() {
            let hand = from_unity([-0.10, 1.21, 0.30 - 0.01 * step as f32]);
            let tip = rested(&model, &mut springs, chest, Some(hand), 30);
            assert!(tip.distance(Vec3::from(*unity)) < 1e-4, "push {step}: the tip rests at {tip}, VRChat's at {unity:?}");
        }
    }

    #[test]
    fn physbones_hang_where_vrchat_hangs_them() {
        let simplified = |depth: usize| serde_json::json!({ "version": "1.0", "integration": "simplified", "pull": 0.2, "spring": 0.4, "stiffness": 0.2, "gravity": 0.4, "gravityFalloff": 0.5, "stretchMotion": 1.0, "radius": [0.02, 0.02], "depth": depth });
        let skirt = physbones(&[([0.08, 0.0, 0.10], Quat::IDENTITY), ([0.0, -0.03, 0.05], Quat::IDENTITY), ([0.0, -0.05, 0.02], Quat::IDENTITY), ([0.0, -0.05, 0.0], Quat::IDENTITY)], &(0..3).map(simplified).collect::<Vec<_>>(), None);
        let advanced = |depth: usize| {
            let along = depth as f32 / 3.0;
            serde_json::json!({ "version": "1.1", "integration": "advanced", "pull": 0.15 * (1.0 - 0.4 * along), "spring": 0.6, "stiffness": 0.3 * (1.0 - 0.6 * along), "gravity": 0.3, "gravityFalloff": 0.5, "immobile": 0.2, "maxStretch": 0.2, "maxSquish": 0.2, "stretchMotion": 0.5, "radius": [0.02, 0.02], "depth": depth, "endpoint": depth == 3 })
        };
        let ear = physbones(&[([0.1, 0.05, 0.0], Quat::IDENTITY), ([0.02, -0.06, 0.0], Quat::IDENTITY), ([0.0, -0.06, 0.01], Quat::IDENTITY), ([0.0, -0.06, 0.0], Quat::IDENTITY), ([0.0, -0.03, 0.0], Quat::IDENTITY)], &(0..4).map(advanced).collect::<Vec<_>>(), Some(serde_json::json!({ "shape": "angle", "x": 50.0 })));
        let chest = from_unity_pose([0.000945, 1.190331, 0.003663], [0.003841, -0.005742, -0.000175, 0.999976]);
        let head = from_unity_pose([0.000895, 1.490226, 0.000747], [-0.030452, 0.004913, 0.000490, 0.999524]);
        // VRChat's in Unity, the chains on a still chest and head: version 1.0 simplified with gravity falloff, and 1.1 advanced with curves, stretch and an angle limit.
        for (model, anchor, unity) in [(&skirt, chest, Vec3::new(-0.08004, -0.15420, 0.11176)), (&ear, head, Vec3::new(-0.11168, -0.16106, 0.00289))] {
            let tip = rested(model, &mut Springs::default(), anchor, None, 600);
            assert!(tip.distance(unity) < 1e-4, "the tip hangs at {tip}, VRChat's at {unity}");
        }
    }

    #[test]
    fn gliding_steadily_leaves_a_physbone_at_rest_and_only_a_change_of_speed_swings_it() {
        let model = breast();
        let mut springs = Springs::default();
        let rest = rested(&model, &mut springs, Mat4::IDENTITY, None, 600);
        let mut pose = model.rest();
        let mut trail = Vec::new();
        for frame in 0..240 {
            let anchor = Mat4::from_translation(Vec3::new(0.0, 0.0, frame as f32 / 60.0));
            pose = model.rest();
            springs.step(&model, &mut pose, anchor, &[], 1.0 / 60.0, &Idle::default());
            trail.push(model.worlds(&pose)[model.nodes.len() - 1].w_axis.truncate().distance(rest));
        }
        assert!(trail[..30].iter().copied().fold(0.0, f32::max) > 0.002, "setting off at 1 m/s swings the tip: {:?}", &trail[..30]);
        assert!(trail[200..].iter().all(|off| *off < 1e-4), "gliding on at that speed it hangs as at rest, where VRChat's would trail: {:?}", &trail[200..]);
    }

    #[test]
    fn a_vrm_1_springs_extras_say_whether_others_push_it() {
        let mut builder = Builder::new(serde_json::json!({
            "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 } } } },
            "VRMC_springBone": { "springs": [
                { "joints": [{ "node": 2 }, { "node": 3 }], "extras": { "collide": ["wearer"] } },
                { "joints": [{ "node": 3 }, { "node": 4 }] }
            ] }
        }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "children": [1], "mesh": 0 }, { "children": [2] }, { "translation": [0.0, 1.0, 0.0], "children": [3] }, { "translation": [0.0, 0.0, 0.1], "children": [4] }, { "translation": [0.0, 0.0, 0.2] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let others: Vec<(usize, bool)> = model.rig.joints.iter().map(|joint| (joint.node, joint.others)).collect();
        assert_eq!(others, [(2, false), (3, true)]);
    }
}
