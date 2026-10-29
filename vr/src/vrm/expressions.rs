use std::collections::{HashMap, HashSet};

use serde::Deserialize;

pub const EXTENSION: &str = "VRCHAT_expressions";
/// Instant transitions followed in one layer in one frame, so a cycle of them cannot hold the frame.
const HOPS: usize = 8;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Button,
    Toggle,
    Submenu,
    Radial,
    TwoAxis,
    FourAxis,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Control {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    #[serde(default)]
    pub parameter: String,
    #[serde(default = "one")]
    pub value: f32,
    #[serde(default)]
    pub axes: Vec<String>,
    #[serde(default)]
    pub controls: Vec<Control>,
    pub icon: Option<usize>,
}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ValueType {
    Bool,
    Int,
    Float,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ValueType,
    #[serde(default)]
    pub default: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Mode {
    If,
    IfNot,
    Greater,
    Less,
    Equals,
    NotEqual,
}

#[derive(Clone, Debug, Deserialize)]
struct Condition {
    parameter: String,
    mode: Mode,
    #[serde(default)]
    threshold: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transition {
    to: i64,
    #[serde(default)]
    conditions: Vec<Condition>,
    exit_time: Option<f32>,
    #[serde(default)]
    not_to_self: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Drive {
    Set,
    Add,
    Random,
    Copy,
}

#[derive(Clone, Debug, Deserialize)]
struct Driver {
    parameter: String,
    drive: Drive,
    #[serde(default)]
    value: f32,
    #[serde(default)]
    min: f32,
    #[serde(default = "one")]
    max: f32,
    #[serde(default = "one")]
    chance: f32,
    #[serde(default)]
    source: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Blend {
    #[serde(rename = "1d")]
    One,
    #[serde(rename = "2d")]
    Two,
    Direct,
}

#[derive(Clone, Debug, Deserialize)]
struct Child {
    motion: Motion,
    #[serde(default = "one")]
    speed: f32,
    #[serde(default)]
    offset: f32,
    #[serde(default)]
    threshold: f32,
    #[serde(default)]
    position: [f32; 2],
    #[serde(default)]
    parameter: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Motion {
    clip: Option<usize>,
    time: Option<String>,
    blend: Option<Blend>,
    #[serde(default)]
    parameters: Vec<String>,
    #[serde(default)]
    children: Vec<Child>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    #[serde(default)]
    motion: Motion,
    #[serde(default = "one")]
    speed: f32,
    #[serde(default = "yes")]
    write_defaults: bool,
    #[serde(default)]
    transitions: Vec<Transition>,
    #[serde(default)]
    drivers: Vec<Driver>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
struct Layer {
    #[serde(default = "one")]
    weight: f32,
    #[serde(default)]
    default: usize,
    #[serde(default)]
    entry: Vec<Transition>,
    #[serde(default)]
    any: Vec<Transition>,
    states: Vec<State>,
}

#[derive(Clone, Debug, Deserialize)]
struct Curve {
    target: usize,
    keys: Vec<[f32; 2]>,
    #[serde(default)]
    step: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct Clip {
    #[serde(default)]
    length: f32,
    #[serde(default, rename = "loop")]
    looped: bool,
    curves: Vec<Curve>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Effect {
    Switch,
    Morph(Vec<[usize; 2]>),
    Scale { node: usize, axis: usize },
    Material { node: usize, choices: Vec<[f32; 4]> },
}

#[derive(Clone, Debug, Deserialize)]
pub struct Target {
    pub effect: Effect,
    #[serde(default)]
    pub default: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Shown {
    pub node: usize,
    pub when: Vec<(usize, f32)>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Expressions {
    pub menu: Vec<Control>,
    pub parameters: Vec<Parameter>,
    layers: Vec<Layer>,
    clips: Vec<Clip>,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub shown: Vec<Shown>,
    #[serde(default, rename = "icons")]
    encoded_icons: Vec<String>,
    #[serde(skip)]
    pub icons: Vec<Icon>,
}

#[derive(Clone, Debug)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn icon(encoded: &str) -> Result<Icon, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|error| error.to_string())?;
    let mut image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|error| error.to_string())?.into_rgba8();
    for pixel in image.pixels_mut() {
        let alpha = pixel[3] as u32;
        for channel in 0..3 {
            pixel[channel] = (pixel[channel] as u32 * alpha / 255) as u8;
        }
    }
    Ok(Icon { width: image.width(), height: image.height(), rgba: image.into_raw() })
}

impl Expressions {
    pub fn parse(value: &serde_json::Value) -> Result<Expressions, String> {
        let mut expressions = Expressions::deserialize(value).map_err(|error| format!("{EXTENSION}: {error}"))?;
        expressions.icons = expressions.encoded_icons.iter().map(|encoded| icon(encoded)).collect::<Result<_, _>>().map_err(|error| format!("{EXTENSION}: an icon: {error}"))?;
        expressions.encoded_icons.clear();
        fn icons_valid(controls: &[Control], icons: usize) -> bool {
            controls.iter().all(|control| control.icon.is_none_or(|icon| icon < icons) && icons_valid(&control.controls, icons))
        }
        if !icons_valid(&expressions.menu, expressions.icons.len()) {
            return Err(format!("{EXTENSION}: a control names an icon it does not have"));
        }
        let clips = expressions.clips.len();
        let targets = expressions.targets.len();
        let motions_valid = |motion: &Motion| -> bool {
            fn check(motion: &Motion, clips: usize) -> bool {
                motion.clip.is_none_or(|clip| clip < clips) && motion.children.iter().all(|child| check(&child.motion, clips))
            }
            check(motion, clips)
        };
        for layer in &expressions.layers {
            let states = layer.states.len() as i64;
            let transitions_valid = |transitions: &[Transition]| transitions.iter().all(|transition| (-1..states).contains(&transition.to));
            if layer.default >= layer.states.len().max(1) || !transitions_valid(&layer.entry) || !transitions_valid(&layer.any) || layer.states.iter().any(|state| !transitions_valid(&state.transitions) || !motions_valid(&state.motion)) {
                return Err(format!("{EXTENSION}: a layer names a state or clip it does not have"));
            }
        }
        if expressions.clips.iter().flat_map(|clip| &clip.curves).any(|curve| curve.target >= targets) || expressions.shown.iter().flat_map(|shown| &shown.when).any(|&(target, _)| target >= targets) {
            return Err(format!("{EXTENSION}: a curve or shown node names a target it does not have"));
        }
        Ok(expressions)
    }

    pub(super) fn check(&self, nodes: usize, meshes: &[super::Mesh]) -> Result<(), String> {
        let fits = self.targets.iter().all(|target| match &target.effect {
            Effect::Switch => true,
            Effect::Morph(binds) => binds.iter().all(|&[mesh, morph]| meshes.get(mesh).is_some_and(|mesh| morph < mesh.weights.len())),
            Effect::Scale { node, axis } => *node < nodes && *axis < 3,
            Effect::Material { node, choices } => *node < nodes && !choices.is_empty(),
        });
        if !fits || self.shown.iter().any(|shown| shown.node >= nodes) {
            return Err(format!("{EXTENSION}: a target or shown node names a node, mesh or blend shape the model does not have"));
        }
        Ok(())
    }

    pub fn switched(&self) -> impl Iterator<Item = usize> + '_ {
        self.shown.iter().map(|shown| shown.node)
    }

    pub fn materials<'a>(&'a self, values: &'a [f32]) -> impl Iterator<Item = (usize, [f32; 4])> + 'a {
        self.targets.iter().zip(values).filter_map(|(target, &value)| match &target.effect {
            Effect::Material { node, choices } => Some((*node, choices[(value.round().max(0.0) as usize).min(choices.len() - 1)])),
            _ => None,
        })
    }

    pub fn apply(&self, values: &[f32], pose: &mut super::Pose) -> HashSet<usize> {
        for (target, &value) in self.targets.iter().zip(values) {
            match &target.effect {
                Effect::Switch => {}
                Effect::Morph(binds) => {
                    for &[mesh, morph] in binds {
                        pose.weights[mesh][morph] = value.clamp(0.0, 1.0);
                    }
                }
                Effect::Scale { node, axis } => pose.scales[*node][*axis] *= value,
                Effect::Material { .. } => {}
            }
        }
        self.shown.iter().filter(|shown| shown.when.iter().any(|&(target, value)| (values[target] - value).abs() >= 0.5)).map(|shown| shown.node).collect()
    }
}

#[derive(Clone)]
struct Playing {
    state: usize,
    time: f32,
    held: HashMap<usize, f32>,
}

pub struct Controller {
    pub values: Vec<f32>,
    index: HashMap<String, usize>,
    playing: Vec<Option<Playing>>,
    random: u64,
}

impl Controller {
    pub fn new(expressions: &Expressions, remembered: &HashMap<String, f32>) -> Controller {
        let index = expressions.parameters.iter().enumerate().map(|(at, parameter)| (parameter.name.clone(), at)).collect();
        let values = expressions.parameters.iter().map(|parameter| remembered.get(&parameter.name).copied().unwrap_or(parameter.default)).collect();
        Controller { values, index, playing: vec![None; expressions.layers.len()], random: 0x9e37_79b9_7f4a_7c15 }
    }

    pub fn get(&self, name: &str) -> f32 {
        self.index.get(name).map_or(0.0, |&at| self.values[at])
    }

    pub fn set(&mut self, name: &str, value: f32) {
        if let Some(&at) = self.index.get(name) {
            self.values[at] = value;
        }
    }

    fn holds(&self, conditions: &[Condition]) -> bool {
        conditions.iter().all(|condition| {
            let value = self.get(&condition.parameter);
            match condition.mode {
                Mode::If => value != 0.0,
                Mode::IfNot => value == 0.0,
                Mode::Greater => value > condition.threshold,
                Mode::Less => value < condition.threshold,
                Mode::Equals => value == condition.threshold,
                Mode::NotEqual => value != condition.threshold,
            }
        })
    }

    fn next_random(&mut self) -> f32 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        (self.random >> 40) as f32 / (1u64 << 24) as f32
    }

    fn drive(&mut self, expressions: &Expressions, drivers: &[Driver]) {
        for driver in drivers {
            let Some(&at) = self.index.get(&driver.parameter) else { continue };
            let integral = expressions.parameters[at].kind != ValueType::Float;
            self.values[at] = match driver.drive {
                Drive::Set => driver.value,
                Drive::Add => self.values[at] + driver.value,
                Drive::Copy => self.get(&driver.source),
                Drive::Random if expressions.parameters[at].kind == ValueType::Bool => (self.next_random() < driver.chance) as u8 as f32,
                Drive::Random => {
                    let value = driver.min + self.next_random() * (driver.max - driver.min);
                    if integral { value.round() } else { value }
                }
            };
        }
    }

    fn enter(&mut self, expressions: &Expressions, layer: usize, state: usize) {
        let held = self.playing[layer].take().map(|playing| playing.held).unwrap_or_default();
        self.playing[layer] = Some(Playing { state, time: 0.0, held });
        self.drive(expressions, &expressions.layers[layer].states[state].drivers);
    }

    fn entered(&self, expressions: &Expressions, layer: usize) -> usize {
        let layer = &expressions.layers[layer];
        layer.entry.iter().find(|transition| transition.to >= 0 && self.holds(&transition.conditions)).map_or(layer.default, |transition| transition.to as usize)
    }

    pub fn step(&mut self, expressions: &Expressions, delta: f32) -> Vec<f32> {
        for layer in 0..expressions.layers.len() {
            if expressions.layers[layer].states.is_empty() {
                continue;
            }
            if self.playing[layer].is_none() {
                let state = self.entered(expressions, layer);
                self.enter(expressions, layer, state);
            }
            let mut any_taken = false;
            for _ in 0..HOPS {
                let playing = self.playing[layer].as_ref().unwrap();
                let state = &expressions.layers[layer].states[playing.state];
                let length = clip_length(expressions, &state.motion);
                let normalized = if length > 0.0 { playing.time / length } else { f32::INFINITY };
                let current = playing.state as i64;
                let any = (!any_taken).then(|| expressions.layers[layer].any.iter().find(|transition| !(transition.to == current && transition.not_to_self) && self.holds(&transition.conditions))).flatten();
                any_taken |= any.is_some();
                let taken = any.or_else(|| state.transitions.iter().find(|transition| transition.exit_time.is_none_or(|exit| normalized >= exit) && (transition.exit_time.is_some() || !transition.conditions.is_empty()) && self.holds(&transition.conditions)));
                let Some(transition) = taken else { break };
                let to = if transition.to < 0 { self.entered(expressions, layer) } else { transition.to as usize };
                let same = to as i64 == current && transition.to >= 0 && any.is_none();
                self.enter(expressions, layer, to);
                if same {
                    break;
                }
            }
            let playing = self.playing[layer].as_mut().unwrap();
            playing.time += delta * expressions.layers[layer].states[playing.state].speed;
        }
        let mut out: Vec<f32> = expressions.targets.iter().map(|target| target.default).collect();
        for (layer, playing) in expressions.layers.iter().zip(0..) {
            let Some(current) = self.playing[playing].clone() else { continue };
            let state = &layer.states[current.state];
            let mut written = HashMap::new();
            self.sample(expressions, &state.motion, current.time, 1.0, &mut written);
            let weight = layer.weight.clamp(0.0, 1.0);
            let playing = self.playing[playing].as_mut().unwrap();
            let values: Vec<(usize, f32)> = if state.write_defaults {
                written.into_iter().collect()
            } else {
                playing.held.extend(written);
                playing.held.iter().map(|(&target, &value)| (target, value)).collect()
            };
            for (target, value) in values {
                out[target] += (value - out[target]) * weight;
            }
        }
        out
    }

    fn sample(&self, expressions: &Expressions, motion: &Motion, time: f32, weight: f32, into: &mut HashMap<usize, f32>) {
        if let Some(clip) = motion.clip {
            let clip = &expressions.clips[clip];
            let at = match &motion.time {
                Some(parameter) => self.get(parameter).clamp(0.0, 1.0) * clip.length,
                None if clip.looped && clip.length > 0.0 => time.rem_euclid(clip.length),
                None => time.clamp(0.0, clip.length),
            };
            for curve in &clip.curves {
                *into.entry(curve.target).or_insert(0.0) += weight * curve.sample(at);
            }
            return;
        }
        let Some(blend) = motion.blend else { return };
        let weights = match blend {
            Blend::Direct => motion.children.iter().map(|child| self.get(&child.parameter)).collect(),
            Blend::One => {
                let at = self.get(motion.parameters.first().map_or("", String::as_str));
                one_d(&motion.children.iter().map(|child| child.threshold).collect::<Vec<_>>(), at)
            }
            Blend::Two => {
                let at = [self.get(motion.parameters.first().map_or("", String::as_str)), self.get(motion.parameters.get(1).map_or("", String::as_str))];
                two_d(&motion.children.iter().map(|child| child.position).collect::<Vec<_>>(), at)
            }
        };
        let animated: Vec<HashMap<usize, f32>> = motion
            .children
            .iter()
            .map(|child| {
                let mut part = HashMap::new();
                let length = clip_length(expressions, &child.motion);
                let at = time * child.speed + length * (child.offset + if child.speed < 0.0 { 1.0 } else { 0.0 });
                self.sample(expressions, &child.motion, at, 1.0, &mut part);
                part
            })
            .collect();
        let mut targets: Vec<usize> = animated.iter().flat_map(|part| part.keys().copied()).collect();
        targets.sort_unstable();
        targets.dedup();
        for target in targets {
            let default = expressions.targets[target].default;
            let value: f32 = if blend == Blend::Direct {
                animated.iter().zip(&weights).filter_map(|(part, &share)| part.get(&target).map(|&value| share * value)).sum()
            } else {
                animated.iter().zip(&weights).map(|(part, &share)| share * part.get(&target).copied().unwrap_or(default)).sum()
            };
            *into.entry(target).or_insert(0.0) += weight * value;
        }
    }
}

fn clip_length(expressions: &Expressions, motion: &Motion) -> f32 {
    match motion.clip {
        Some(clip) => expressions.clips[clip].length,
        None => motion.children.iter().map(|child| clip_length(expressions, &child.motion)).fold(0.0, f32::max),
    }
}

impl Curve {
    fn sample(&self, at: f32) -> f32 {
        let Some(first) = self.keys.first() else { return 0.0 };
        let after = self.keys.partition_point(|key| key[0] <= at);
        if after == 0 {
            return first[1];
        }
        let [t0, v0] = self.keys[after - 1];
        match self.keys.get(after) {
            Some(&[t1, v1]) if !self.step && t1 > t0 => v0 + (v1 - v0) * (at - t0) / (t1 - t0),
            _ => v0,
        }
    }
}

fn one_d(thresholds: &[f32], at: f32) -> Vec<f32> {
    let mut weights = vec![0.0; thresholds.len()];
    let mut order: Vec<usize> = (0..thresholds.len()).collect();
    order.sort_by(|&a, &b| thresholds[a].total_cmp(&thresholds[b]));
    let (Some(&low), Some(&high)) = (order.first(), order.last()) else { return weights };
    if at <= thresholds[low] {
        weights[low] = 1.0;
    } else if at >= thresholds[high] {
        weights[high] = 1.0;
    } else {
        let upper = order.partition_point(|&child| thresholds[child] <= at);
        let (a, b) = (order[upper - 1], order[upper]);
        let share = (at - thresholds[a]) / (thresholds[b] - thresholds[a]);
        weights[a] = 1.0 - share;
        weights[b] = share;
    }
    weights
}

fn two_d(positions: &[[f32; 2]], at: [f32; 2]) -> Vec<f32> {
    let distances: Vec<f32> = positions.iter().map(|&[x, y]| (x - at[0]).powi(2) + (y - at[1]).powi(2)).collect();
    if let Some(exact) = distances.iter().position(|&distance| distance < 1e-8) {
        return (0..positions.len()).map(|child| (child == exact) as u8 as f32).collect();
    }
    let inverse: Vec<f32> = distances.iter().map(|distance| 1.0 / distance).collect();
    let total: f32 = inverse.iter().sum();
    inverse.iter().map(|weight| weight / total).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toggle(write_defaults: bool) -> Expressions {
        let off = if write_defaults { serde_json::json!({}) } else { serde_json::json!({"clip": 1}) };
        Expressions::parse(&serde_json::json!({
            "menu": [{"name": "Hat", "type": "toggle", "parameter": "Hat"}, {"name": "Size", "type": "radial", "parameter": "Size", "axes": ["Size"]}],
            "parameters": [{"name": "Hat", "type": "bool", "saved": true}, {"name": "Size", "type": "float", "default": 0.25}],
            "layers": [
                {"states": [{"motion": off, "writeDefaults": write_defaults, "transitions": [{"to": 1, "conditions": [{"parameter": "Hat", "mode": "if"}]}]},
                            {"motion": {"clip": 0}, "writeDefaults": write_defaults, "transitions": [{"to": 0, "conditions": [{"parameter": "Hat", "mode": "ifNot"}]}]}]},
                {"states": [{"motion": {"clip": 2, "time": "Size"}}]}
            ],
            "clips": [
                {"curves": [{"target": 0, "keys": [[0, 1]], "step": true}]},
                {"curves": [{"target": 0, "keys": [[0, 0]], "step": true}]},
                {"length": 1, "curves": [{"target": 1, "keys": [[0, 0], [1, 1]]}]}
            ],
            "targets": [{"effect": "switch"}, {"effect": {"morph": [[0, 0]]}}],
            "shown": [{"node": 3, "when": [[0, 1]]}]
        }))
        .unwrap()
    }

    #[test]
    fn a_toggle_switches_its_target_and_back_with_or_without_write_defaults() {
        for write_defaults in [true, false] {
            let expressions = toggle(write_defaults);
            let mut controller = Controller::new(&expressions, &HashMap::new());
            assert_eq!(controller.step(&expressions, 0.011)[0], 0.0);
            controller.set("Hat", 1.0);
            assert_eq!(controller.step(&expressions, 0.011)[0], 1.0, "write defaults {write_defaults}");
            controller.set("Hat", 0.0);
            assert_eq!(controller.step(&expressions, 0.011)[0], 0.0, "write defaults {write_defaults}");
        }
    }

    #[test]
    fn a_radial_sets_its_clip_time_and_a_remembered_value_applies_from_the_first_frame() {
        let expressions = toggle(true);
        let mut controller = Controller::new(&expressions, &HashMap::from([("Size".to_owned(), 0.75), ("Hat".to_owned(), 1.0)]));
        let values = controller.step(&expressions, 0.011);
        assert_eq!(values, vec![1.0, 0.75]);
        controller.set("Size", 0.5);
        assert!((controller.step(&expressions, 0.011)[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn blend_trees_weigh_their_children() {
        assert_eq!(one_d(&[0.0, 1.0], 0.25), vec![0.75, 0.25]);
        assert_eq!(one_d(&[1.0, 0.0, 0.5], 2.0), vec![1.0, 0.0, 0.0]);
        assert_eq!(two_d(&[[0.0, 0.0], [1.0, 0.0]], [1.0, 0.0]), vec![0.0, 1.0]);
        let halfway = two_d(&[[-1.0, 0.0], [1.0, 0.0]], [0.0, 0.0]);
        assert!((halfway[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn blend_children_play_backwards_or_from_an_offset() {
        let mut expressions = Expressions::parse(&serde_json::json!({
            "menu": [], "parameters": [{"name": "Choice", "type": "float"}],
            "layers": [{"states": [{"motion": {"blend": "1d", "parameters": ["Choice"], "children": [
                {"motion": {"clip": 0}, "threshold": 0, "speed": -1},
                {"motion": {"clip": 0}, "threshold": 1}
            ]}}]}],
            "clips": [{"length": 1, "curves": [{"target": 0, "keys": [[0, 0], [1, 1]]}]}],
            "targets": [{"effect": "switch"}]
        })).unwrap();
        let mut controller = Controller::new(&expressions, &HashMap::new());
        assert_eq!(controller.step(&expressions, 0.25), vec![0.75]);
        assert_eq!(controller.step(&expressions, 1.0), vec![0.0]);
        controller.set("Choice", 1.0);
        assert_eq!(controller.step(&expressions, 0.0), vec![1.0]);
        let child = &mut expressions.layers[0].states[0].motion.children[1];
        child.speed = 0.0;
        child.offset = 0.25;
        assert_eq!(controller.step(&expressions, 0.0), vec![0.25]);
    }

    #[test]
    fn drivers_set_parameters_on_entering_their_state_and_exit_returns_through_entry() {
        let expressions = Expressions::parse(&serde_json::json!({
            "menu": [], "parameters": [{"name": "Preset", "type": "int"}, {"name": "Hat", "type": "bool"}],
            "layers": [{"entry": [], "states": [
                {"transitions": [{"to": 1, "conditions": [{"parameter": "Preset", "mode": "equals", "threshold": 1}]}]},
                {"drivers": [{"parameter": "Hat", "drive": "set", "value": 1}, {"parameter": "Preset", "drive": "set", "value": 0}], "transitions": [{"to": -1, "exitTime": 0}]}
            ]}],
            "clips": [], "targets": []
        }))
        .unwrap();
        let mut controller = Controller::new(&expressions, &HashMap::new());
        controller.set("Preset", 1.0);
        controller.step(&expressions, 0.011);
        assert_eq!((controller.get("Hat"), controller.get("Preset")), (1.0, 0.0));
        assert_eq!(controller.playing[0].as_ref().unwrap().state, 0);
    }

    #[test]
    fn a_reference_past_the_end_is_refused() {
        let bad = serde_json::json!({"menu": [], "parameters": [], "layers": [{"states": [{"motion": {"clip": 3}}]}], "clips": [], "targets": []});
        assert!(Expressions::parse(&bad).is_err());
    }
}
