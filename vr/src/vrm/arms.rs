use std::collections::HashSet;

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{Humanoid, Model, Skinned, Version};
use crate::motion::Clip;

pub const EXTENSION: &str = "VOICE_arms";
/// The measurement's revision: a spread found by another measurement is refused rather than worn.
pub const REVISION: u64 = 1;
/// The widest spread the search reaches, radians; past it an arm reads as held out on purpose.
const WIDEST: f32 = 25.0 * std::f32::consts::PI / 180.0;
/// How deep skin may meet skin before it counts as clipping, metres: two surfaces resting on each other.
pub const TOUCH: f32 = 0.003;
/// The deepest samples dropped from each frame's depth, so one sliver of a bin does not stand for the frame.
const STRAYS: usize = 4;
/// The widest gap between surface samples, metres.
const SPACING: f32 = 0.01;
/// The farthest a forearm or hand sample looks for the thigh skin it may have sunk under, metres; deeper than this the nearest skin is the far side.
const REACH: f32 = 0.05;
/// Frames a second each clip is sampled at.
pub const RATE: f32 = 15.0;
const RESOLUTION: f32 = 0.25 * std::f32::consts::PI / 180.0;

/// How far a figure's upper arms turn out from their rest, measured once on its own body against the clips it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct Spread {
    pub angle: f32,
    pub body: String,
    pub clips: String,
    pub revision: u64,
}

impl Spread {
    pub fn json(&self) -> Value {
        serde_json::json!({ "angle": self.angle, "body": self.body, "clips": self.clips, "revision": self.revision })
    }
}

/// The digest of what a spread is measured on: the skeleton, skins, meshes, humanoid and menu the file's JSON names, and the
/// binary its accessors read. Images are left out: they cost most of the reading and move no arm.
pub fn body_digest(json: &Value, binary: &[u8]) -> String {
    let mut hasher = Sha256::new();
    let extensions = &json["extensions"];
    for part in [&json["nodes"], &json["skins"], &json["meshes"], &json["accessors"], &json["bufferViews"], &extensions["VRMC_vrm"]["humanoid"], &extensions["VRM"]["humanoid"], &extensions[super::expressions::EXTENSION]] {
        hasher.update(serde_json::to_vec(part).unwrap_or_default());
        hasher.update([0]);
    }
    let views = json["bufferViews"].as_array();
    let mut read: Vec<usize> = Vec::new();
    for accessor in json["accessors"].as_array().into_iter().flatten() {
        read.extend(accessor["bufferView"].as_u64().map(|view| view as usize));
        for part in ["indices", "values"] {
            read.extend(accessor["sparse"][part]["bufferView"].as_u64().map(|view| view as usize));
        }
    }
    read.sort_unstable();
    read.dedup();
    for view in read {
        let Some(view) = views.and_then(|views| views.get(view)) else { continue };
        let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let end = start + view["byteLength"].as_u64().unwrap_or(0) as usize;
        hasher.update(binary.get(start..end).unwrap_or_default());
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The digest of the clips a spread is measured against: each one's name and the content hash of its source, in name order.
pub fn clips_digest<'a>(clips: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut clips: Vec<_> = clips.into_iter().collect();
    clips.sort();
    let mut hasher = Sha256::new();
    for (name, source) in clips {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(source.as_bytes());
        hasher.update([0]);
    }
    hex(&hasher.finalize())
}

/// The content hash of a clip's source, as the catalog hashes it.
pub fn source_hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The content hash of the walk bundled with the host.
pub fn walk_source() -> String {
    source_hash(WALK)
}

pub const WALK: &[u8] = include_bytes!("../../motions/walk.json");

/// The extension as the file carries it, against the digest of the file's own binary chunk.
pub fn parse(json: &Value, binary: &[u8]) -> Result<Spread, String> {
    let value = json["extensions"].get(EXTENSION).ok_or(format!("no {EXTENSION}: publish the figure through publish-avatar, which measures its arm spread"))?;
    let spread = Spread {
        angle: value["angle"].as_f64().filter(|angle| angle.is_finite() && (0.0..=WIDEST as f64 + 1e-6).contains(angle)).ok_or(format!("{EXTENSION} angle is missing or out of range"))? as f32,
        body: value["body"].as_str().ok_or(format!("{EXTENSION} without body"))?.to_owned(),
        clips: value["clips"].as_str().ok_or(format!("{EXTENSION} without clips"))?.to_owned(),
        revision: value["revision"].as_u64().ok_or(format!("{EXTENSION} without revision"))?,
    };
    if spread.revision != REVISION {
        return Err(format!("{EXTENSION} was measured by revision {}, this host measures {REVISION}: republish the figure", spread.revision));
    }
    if spread.body != body_digest(json, binary) {
        return Err(format!("{EXTENSION} was measured on another body than this file's: republish the figure"));
    }
    Ok(spread)
}

/// The turn that spreads the `side` upper arm out by `angle`, in the normalized frame a clip turns it in.
pub fn turn(version: Version, side: &str, angle: f32) -> Quat {
    let outward = if side == "left" { 1.0 } else { -1.0 };
    let turn = Quat::from_rotation_z(outward * angle);
    match version {
        Version::One => turn,
        Version::Zero => Quat::from_xyzw(-turn.x, turn.y, -turn.z, turn.w),
    }
}

/// A point on the skin: a triangle's corners and its weights over them.
type Sample = ([u32; 3], [f32; 3]);

struct Side {
    thigh: Vec<Sample>,
    arm: Vec<Sample>,
}

/// The figure's forearms and hands and its thighs and hip flare as samples over their skin, to measure how deep one sinks into the other.
pub struct Clearance {
    sides: Vec<Side>,
    vertices: Vec<usize>,
}

fn samples(triangles: &[[u32; 3]], rest: &[Vec3]) -> Vec<Sample> {
    let mut out = Vec::new();
    for &triangle in triangles {
        let [a, b, c] = triangle.map(|corner| rest[corner as usize]);
        let steps = ((a.distance(b).max(b.distance(c)).max(c.distance(a)) / SPACING).ceil() as usize).clamp(1, 8);
        if steps == 1 {
            out.push((triangle, [1.0 / 3.0; 3]));
            continue;
        }
        for i in 0..=steps {
            for j in 0..=steps - i {
                let (u, v) = (i as f32 / steps as f32, j as f32 / steps as f32);
                out.push((triangle, [1.0 - u - v, u, v]));
            }
        }
    }
    out
}

impl Clearance {
    pub fn new(model: &Model, skinned: &Skinned) -> Result<Clearance, String> {
        let mut pose = model.rest();
        let hidden = model.menu.as_ref().map_or_else(HashSet::new, |menu| menu.apply(&menu.targets.iter().map(|target| target.default).collect::<Vec<_>>(), &mut pose));
        let mut drawn = vec![true; skinned.vertices.len()];
        for span in hidden.iter().filter_map(|node| skinned.spans.get(node)) {
            drawn[span.clone()].fill(false);
        }
        let palette = skinned.palette(&model.worlds(&model.rest()), Mat4::IDENTITY);
        let rest: Vec<Vec3> = (0..skinned.vertices.len()).map(|vertex| skinned.skin(&palette, vertex)).collect();
        let bone = |name: &str| model.humanoid.get(name).copied().ok_or(format!("the humanoid has no {name}"));
        let owner: Vec<usize> = skinned.vertices.iter().map(|vertex| {
            let (joint, _) = vertex.joints.iter().zip(vertex.weights).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
            skinned.joint_node(*joint)
        }).collect();
        let below = |node: usize, top: usize| {
            let mut at = Some(node);
            while let Some(here) = at {
                if here == top {
                    return true;
                }
                at = model.nodes[here].parent;
            }
            false
        };
        let hips = bone("hips")?;
        let mut sides = Vec::new();
        for (side, other) in [("left", "right"), ("right", "left")] {
            let (hip, knee, other_hip, forearm) = (bone(&format!("{side}UpperLeg"))?, bone(&format!("{side}LowerLeg"))?, bone(&format!("{other}UpperLeg"))?, bone(&format!("{side}LowerArm"))?);
            let rests = model.worlds(&model.rest());
            let (own, theirs) = (rests[hip].w_axis.truncate(), rests[other_hip].w_axis.truncate());
            let in_thigh = |vertex: usize| (below(owner[vertex], hip) && !below(owner[vertex], knee)) || (owner[vertex] == hips && rest[vertex].y < own.y && rest[vertex].distance_squared(own) < rest[vertex].distance_squared(theirs));
            let in_arm = |vertex: usize| below(owner[vertex], forearm);
            let mut thigh = Vec::new();
            let mut arm = Vec::new();
            for corners in skinned.indices.chunks_exact(3) {
                let triangle = [corners[0], corners[1], corners[2]];
                if !triangle.iter().all(|&corner| drawn[corner as usize]) {
                    continue;
                }
                if triangle.iter().all(|&corner| in_thigh(corner as usize)) {
                    thigh.push(triangle);
                } else if triangle.iter().all(|&corner| in_arm(corner as usize)) {
                    arm.push(triangle);
                }
            }
            if thigh.is_empty() || arm.is_empty() {
                return Err(format!("no drawn {side} {} skin to measure", if thigh.is_empty() { "thigh" } else { "forearm or hand" }));
            }
            sides.push(Side { thigh: samples(&thigh, &rest), arm: samples(&arm, &rest) });
        }
        let mut vertices: Vec<usize> = sides.iter().flat_map(|side| side.thigh.iter().chain(&side.arm)).flat_map(|(triangle, _)| triangle.map(|corner| corner as usize)).collect();
        vertices.sort_unstable();
        vertices.dedup();
        Ok(Clearance { sides, vertices })
    }

    /// How deep, in metres, the forearms and hands sink into the thighs in `pose`: the deepest sample but a few strays.
    pub fn depth(&self, model: &Model, skinned: &Skinned, humanoid: &Humanoid, spread: f32) -> f32 {
        let pose = model.pose_spread(humanoid, spread);
        let worlds = model.worlds(&pose);
        let palette = skinned.palette(&worlds, Mat4::IDENTITY);
        let mut posed = vec![Vec3::ZERO; skinned.vertices.len()];
        for &vertex in &self.vertices {
            posed[vertex] = skinned.skin(&palette, vertex);
        }
        let point = |(triangle, weights): &Sample| triangle.iter().zip(weights).fold(Vec3::ZERO, |sum, (&corner, &weight)| sum + posed[corner as usize] * weight);
        let skin: Vec<(Vec3, Vec3)> = self.sides.iter().flat_map(|side| &side.thigh).map(|(triangle, weights)| {
            let [a, b, c] = triangle.map(|corner| posed[corner as usize]);
            (a * weights[0] + b * weights[1] + c * weights[2], (b - a).cross(c - a).normalize_or_zero())
        }).collect();
        let grid = Grid::new(&skin);
        let mut depths = Vec::new();
        for side in &self.sides {
            for sample in &side.arm {
                let point = point(sample);
                let nearest = grid.nearest(&skin, point);
                if let Some((skin, normal)) = nearest {
                    let depth = (skin - point).dot(normal);
                    if depth > 0.0 {
                        depths.push(depth);
                    }
                }
            }
        }
        if depths.len() <= STRAYS {
            return 0.0;
        }
        depths.select_nth_unstable_by(STRAYS, |a, b| b.total_cmp(a));
        depths[STRAYS]
    }

    /// Each sampled frame's depth over `clip` played alone over the standing pose, as the overlay plays an idle or the walk.
    pub fn over(&self, model: &Model, skinned: &Skinned, standing: &Humanoid, clip: &Clip, spread: f32) -> Vec<f32> {
        let frames = (clip.duration() * RATE).ceil().max(1.0) as usize;
        (0..frames)
            .map(|frame| {
                let sampled = clip.sample(frame as f32 / RATE);
                let mut humanoid = standing.clone();
                humanoid.rotations.extend(sampled.rotations);
                humanoid.hips = sampled.hips;
                self.depth(model, skinned, &humanoid, spread)
            })
            .collect()
    }
}

/// The least spread, to `RESOLUTION`, that keeps every frame of each of `clips` within `TOUCH`, or None for a clip no spread up to `WIDEST` keeps there: one whose pose lays the hands on the body.
pub fn needs(model: &Model, skinned: &Skinned, clips: &[&Clip]) -> Result<Vec<Option<f32>>, String> {
    let clearance = Clearance::new(model, skinned)?;
    let standing = model.standing();
    let need = |clip: &Clip| {
        let clear = |angle: f32| clearance.over(model, skinned, &standing, clip, angle).iter().all(|&depth| depth <= TOUCH);
        if clear(0.0) {
            return Some(0.0);
        }
        if !clear(WIDEST) {
            return None;
        }
        let (mut low, mut high) = (0.0, WIDEST);
        while high - low > RESOLUTION {
            let middle = (low + high) / 2.0;
            if clear(middle) { high = middle } else { low = middle }
        }
        Some(high)
    };
    Ok(std::thread::scope(|scope| {
        let runs: Vec<_> = clips.iter().map(|clip| scope.spawn(move || need(clip))).collect();
        runs.into_iter().map(|run| run.join().unwrap()).collect()
    }))
}

/// The figure's spread: the most any clip a spread can clear needs.
pub fn spread(needs: &[Option<f32>]) -> f32 {
    needs.iter().flatten().copied().fold(0.0, f32::max)
}

/// The digest of a figure file's binary chunk, which a spread is measured on.
pub fn body_of(bytes: &[u8]) -> Result<String, String> {
    let (json, binary) = super::chunks(bytes)?;
    Ok(body_digest(&json, binary))
}

/// Thigh skin samples binned in `REACH`-wide cells, for the nearest within `REACH` of a point.
struct Grid {
    low: glam::IVec3,
    size: glam::IVec3,
    starts: Vec<u32>,
    order: Vec<u32>,
}

impl Grid {
    fn cell(point: Vec3) -> glam::IVec3 {
        (point / REACH).floor().as_ivec3()
    }

    fn new(skin: &[(Vec3, Vec3)]) -> Grid {
        let cells: Vec<glam::IVec3> = skin.iter().map(|(point, _)| Grid::cell(*point)).collect();
        let low = cells.iter().copied().fold(glam::IVec3::MAX, glam::IVec3::min);
        let high = cells.iter().copied().fold(glam::IVec3::MIN, glam::IVec3::max);
        let size = if cells.is_empty() { glam::IVec3::ZERO } else { high - low + 1 };
        let index = |cell: glam::IVec3| { let at = cell - low; (at.x + size.x * (at.y + size.y * at.z)) as usize };
        let mut starts = vec![0u32; (size.x * size.y * size.z) as usize + 1];
        for &cell in &cells {
            starts[index(cell) + 1] += 1;
        }
        for at in 1..starts.len() {
            starts[at] += starts[at - 1];
        }
        let mut next = starts.clone();
        let mut order = vec![0u32; cells.len()];
        for (sample, &cell) in cells.iter().enumerate() {
            let slot = &mut next[index(cell)];
            order[*slot as usize] = sample as u32;
            *slot += 1;
        }
        Grid { low, size, starts, order }
    }

    fn nearest(&self, skin: &[(Vec3, Vec3)], point: Vec3) -> Option<(Vec3, Vec3)> {
        if skin.is_empty() {
            return None;
        }
        let centre = Grid::cell(point) - self.low;
        let mut best: Option<(f32, usize)> = None;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let at = centre + glam::IVec3::new(dx, dy, dz);
                    if at.cmplt(glam::IVec3::ZERO).any() || at.cmpge(self.size).any() {
                        continue;
                    }
                    let index = (at.x + self.size.x * (at.y + self.size.y * at.z)) as usize;
                    for &sample in &self.order[self.starts[index] as usize..self.starts[index + 1] as usize] {
                        let distance = skin[sample as usize].0.distance_squared(point);
                        if distance < REACH * REACH && best.is_none_or(|(nearest, _)| distance < nearest) {
                            best = Some((distance, sample as usize));
                        }
                    }
                }
            }
        }
        best.map(|(_, sample)| skin[sample])
    }
}

/// `bytes` carrying a spread of `angle` measured on its own body against `clips`.
#[cfg(test)]
pub fn measured(bytes: &[u8], angle: f32, clips: &str) -> Vec<u8> {
    let (json, binary) = super::chunks(bytes).unwrap();
    let spread = Spread { angle, body: body_digest(&json, binary), clips: clips.to_owned(), revision: REVISION };
    rewritten(bytes, |json| json["extensions"][EXTENSION] = spread.json())
}

/// `bytes` with its JSON changed by `edit` and its binary kept.
#[cfg(test)]
pub fn rewritten(bytes: &[u8], edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let (mut json, binary) = super::chunks(bytes).unwrap();
    edit(&mut json);
    let mut text = serde_json::to_vec(&json).unwrap();
    text.resize(text.len().next_multiple_of(4), b' ');
    let length = 12 + 8 + text.len() + if binary.is_empty() { 0 } else { 8 + binary.len() };
    let mut out = Vec::new();
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((length as u32).to_le_bytes());
    out.extend((text.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(text);
    if !binary.is_empty() {
        out.extend((binary.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(binary);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirrored(version: Version, turn: Quat) -> Quat {
        match version {
            Version::One => turn,
            Version::Zero => Quat::from_xyzw(-turn.x, turn.y, -turn.z, turn.w),
        }
    }

    #[test]
    fn a_spread_turns_a_lowered_arm_out_from_the_body_on_either_side_in_either_version() {
        for version in [Version::One, Version::Zero] {
            for (side, outward) in [("left", 1.0f32), ("right", -1.0)] {
                let facing = if version == Version::One { 1.0 } else { -1.0 };
                let rest = Vec3::X * outward * facing;
                let lowered = mirrored(version, Quat::from_rotation_z(-outward * 70f32.to_radians()));
                let hanging = lowered * rest;
                let spread = lowered * turn(version, side, 10f32.to_radians()) * rest;
                assert!(spread.x.abs() > hanging.x.abs() + 0.1, "{version:?} {side}: {hanging} → {spread}");
                assert!((hanging.angle_between(spread) - 10f32.to_radians()).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn a_spread_is_worn_only_as_measured_on_this_body_by_this_revision() {
        let binary = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let body = serde_json::json!({ "nodes": [{ "translation": [0.0, 1.0, 0.0] }], "accessors": [{ "bufferView": 0 }], "bufferViews": [{ "byteOffset": 0, "byteLength": 4 }, { "byteOffset": 4, "byteLength": 4 }], "images": [{ "bufferView": 1 }] });
        let spread = Spread { angle: 0.1, body: body_digest(&body, &binary), clips: "c".into(), revision: REVISION };
        let carrying = |spread: &Spread, body: &Value| { let mut json = body.clone(); json["extensions"][EXTENSION] = spread.json(); json };
        assert_eq!(parse(&carrying(&spread, &body), &binary).unwrap(), spread);
        assert_eq!(parse(&carrying(&spread, &body), &[1, 2, 3, 4, 0, 0, 0, 0]).unwrap(), spread, "an image is no part of the body");
        assert!(parse(&body, &binary).unwrap_err().contains("publish-avatar"));
        assert!(parse(&carrying(&spread, &body), &[0, 2, 3, 4, 5, 6, 7, 8]).unwrap_err().contains("another body"));
        for edit in [
            (|json: &mut Value| json["nodes"][0]["translation"][1] = 1.1.into()) as fn(&mut Value),
            |json| json["bufferViews"][0]["byteStride"] = 4.into(),
            |json| json["extensions"][super::super::expressions::EXTENSION] = serde_json::json!({ "targets": [] }),
        ] {
            let mut moved = body.clone();
            edit(&mut moved);
            assert!(parse(&carrying(&spread, &moved), &binary).unwrap_err().contains("another body"), "{moved}");
        }
        assert!(parse(&carrying(&Spread { revision: REVISION + 1, ..spread.clone() }, &body), &binary).unwrap_err().contains("revision"));
        assert!(parse(&carrying(&Spread { angle: 1.0, ..spread.clone() }, &body), &binary).is_err());
    }

    #[test]
    fn the_digest_of_the_clips_follows_their_names_and_sources_in_any_order() {
        let digest = clips_digest([("idle", "a"), ("walk", "b")]);
        assert_eq!(digest, clips_digest([("walk", "b"), ("idle", "a")]));
        assert_ne!(digest, clips_digest([("idle", "a"), ("walk", "c")]));
        assert_ne!(digest, clips_digest([("idle", "a")]));
    }
}

