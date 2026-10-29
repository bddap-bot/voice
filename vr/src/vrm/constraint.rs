use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

use super::{floats, index, number, rotation, Model, Pose};

pub const EXTENSION: &str = "UNITY_constraints";

pub(super) struct Source {
    node: Option<usize>,
    world: Mat4,
    offset: Mat4,
    weight: f32,
}

pub(super) struct Constraint {
    node: usize,
    kind: String,
    sources: Vec<Source>,
    weight: f32,
    translation: [bool; 3],
    rotation: [bool; 3],
    rest: Mat4,
    offset: Mat4,
}

fn matrix(value: &Value) -> Result<Mat4, String> {
    let values = value.as_array().filter(|values| values.len() == 16).ok_or("constraint matrix must have 16 numbers")?;
    if values.iter().any(|value| !value.as_f64().is_some_and(|n| n.is_finite())) {
        return Err("constraint matrix must be finite".into());
    }
    let m = Mat4::from_cols_array(&floats(value, Mat4::IDENTITY.to_cols_array()));
    if m.determinant().abs() < 1e-9 {
        return Err("constraint matrix must be invertible".into());
    }
    Ok(m)
}

pub(super) fn parse(value: Option<&Value>, nodes: usize) -> Result<Vec<Constraint>, String> {
    let Some(value) = value else { return Ok(Vec::new()) };
    let at = |value: &Value| index(value).filter(|&i| i < nodes).ok_or_else(|| "constraint node out of range".to_owned());
    let mask = |value: &Value| -> Result<[bool; 3], String> {
        let a = value.as_array().filter(|a| a.len() == 3).ok_or("constraint mask must have three booleans")?;
        Ok([a[0].as_bool().ok_or("constraint mask")?, a[1].as_bool().ok_or("constraint mask")?, a[2].as_bool().ok_or("constraint mask")?])
    };
    let mut constraints = Vec::new();
    for c in value["constraints"].as_array().ok_or("constraints must be an array")? {
        let kind = c["kind"].as_str().ok_or("constraint kind missing")?;
        if !matches!(kind, "Parent" | "Position" | "LookAt") {
            return Err(format!("unsupported constraint {kind}"));
        }
        let mut sources = Vec::new();
        for source in c["sources"].as_array().ok_or("constraint sources missing")? {
            let node = source.get("node").map(at).transpose()?;
            let world = if node.is_some() { Mat4::IDENTITY } else { matrix(&source["world"])? };
            let weight = number(&source["weight"], 0.0);
            if !weight.is_finite() || weight < 0.0 {
                return Err("invalid constraint source weight".into());
            }
            sources.push(Source { node, world, offset: matrix(&source["offset"])?, weight });
        }
        let weight = number(&c["weight"], 1.0);
        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
            return Err("invalid constraint weight".into());
        }
        constraints.push(Constraint { node: at(&c["node"])?, kind: kind.into(), sources, weight, translation: mask(&c["translation"])?, rotation: mask(&c["rotation"])?, rest: matrix(&c["rest"])?, offset: matrix(&c["offset"])? });
    }
    Ok(constraints)
}

#[derive(Default)]
pub struct Constraints {
    previous: HashMap<usize, Mat4>,
}

impl Constraints {
    pub fn reset(&mut self) {
        self.previous.clear();
    }

    pub fn step(&mut self, model: &Model, pose: &mut Pose, body: Mat4) {
        for c in &model.constraints {
            let worlds = model.worlds(pose);
            let parent = model.nodes[c.node].parent.map_or(body, |p| body * worlds[p]);
            let current = body * worlds[c.node];
            let mut position = Vec3::ZERO;
            let mut turn = Quat::IDENTITY;
            let mut total = 0.0;
            for source in &c.sources {
                if source.weight == 0.0 { continue; }
                // A self source reads the last evaluated world transform, not a new controller-relative rest pose.
                let world = source.node.map_or(source.world, |node| {
                    if node == c.node { self.previous.get(&node).copied().unwrap_or(body * worlds[node]) } else { body * worlds[node] }
                });
                let target = if c.kind == "Parent" { world * source.offset } else { world };
                position += target.w_axis.truncate() * source.weight;
                turn = turn.slerp(rotation(&target), source.weight / (total + source.weight));
                total += source.weight;
            }
            let rest = parent * c.rest;
            let (_, rest_turn, rest_position) = rest.to_scale_rotation_translation();
            let (scale, current_turn, current_position) = current.to_scale_rotation_translation();
            let mut target_position = rest_position;
            let mut target_turn = rest_turn;
            if total > 0.0 {
                position /= total;
                match c.kind.as_str() {
                    "Position" => target_position = position + c.offset.w_axis.truncate(),
                    "Parent" => { target_position = position; target_turn = turn; }
                    "LookAt" => {
                        if let Some(forward) = (position - current_position).try_normalize() {
                            if let Some(right) = Vec3::Y.cross(forward).try_normalize() {
                                target_turn = Quat::from_mat3(&glam::Mat3::from_cols(right, forward.cross(right), forward)) * rotation(&c.offset);
                            } else { target_turn = current_turn; }
                        } else { target_turn = current_turn; }
                    }
                    _ => unreachable!(),
                }
            }
            let p = if c.kind == "LookAt" { current_position } else { rest_position.lerp(target_position, c.weight) };
            let r = if c.kind == "Position" { current_turn } else { rest_turn.slerp(target_turn, c.weight) };
            let local = parent.inverse() * Mat4::from_scale_rotation_translation(scale, r, p);
            let (_, mut r, mut p) = local.to_scale_rotation_translation();
            if c.kind != "LookAt" {
                for axis in 0..3 {
                    if !c.translation[axis] { p[axis] = pose.translations[c.node][axis]; }
                }
            }
            if c.kind == "Parent" && c.rotation != [true, true, true] {
                let (y, x, z) = r.to_euler(glam::EulerRot::YXZ);
                let (old_y, old_x, old_z) = pose.rotations[c.node].to_euler(glam::EulerRot::YXZ);
                r = Quat::from_euler(glam::EulerRot::YXZ, if c.rotation[1] { y } else { old_y }, if c.rotation[0] { x } else { old_x }, if c.rotation[2] { z } else { old_z });
            }
            pose.translations[c.node] = p;
            pose.rotations[c.node] = r.normalize();
            self.previous.insert(c.node, parent * model.local(pose, c.node));
        }
    }
}

impl Model {
    pub fn constraint_motion(&self, worlds: &[Mat4], body: Mat4) -> Vec<(usize, String, Quat, Vec3)> {
        self.constraints.iter().map(|c| {
            let world = body * worlds[c.node];
            (c.node, self.nodes[c.node].name.clone().unwrap_or_default(), rotation(&world), world.w_axis.truncate())
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::tests::Builder;
    use serde_json::json;

    fn fixture() -> Model {
        let identity = Mat4::IDENTITY.to_cols_array();
        let constraint = |kind: &str, node: usize, sources: Value| json!({"kind": kind, "node": node, "sources": sources, "weight": 1.0, "translation": [true, true, true], "rotation": [true, true, true], "rest": identity, "offset": identity});
        let mut builder = Builder::new(json!({"VRMC_vrm": {"specVersion": "1.0", "humanoid": {"humanBones": {"hips": {"node": 1}}}}, EXTENSION: {"constraints": [
            constraint("Parent", 2, json!([{"world": identity, "offset": identity, "weight": 1.0}])),
            constraint("Position", 3, json!([{"node": 4, "offset": identity, "weight": 0.01}, {"node": 3, "offset": identity, "weight": 1.0}]))
        ]}}));
        let positions = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 0.1, 0.0]);
        let indices = builder.indices(&[0, 1, 2]);
        builder.set("nodes", json!([
            {"children": [1, 2, 4], "mesh": 0}, {}, {"children": [3]}, {"translation": [1.0, 0.0, 0.0]}, {"translation": [1.0, 0.0, 0.0]}
        ]));
        builder.set("meshes", json!([{"primitives": [{"attributes": {"POSITION": positions}, "indices": indices}]}]));
        Model::parse(&builder.glb()).unwrap()
    }

    #[test]
    fn world_anchor_stays_fixed_while_controller_moves_and_turns() {
        let model = fixture();
        let mut solver = Constraints::default();
        for angle in [0.0, 0.7, 1.3] {
            let body = Mat4::from_scale_rotation_translation(Vec3::splat(0.3), Quat::from_rotation_y(angle), Vec3::new(3.0, 2.0, -4.0));
            let mut pose = model.rest();
            solver.step(&model, &mut pose, body);
            let world = body * model.worlds(&pose)[2];
            assert!(world.w_axis.truncate().length() < 1e-5);
            assert!(rotation(&world).angle_between(Quat::IDENTITY) < 1e-5);
        }
    }

    #[test]
    fn weighted_self_source_follows_from_previous_world_position() {
        let model = fixture();
        let mut solver = Constraints::default();
        let mut pose = model.rest();
        solver.step(&model, &mut pose, Mat4::IDENTITY);
        let body = Mat4::from_translation(Vec3::X);
        for step in 1..=20 {
            let mut pose = model.rest();
            solver.step(&model, &mut pose, body);
            let x = (body * model.worlds(&pose)[3]).w_axis.x;
            let expected = 2.0 - (1.0_f32 / 1.01).powi(step);
            assert!((x - expected).abs() < 1e-5, "step {step}: {x} != {expected}");
        }
    }

    #[test]
    fn look_at_uses_weighted_source_position_without_moving_target() {
        let mut model = fixture();
        let identity = Mat4::IDENTITY.to_cols_array();
        model.constraints = parse(Some(&json!({"constraints": [{"kind": "LookAt", "node": 3, "sources": [{"node": 4, "weight": 1.0, "offset": identity}], "weight": 1.0, "translation": [true, true, true], "rotation": [true, true, true], "rest": identity, "offset": identity}]})), model.nodes()).unwrap();
        let mut pose = model.rest();
        pose.translations[4] = Vec3::new(2.0, 0.0, 1.0);
        let before = model.worlds(&pose)[3].w_axis.truncate();
        Constraints::default().step(&model, &mut pose, Mat4::IDENTITY);
        let worlds = model.worlds(&pose);
        let desired = (worlds[4].w_axis - worlds[3].w_axis).truncate().normalize();
        assert!((worlds[3].w_axis.truncate() - before).length() < 1e-6);
        assert!((rotation(&worlds[3]) * Vec3::Z - desired).length() < 1e-6);
    }

    #[test]
    fn invalid_source_cannot_silently_drop_constraint() {
        assert!(parse(Some(&json!({"constraints": [{"kind": "Position", "node": 2, "sources": [{"node": 999}]}]})), 5).is_err());
    }
}
