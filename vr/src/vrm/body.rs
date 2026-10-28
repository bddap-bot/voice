use std::collections::HashSet;

use glam::{Mat4, Quat, Vec3};

use super::{rotation, Model, Pose, Skinned};

/// The humanoid bones whose flesh can meet another's, each after the bone it hangs from.
const PARTS: [&str; 18] = [
    "hips", "spine", "chest", "upperChest", "neck", "head",
    "leftUpperArm", "leftLowerArm", "leftHand", "rightUpperArm", "rightLowerArm", "rightHand",
    "leftUpperLeg", "leftLowerLeg", "leftFoot", "rightUpperLeg", "rightLowerLeg", "rightFoot",
];
const TORSO: [&str; 4] = ["hips", "spine", "chest", "upperChest"];
/// How readily each arm joint gives way: a wrist turned aside reads as wrong sooner than a shoulder or elbow.
const ARM: [(&str, f32); 3] = [("UpperArm", 1.0), ("LowerArm", 1.0), ("Hand", 0.3)];
/// The most any joint turns in one pass, radians.
const STEP: f32 = 0.35;
/// The most any joint turns in all, radians: a pose turned further reads as more wrong than the contact it would clear.
const MOST: f32 = 0.8;
/// The fastest a correction changes, radians a second, so it never pops.
const RATE: f32 = 4.0;
/// Overlap the solver leaves alone, metres: a capsule is a rough body, and chasing less only moves the pose.
const SLACK: f32 = 0.002;
const PASSES: usize = 6;
/// The widest gap left between kept skin points, metres, and the most kept per part.
const SPACING: f32 = 0.025;
const SAMPLES: usize = 512;
/// How far past the skin points a push carries, metres, for the skin between them.
const MARGIN: f32 = 0.006;
/// How long a correction takes to ease back toward the authored pose once nothing holds it out, seconds.
const RELEASE: f32 = 0.25;

struct Pair {
    a: usize,
    b: usize,
    /// How deep each skin point of `a`, then of `b`, already lies in the other's flesh at rest or standing: contact the mesh itself makes, which no pose is blamed for.
    allowed: [Vec<f32>; 2],
}

struct Placed {
    axes: Vec<[Vec3; 2]>,
    skin: Vec<Vec<Vec3>>,
}

/// One part of the figure as a capsule fixed to its bone.
struct Part {
    name: &'static str,
    node: usize,
    parent: Option<usize>,
    /// The capsule's axis in the bone's frame.
    ends: [Vec3; 2],
    /// Half the skin lies within this of the axis: the flesh another part may not enter.
    radius: f32,
    /// Nearly all the skin lies within this: how far the part's own surface reaches.
    outer: f32,
    /// Its bone's place in `Body::nodes`.
    slot: usize,
    /// Points spread over its skin, each in the frame of the joint that moves it most (as a finger's), by that joint's place in `Body::nodes`: what may not enter another part's flesh.
    skin: Vec<(usize, Vec3)>,
    /// The arm joints, shoulder first, whose turn moves this part, each with how readily it gives; empty for the torso, head and legs, which keep their pose.
    chain: Vec<(usize, f32)>,
}

/// Collision proxies fitted to a figure's own skin, and the pose change that keeps its arms out of its body and each other.
pub struct Body {
    parts: Vec<Part>,
    /// The nodes the parts and their skin points hang from.
    nodes: Vec<usize>,
    /// Parts that do not join and so may not overlap.
    pairs: Vec<Pair>,
    /// Each vertex's part: the part of the bone that moves it most, unless a spring joint lies between, as on hair and skirts.
    pub owners: Vec<Option<usize>>,
}

/// The closest points of segments `a` and `b`.
fn closest(a: [Vec3; 2], b: [Vec3; 2]) -> (Vec3, Vec3) {
    let (u, v, w) = (a[1] - a[0], b[1] - b[0], a[0] - b[0]);
    let (uu, uv, vv, uw, vw) = (u.dot(u), u.dot(v), v.dot(v), u.dot(w), v.dot(w));
    let denominator = uu * vv - uv * uv;
    let mut s = if denominator > 1e-12 { ((uv * vw - vv * uw) / denominator).clamp(0.0, 1.0) } else { 0.0 };
    let t = if vv > 1e-12 { ((uv * s + vw) / vv).clamp(0.0, 1.0) } else { 0.0 };
    if uu > 1e-12 {
        s = ((uv * t - uw) / uu).clamp(0.0, 1.0);
    }
    (a[0] + u * s, b[0] + v * t)
}

/// The distance from `point` to the segment `ends`.
pub fn from_axis(point: Vec3, ends: [Vec3; 2]) -> f32 {
    point.distance(on_axis(point, ends))
}

fn limit(turn: Quat) -> Quat {
    let angle = turn.angle_between(Quat::IDENTITY);
    if angle > MOST { Quat::IDENTITY.slerp(turn, MOST / angle) } else { turn }
}

fn on_axis(point: Vec3, ends: [Vec3; 2]) -> Vec3 {
    let axis = ends[1] - ends[0];
    let along = if axis.length_squared() > 1e-12 { ((point - ends[0]).dot(axis) / axis.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    ends[0] + axis * along
}

/// The indices of `points` thinned until every one lies within `spacing` of a kept one, or `count` are kept, each kept point the farthest from those before it.
fn spread(points: &[Vec3], spacing: f32, count: usize) -> Vec<usize> {
    let Some(&first) = points.first() else { return Vec::new() };
    let mut kept = vec![0];
    let mut nearest: Vec<f32> = points.iter().map(|point| point.distance_squared(first)).collect();
    while kept.len() < count {
        let (far, gap) = nearest.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).unwrap();
        if *gap <= spacing * spacing {
            break;
        }
        kept.push(far);
        for (distance, point) in nearest.iter_mut().zip(points) {
            *distance = distance.min(point.distance_squared(points[far]));
        }
    }
    kept
}

fn quantile(mut values: Vec<f32>, at: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    values[((values.len() - 1) as f32 * at).round() as usize]
}

/// A rigid motion about a pivot, applied after the parent's.
fn about(pivot: Vec3, turn: Quat) -> Mat4 {
    Mat4::from_translation(pivot) * Mat4::from_quat(turn) * Mat4::from_translation(-pivot)
}

impl Body {
    pub fn fit(model: &Model, skinned: &Skinned) -> Body {
        let rest = model.rest();
        let worlds = model.worlds(&rest);
        let present: Vec<(&'static str, usize)> = PARTS.iter().filter_map(|&name| Some((name, *model.humanoid.get(name)?))).collect();
        let springs: HashSet<usize> = model.rig.nodes().collect();
        let mut owner_of: Vec<Option<Option<usize>>> = vec![None; model.nodes.len()];
        for (index, &(_, node)) in present.iter().enumerate() {
            owner_of[node] = Some(Some(index));
        }
        fn owner(model: &Model, springs: &HashSet<usize>, memo: &mut [Option<Option<usize>>], node: usize) -> Option<usize> {
            if let Some(known) = memo[node] {
                return known;
            }
            let found = if springs.contains(&node) { None } else { model.nodes[node].parent.and_then(|parent| owner(model, springs, memo, parent)) };
            memo[node] = Some(found);
            found
        }
        let positions = skinned.positions(&skinned.palette(&worlds, Mat4::IDENTITY));
        let owners: Vec<Option<usize>> = skinned
            .vertices
            .iter()
            .map(|vertex| {
                let (slot, _) = vertex.joints.iter().zip(vertex.weights).max_by(|a, b| a.1.total_cmp(&b.1))?;
                owner(model, &springs, &mut owner_of, skinned.joint_node(*slot))
            })
            .collect();
        let mut flesh: Vec<Vec<Vec3>> = vec![Vec::new(); present.len()];
        let mut holders: Vec<Vec<usize>> = vec![Vec::new(); present.len()];
        for ((position, part), vertex) in positions.iter().zip(&owners).zip(&skinned.vertices) {
            if let Some(part) = part {
                flesh[*part].push(*position);
                let (slot, _) = vertex.joints.iter().zip(vertex.weights).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
                holders[*part].push(skinned.joint_node(*slot));
            }
        }
        let mut nodes: Vec<usize> = Vec::new();
        let mut slot = |node: usize| nodes.iter().position(|&known| known == node).unwrap_or_else(|| {
            nodes.push(node);
            nodes.len() - 1
        });
        let parents: Vec<Option<usize>> = present.iter().map(|&(_, node)| model.nodes[node].parent.and_then(|parent| owner(model, &springs, &mut owner_of, parent))).collect();
        let at = |node: usize| worlds[node].w_axis.truncate();
        let mut parts = Vec::new();
        for (index, &(name, node)) in present.iter().enumerate() {
            let origin = at(node);
            let points = &flesh[index];
            let child = (0..present.len()).find(|&other| parents[other] == Some(index));
            let ends = if TORSO.contains(&name) {
                let frame = model.frame(&worlds, node);
                let local: Vec<Vec3> = points.iter().map(|point| frame.inverse() * (*point - origin)).collect();
                let middle = Vec3::new(quantile(local.iter().map(|p| p.x).collect(), 0.5), quantile(local.iter().map(|p| p.y).collect(), 0.5), quantile(local.iter().map(|p| p.z).collect(), 0.5));
                let depth = quantile(local.iter().map(|p| (p.z - middle.z).abs()).collect(), 0.7);
                let width = quantile(local.iter().map(|p| (p.x - middle.x).abs()).collect(), 0.9);
                let side = Vec3::X * (width - depth).max(0.0);
                ([middle - side, middle + side].map(|end| origin + frame * end), depth)
            } else {
                let (tip, open) = match child {
                    Some(child) => (at(present[child].1), false),
                    None if points.is_empty() => (origin, true),
                    None => {
                        let mean = points.iter().copied().sum::<Vec3>() / points.len() as f32;
                        let direction = (mean - origin).normalize_or_zero();
                        (origin + direction * quantile(points.iter().map(|point| (*point - origin).dot(direction)).collect(), 0.9), true)
                    }
                };
                let axis = tip - origin;
                let length = axis.length();
                let around: Vec<f32> = points
                    .iter()
                    .filter_map(|point| {
                        let along = if length > 0.0 { (*point - origin).dot(axis) / (length * length) } else { 0.0 };
                        (0.0..=1.0).contains(&along).then(|| (*point - origin - axis * along).length())
                    })
                    .collect();
                let radius = quantile(around, 0.5);
                let tip = if open && length > 0.0 { origin + axis * ((length - radius).max(0.0) / length) } else { tip };
                ([origin, tip], radius)
            };
            let (ends, radius) = ends;
            let outer = quantile(points.iter().map(|point| from_axis(*point, ends)).collect(), 0.98).max(radius);
            let side = ["left", "right"].into_iter().find(|side| name.starts_with(side));
            let chain: Vec<(usize, f32)> = match side {
                Some(side) if name.contains("Arm") || name.ends_with("Hand") => ARM.iter().filter_map(|&(bone, give)| Some((present.iter().position(|&(other, _)| *other == format!("{side}{bone}"))?, give))).filter(|&(joint, _)| joint <= index).collect(),
                _ => Vec::new(),
            };
            let skin = spread(points, SPACING, SAMPLES).into_iter().map(|at| (slot(holders[index][at]), worlds[holders[index][at]].inverse().transform_point3(points[at]))).collect();
            parts.push(Part { name, node, slot: slot(node), parent: parents[index], ends: ends.map(|end| worlds[node].inverse().transform_point3(end)), radius, outer, skin, chain });
        }
        let mut body = Body { parts, nodes, pairs: Vec::new(), owners };
        // A part with no skin of its own, as a bare neck, is a joint: the parts either side of it join.
        let adjacent = |a: usize, b: usize| {
            let up = |part: usize| body.parts[part].parent;
            let joins = |child: usize, parent: usize| up(child) == Some(parent) || up(child).is_some_and(|between| body.parts[between].skin.is_empty() && up(between) == Some(parent));
            joins(a, b) || joins(b, a)
        };
        let pairs: Vec<(usize, usize)> = (0..body.parts.len()).flat_map(|a| (a + 1..body.parts.len()).map(move |b| (a, b))).filter(|&(a, b)| !adjacent(a, b)).collect();
        let standing = model.pose(&model.standing());
        let baselines = [rest, standing].map(|pose| {
            let worlds = model.worlds(&pose);
            body.place(&body.nodes.iter().map(|&node| worlds[node]).collect::<Vec<_>>(), &vec![Mat4::IDENTITY; body.parts.len()])
        });
        body.pairs = pairs
            .into_iter()
            .map(|(a, b)| {
                let inside = |from: usize, into: usize| -> Vec<f32> {
                    (0..body.parts[from].skin.len()).map(|point| baselines.iter().map(|placed| body.parts[into].radius - from_axis(placed.skin[from][point], placed.axes[into])).fold(0.0, f32::max)).collect()
                };
                Pair { a, b, allowed: [inside(a, b), inside(b, a)] }
            })
            .collect();
        body
    }

    /// Each part's capsule axis in the model's space, with its flesh radius.
    pub fn capsules(&self, worlds: &[Mat4]) -> Vec<([Vec3; 2], f32)> {
        self.parts.iter().map(|part| (part.ends.map(|end| worlds[part.node].transform_point3(end)), part.radius)).collect()
    }

    /// Each part's axis and skin points, `worlds` holding each of `nodes` as posed and `motions` each part's correction.
    fn place(&self, worlds: &[Mat4], motions: &[Mat4]) -> Placed {
        Placed {
            axes: self.parts.iter().zip(motions).map(|(part, motion)| part.ends.map(|end| (*motion * worlds[part.slot]).transform_point3(end))).collect(),
            skin: self.parts.iter().zip(motions).map(|(part, motion)| part.skin.iter().map(|&(slot, point)| (*motion * worlds[slot]).transform_point3(point)).collect()).collect(),
        }
    }

    /// The deepest any skin point of `a` lies in the flesh of `b`, or of `b` in `a`, past how deep it lies there at rest or standing, with a point on each whose difference is the way apart; None when their capsules cannot meet.
    fn contact(&self, placed: &Placed, pair: &Pair) -> Option<(f32, Vec3, Vec3)> {
        let (a, b) = (pair.a, pair.b);
        let (axis_a, axis_b) = (placed.axes[a], placed.axes[b]);
        let (p, q) = closest(axis_a, axis_b);
        let (part_a, part_b) = (&self.parts[a], &self.parts[b]);
        if p.distance(q) >= (part_a.outer + part_b.radius).max(part_a.radius + part_b.outer) {
            return None;
        }
        let deepest = |skin: &[Vec3], allowed: &[f32], axis: [Vec3; 2], radius: f32| skin.iter().zip(allowed).map(|(point, allowed)| (radius - from_axis(*point, axis) - allowed, *point)).max_by(|x, y| x.0.total_cmp(&y.0));
        // Pushed along the line between the axes, not each point's own way out: an arm sunk past the body's axis would otherwise be pulled both ways at once.
        let apart = p - q;
        let into_b = deepest(&placed.skin[a], &pair.allowed[0], axis_b, part_b.radius).map(|(depth, point)| (depth, point, point - apart));
        let into_a = deepest(&placed.skin[b], &pair.allowed[1], axis_a, part_a.radius).map(|(depth, point)| (depth, point + apart, point));
        [into_b, into_a].into_iter().flatten().max_by(|x, y| x.0.total_cmp(&y.0))
    }

    /// Whether the figure has any part this body would move.
    pub fn moves(&self) -> bool {
        self.parts.iter().any(|part| !part.chain.is_empty())
    }

    /// Whether parts `a` and `b` are kept apart: they do not join.
    pub fn apart(&self, a: usize, b: usize) -> bool {
        self.pairs.iter().any(|pair| (pair.a, pair.b) == (a.min(b), a.max(b)))
    }

    pub fn parts(&self) -> usize {
        self.parts.len()
    }

    pub fn name(&self, part: usize) -> &'static str {
        self.parts[part].name
    }

    /// Each part's capsule at rest, its parent and its vertex count, one line each.
    pub fn describe(&self, model: &Model) -> String {
        let worlds = model.worlds(&model.rest());
        let capsules = self.capsules(&worlds);
        self.parts
            .iter()
            .enumerate()
            .map(|(index, part)| {
                let ends = capsules[index].0.map(|end| format!("[{:.3} {:.3} {:.3}]", end.x, end.y, end.z));
                format!("{} under {:?}: {} vertices, {} to {} radius {:.3} outer {:.3}, {} skin points, chain {:?}\n", part.name, part.parent.map(|parent| self.parts[parent].name), self.owners.iter().filter(|owner| **owner == Some(index)).count(), ends[0], ends[1], part.radius, part.outer, part.skin.len(), part.chain)
            })
            .collect()
    }

    /// Turns the arms by the least angles that take each out of the body, the head, the legs and each other, no deeper than its skin already meets at rest or standing.
    /// Starts from the turns `held` from the last frame, eased back toward the authored pose over `RELEASE`, so a correction follows the motion instead of jumping between ways out; returns the largest turn, radians.
    pub fn separate(&self, model: &Model, pose: &mut Pose, held: &mut Held, delta: f32) -> f32 {
        if !self.moves() {
            return 0.0;
        }
        let count = self.parts.len();
        if held.turns.len() != count {
            held.turns = vec![Quat::IDENTITY; count];
        }
        let worlds: Vec<Mat4> = self.nodes.iter().map(|&node| model.world(pose, node)).collect();
        let origins: Vec<Vec3> = self.parts.iter().map(|part| worlds[part.slot].w_axis.truncate()).collect();
        let parents: Vec<Quat> = self.parts.iter().map(|part| model.nodes[part.node].parent.map_or(Quat::IDENTITY, |parent| rotation(&model.world(pose, parent)))).collect();
        let motions = |turns: &[Quat]| {
            let mut motions: Vec<Mat4> = Vec::with_capacity(count);
            for (index, part) in self.parts.iter().enumerate() {
                let above = part.parent.map_or(Mat4::IDENTITY, |parent| motions[parent]);
                motions.push(if turns[index] == Quat::IDENTITY { above } else { about(above.transform_point3(origins[index]), turns[index]) * above });
            }
            motions
        };
        // A turn in the frame of its joint's parent, as it was moved by the turns above it.
        let frame = |motions: &[Mat4], index: usize| self.parts[index].parent.map_or(Quat::IDENTITY, |parent| rotation(&motions[parent])) * parents[index];
        let start = |locals: &[Quat]| {
            let mut turns = Vec::with_capacity(count);
            let mut motion: Vec<Mat4> = Vec::with_capacity(count);
            for (index, part) in self.parts.iter().enumerate() {
                let above = part.parent.map_or(Mat4::IDENTITY, |parent| motion[parent]);
                let turn = if locals[index] == Quat::IDENTITY { Quat::IDENTITY } else { frame(&motion, index) * locals[index] * frame(&motion, index).inverse() };
                motion.push(if turn == Quat::IDENTITY { above } else { about(above.transform_point3(origins[index]), turn) * above });
                turns.push(turn);
            }
            (turns, motion)
        };
        let placed = |motions: &[Mat4]| self.place(&worlds, motions);
        let contacts = |placed: &Placed| -> Vec<(usize, usize, f32, Vec3, Vec3)> {
            self.pairs
                .iter()
                .filter(|pair| !self.parts[pair.a].chain.is_empty() || !self.parts[pair.b].chain.is_empty())
                .filter_map(|pair| {
                    let (depth, p, q) = self.contact(placed, pair)?;
                    (depth > SLACK).then_some((pair.a, pair.b, depth, p, q))
                })
                .collect()
        };
        let total = |contacts: &[(usize, usize, f32, Vec3, Vec3)]| contacts.iter().map(|contact| contact.2).sum::<f32>();
        let keep = (-delta / RELEASE).exp();
        let eased: Vec<Quat> = held.turns.iter().map(|turn| Quat::IDENTITY.slerp(*turn, keep)).collect();
        let (mut turns, mut motion) = start(&eased);
        let mut found = contacts(&placed(&motion));
        if !found.is_empty() && held.turns != eased {
            // Easing off would let the contact back in: keep last frame's correction rather than ease in and push out again, which would shimmer.
            let (kept_turns, kept_motion) = start(&held.turns);
            let kept_found = contacts(&placed(&kept_motion));
            if kept_found.is_empty() {
                (turns, motion, found) = (kept_turns, kept_motion, kept_found);
            }
        }
        for _ in 0..PASSES {
            if found.is_empty() {
                break;
            }
            let mut steps = vec![Vec3::ZERO; count];
            for &(a, b, excess, p, q) in &found {
                let apart = (p - q).try_normalize().unwrap_or_else(|| {
                    let axes = placed(&motion).axes;
                    let centre = |part: usize| (axes[part][0] + axes[part][1]) / 2.0;
                    (centre(a) - centre(b)).try_normalize().unwrap_or(Vec3::X)
                });
                let moving = [a, b].iter().filter(|&&part| !self.parts[part].chain.is_empty()).count() as f32;
                for (part, at, push) in [(a, p, apart), (b, q, -apart)] {
                    let share = 1.0 / moving;
                    let levers: Vec<(usize, f32, Vec3)> = self.parts[part].chain.iter().map(|&(joint, give)| (joint, give, (at - motion[joint].transform_point3(origins[joint])).cross(push))).collect();
                    let sum: f32 = levers.iter().map(|(_, give, lever)| give * lever.length_squared()).sum();
                    if sum < 1e-10 {
                        continue;
                    }
                    for (joint, give, lever) in levers {
                        steps[joint] += lever * (give * (excess + MARGIN) * share / sum);
                    }
                }
            }
            let mut scale = 1.0;
            let before = total(&found);
            let accepted = loop {
                let tried: Vec<Quat> = turns.iter().zip(&steps).map(|(turn, step)| if *step == Vec3::ZERO { *turn } else { limit((Quat::from_scaled_axis((*step * scale).clamp_length_max(STEP)) * *turn).normalize()) }).collect();
                let tried_motion = motions(&tried);
                let tried_found = contacts(&placed(&tried_motion));
                if total(&tried_found) < before {
                    break Some((tried, tried_motion, tried_found));
                }
                scale /= 2.0;
                if scale < 0.1 {
                    break None;
                }
            };
            let Some((next, next_motion, next_found)) = accepted else { break };
            (turns, motion, found) = (next, next_motion, next_found);
        }
        let mut largest: f32 = 0.0;
        for (index, part) in self.parts.iter().enumerate() {
            if turns[index] == Quat::IDENTITY && held.turns[index] == Quat::IDENTITY {
                continue;
            }
            let solved = (frame(&motion, index).inverse() * turns[index] * frame(&motion, index)).normalize();
            let change = held.turns[index].angle_between(solved);
            let local = if change > RATE * delta { held.turns[index].slerp(solved, RATE * delta / change) } else { solved };
            held.turns[index] = local;
            pose.rotations[part.node] = (local * pose.rotations[part.node]).normalize();
            largest = largest.max(local.angle_between(Quat::IDENTITY));
        }
        largest
    }
}

/// The correction a figure carries from one frame to the next: each part's turn in its parent's frame.
#[derive(Default)]
pub struct Held {
    turns: Vec<Quat>,
}

impl Model {
    /// One node's place in the model's space, walking up its parents alone.
    pub fn world(&self, pose: &Pose, node: usize) -> Mat4 {
        let local = self.local(pose, node);
        self.nodes[node].parent.map_or(local, |parent| self.world(pose, parent) * local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::Humanoid;

    const FIGURE: &[u8] = include_bytes!("../../golden/figure.vrm");

    fn deepest(body: &Body, model: &Model, pose: &Pose) -> f32 {
        let worlds = model.worlds(pose);
        let placed = body.place(&body.nodes.iter().map(|&node| worlds[node]).collect::<Vec<_>>(), &vec![Mat4::IDENTITY; body.parts.len()]);
        body.pairs.iter().filter(|pair| !body.parts[pair.a].chain.is_empty() || !body.parts[pair.b].chain.is_empty()).filter_map(|pair| Some(body.contact(&placed, pair)?.0)).fold(0.0, f32::max)
    }

    #[test]
    fn closest_points_of_crossing_and_parallel_segments() {
        let (p, q) = closest([Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)], [Vec3::new(0.0, -1.0, 1.0), Vec3::new(0.0, 1.0, 1.0)]);
        assert!(p.abs_diff_eq(Vec3::ZERO, 1e-6) && q.abs_diff_eq(Vec3::Z, 1e-6));
        let (p, q) = closest([Vec3::ZERO, Vec3::X], [Vec3::new(2.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.0)]);
        assert!(p.abs_diff_eq(Vec3::X, 1e-6) && q.abs_diff_eq(Vec3::new(2.0, 1.0, 0.0), 1e-6));
    }

    #[test]
    fn a_standing_figure_is_left_as_it_stands() {
        let model = Model::parse(FIGURE).unwrap();
        let body = Body::fit(&model, &model.skinned().unwrap());
        assert!(body.moves(), "the figure has arms to move");
        let mut pose = model.pose(&model.standing());
        let before = pose.rotations.clone();
        assert_eq!(body.separate(&model, &mut pose, &mut Held::default(), 1.0 / 90.0), 0.0);
        assert_eq!(pose.rotations, before);
    }

    fn through(model: &Model) -> Humanoid {
        let mut humanoid = model.standing();
        let across = Quat::from_rotation_y(-1.9) * Quat::from_rotation_z(-1.2);
        humanoid.rotations.insert("leftUpperArm".into(), across);
        humanoid.rotations.insert("leftLowerArm".into(), Quat::from_rotation_y(-2.2));
        humanoid
    }

    #[test]
    fn an_arm_swung_through_the_chest_comes_out_by_a_small_turn() {
        let model = Model::parse(FIGURE).unwrap();
        let body = Body::fit(&model, &model.skinned().unwrap());
        let authored = model.pose(&through(&model));
        let through = deepest(&body, &model, &authored);
        assert!(through > 0.03, "the authored arm sinks {through} m into the figure");
        let mut held = Held::default();
        let (mut pose, mut turned) = (authored.clone(), 0.0);
        for _ in 0..45 {
            pose = authored.clone();
            turned = body.separate(&model, &mut pose, &mut held, 1.0 / 90.0);
        }
        let left = deepest(&body, &model, &pose);
        assert!(left < 0.01, "{left} m of overlap is left after turning {turned} rad");
        assert!(turned < 1.2, "the arm turns no further than it must: {turned} rad");
        let still: Vec<usize> = ["hips", "spine", "chest", "head", "leftUpperLeg", "rightUpperArm"].iter().map(|bone| model.humanoid[*bone]).collect();
        assert!(still.iter().all(|&node| pose.rotations[node] == authored.rotations[node]), "only the swung arm turns");
    }

    #[test]
    fn a_correction_holds_through_the_contact_and_eases_off_after_it() {
        let model = Model::parse(FIGURE).unwrap();
        let body = Body::fit(&model, &model.skinned().unwrap());
        let mut held = Held::default();
        let frame = 1.0 / 90.0;
        let mut turns = Vec::new();
        let mut pose = model.pose(&through(&model));
        for _ in 0..90 {
            pose = model.pose(&through(&model));
            turns.push(body.separate(&model, &mut pose, &mut held, frame));
        }
        assert!(turns.windows(2).all(|pair| (pair[1] - pair[0]).abs() <= RATE * frame + 1e-4), "it turns no faster than its rate: {turns:?}");
        assert!(deepest(&body, &model, &pose) < 0.01, "and clears the contact within a second");
        assert!(turns.windows(2).skip(70).all(|pair| (pair[1] - pair[0]).abs() < 0.002), "a held contact settles and keeps its correction: {turns:?}");
        let standing = model.pose(&model.standing());
        let mut pose = standing.clone();
        let first = body.separate(&model, &mut pose, &mut held, frame);
        assert!(first > 0.5 * turns[89], "the correction does not snap off: {first} after {}", turns[89]);
        for _ in 0..(8.0 * RELEASE / frame) as usize {
            pose = standing.clone();
            body.separate(&model, &mut pose, &mut held, frame);
        }
        assert!(pose.rotations.iter().zip(&standing.rotations).all(|(eased, authored)| eased.angle_between(*authored) < 0.01), "it eases back to the authored pose");
    }
}
