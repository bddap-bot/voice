use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use super::Image;
use super::spring::Idle;

pub const EXTENSION: &str = "VRCHAT_expressions";
/// The effects and look fields the overlay reads; the catalog's publish guard refuses an avatar naming any other.
const READ: &str = include_str!("effects.json");
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
    keys: Vec<Key>,
    #[serde(default)]
    step: bool,
}

/// A Unity keyframe: `[time, value]`, or `[time, value, inSlope, outSlope]` with an optional
/// `inWeight, outWeight` pair; a null slope is Unity's infinite (constant) tangent.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(try_from = "Vec<Option<f32>>")]
struct Key {
    time: f32,
    value: f32,
    slopes: Option<[f32; 2]>,
    weights: [f32; 2],
}

impl TryFrom<Vec<Option<f32>>> for Key {
    type Error = String;

    fn try_from(row: Vec<Option<f32>>) -> Result<Self, String> {
        let slope = |at: usize| row[at].unwrap_or(f32::INFINITY);
        let (Some(time), Some(value)) = (row.first().copied().flatten(), row.get(1).copied().flatten()) else {
            return Err(format!("a key needs a time and a value, got {row:?}"));
        };
        let (slopes, weights) = match row.len() {
            2 => (None, [THIRD; 2]),
            4 => (Some([slope(2), slope(3)]), [THIRD; 2]),
            6 => match (row[4], row[5]) {
                (Some(into), Some(out)) => (Some([slope(2), slope(3)]), [into.clamp(0.0, 1.0), out.clamp(0.0, 1.0)]),
                _ => return Err(format!("key weights must be numbers, got {row:?}")),
            },
            _ => return Err(format!("a key has 2, 4 or 6 numbers, got {row:?}")),
        };
        Ok(Key { time, value, slopes, weights })
    }
}

/// Unity's tangent weight that makes a weighted segment its plain Hermite.
const THIRD: f32 = 1.0 / 3.0;

#[derive(Clone, Debug, Deserialize)]
struct Clip {
    #[serde(default)]
    length: f32,
    #[serde(default, rename = "loop")]
    looped: bool,
    curves: Vec<Curve>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Hue {
    pub node: usize,
    pub rect: [f32; 4],
    pub space: f32,
    pub shift: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Color {
    pub node: usize,
    pub rect: [f32; 4],
    pub channel: usize,
    pub color: [f32; 3],
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Tone {
    pub node: usize,
    pub rect: [f32; 4],
    pub channel: usize,
}

/// A value of the looks a material lays over its albedo and adds to its light, by its row in `Surface::look`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Field {
    Decal,
    DecalIntensity,
    Emission,
    EmissionHue,
    MatcapHueShift,
    MatcapIntensity,
    MatcapReplace,
    MatcapMultiply,
    MatcapAdd,
    MatcapMixed,
}

pub const FIELDS: usize = Field::MatcapMixed as usize + 1;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LookRegion {
    pub node: usize,
    pub rect: [f32; 4],
    pub field: Field,
    pub slot: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Mapping {
    /// The normal against the view, held upright.
    Normal,
    /// The reflected view against the world's axes.
    Reflection,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MatcapHue {
    pub shift: f32,
    pub space: f32,
    pub select: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Circle {
    pub border: f32,
    pub blur: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Matcap {
    pub texture: Option<u32>,
    pub mode: Mapping,
    pub border: f32,
    pub color: [f32; 4],
    pub intensity: f32,
    pub base_mix: f32,
    pub replace: f32,
    pub multiply: f32,
    pub add: f32,
    pub screen: f32,
    pub mixed: f32,
    pub light_add: f32,
    pub emission: f32,
    pub light_mask: f32,
    pub hue: Option<MatcapHue>,
    pub circle: Option<Circle>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Decal {
    pub alpha: f32,
    pub intensity: f32,
    pub multiply: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Emission {
    pub color: [f32; 3],
    pub strength: f32,
    pub hue: f32,
    pub shifts_hue: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Look {
    pub decals: [Option<Decal>; 4],
    pub matcaps: [Option<Matcap>; 4],
    pub emission: Option<Emission>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncodedLooks {
    index: String,
    materials: Vec<Look>,
    #[serde(default)]
    decals: Vec<String>,
    #[serde(default)]
    matcaps: Vec<String>,
    #[serde(default)]
    matcap_masks: Option<String>,
    #[serde(default)]
    emission: Option<String>,
}

/// What each atlas texel's material lays over the albedo: `index`'s red channel names a texel's entry in `materials`,
/// one-based, zero for none.
#[derive(Clone, Debug)]
pub struct Looks {
    pub index: Image,
    pub materials: Vec<Look>,
    pub decals: Vec<Image>,
    pub matcaps: Vec<Image>,
    pub matcap_masks: Option<Image>,
    pub emission: Option<Image>,
}

/// One component of a node's Unity local rotation: of `rest` read as localEulerAngles in degrees when it has three, a quaternion when four.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Rotation {
    pub node: usize,
    pub axis: usize,
    pub rest: Vec<f32>,
}

impl Rotation {
    fn unity(&self, components: &[f32]) -> Quat {
        match *components {
            [x, y, z] => Quat::from_rotation_y(y.to_radians()) * Quat::from_rotation_x(x.to_radians()) * Quat::from_rotation_z(z.to_radians()),
            [x, y, z, w] => Quat::from_xyzw(x, y, z, w).normalize(),
            _ => Quat::IDENTITY,
        }
    }

    /// The turn of the node's glTF local rotation that takes it from `rest` to `components`.
    fn turn(&self, components: &[f32]) -> Quat {
        let delta = self.unity(&self.rest).inverse() * self.unity(components);
        Quat::from_xyzw(delta.x, -delta.y, -delta.z, delta.w).normalize()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Effect {
    Switch,
    Hue(Vec<Hue>),
    Color(Vec<Color>),
    Tone(Vec<Tone>),
    Look(#[serde(deserialize_with = "read_regions")] Vec<LookRegion>),
    Morph(Vec<[usize; 2]>),
    Scale { node: usize, axis: usize },
    Rotation(Rotation),
    Translation { node: usize, direction: [f32; 3] },
    Material { node: usize, choices: Vec<[f32; 4]> },
    /// An effect a newer converter writes: its target holds still rather than refusing the avatar.
    #[serde(skip)]
    Unread(String),
}

#[derive(Deserialize)]
struct Read {
    effects: Vec<String>,
    looks: Vec<String>,
}

fn read() -> &'static Read {
    static KNOWN: OnceLock<Read> = OnceLock::new();
    KNOWN.get_or_init(|| serde_json::from_str(READ).unwrap())
}

fn read_effect<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Effect, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let name = match &value {
        serde_json::Value::String(name) => name,
        serde_json::Value::Object(map) if map.len() == 1 => map.keys().next().unwrap(),
        _ => return Err(serde::de::Error::custom("an effect is a name or an object of one name")),
    };
    if !read().effects.contains(name) {
        return Ok(Effect::Unread(name.clone()));
    }
    Effect::deserialize(value).map_err(serde::de::Error::custom)
}

/// A region of a look field a newer converter writes holds still, as an unread effect's target does.
fn read_regions<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<LookRegion>, D::Error> {
    let regions = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let (known, unread): (Vec<_>, Vec<_>) = regions.into_iter().partition(|region| region["field"].as_str().is_some_and(|field| read().looks.iter().any(|known| known == field)));
    if !unread.is_empty() {
        tracing::warn!("{EXTENSION}: look regions of fields the overlay does not read hold still: {:?}", unread.iter().map(|region| region["field"].to_string()).collect::<BTreeSet<_>>());
    }
    known.into_iter().map(|region| LookRegion::deserialize(region).map_err(serde::de::Error::custom)).collect()
}

#[derive(Clone, Debug, Deserialize)]
pub struct Target {
    #[serde(deserialize_with = "read_effect")]
    pub effect: Effect,
    #[serde(default)]
    pub default: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Shown {
    pub node: usize,
    pub when: Vec<(usize, f32)>,
}

/// A PhysBone the menu switches: its joints run only while every switch in `when` is at its value.
#[derive(Clone, Debug, Deserialize)]
pub struct SwitchedSpring {
    pub joints: Vec<usize>,
    pub when: Vec<(usize, f32)>,
}

/// A collider, by its place in VRMC_springBone's list, that presses only while every switch in `when` is at its value.
#[derive(Clone, Debug, Deserialize)]
pub struct SwitchedCollider {
    pub collider: usize,
    pub when: Vec<(usize, f32)>,
}

fn off(when: &[(usize, f32)], values: &[f32]) -> bool {
    when.iter().any(|&(target, value)| (values[target] - value).abs() >= 0.5)
}

#[derive(Clone, Debug, Deserialize)]
pub struct Expressions {
    #[serde(skip)]
    source: String,
    pub menu: Vec<Control>,
    pub parameters: Vec<Parameter>,
    layers: Vec<Layer>,
    clips: Vec<Clip>,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub shown: Vec<Shown>,
    #[serde(default)]
    springs: Vec<SwitchedSpring>,
    #[serde(default)]
    colliders: Vec<SwitchedCollider>,
    #[serde(default, rename = "icons")]
    encoded_icons: Vec<String>,
    #[serde(skip)]
    pub icons: Vec<Image>,
    #[serde(default, rename = "hueMask")]
    encoded_hue_mask: Option<String>,
    #[serde(skip)]
    pub hue_mask: Option<Image>,
    #[serde(default, rename = "colorAtlas")]
    encoded_color_atlas: Option<String>,
    #[serde(skip)]
    pub color_atlas: Option<Image>,
    #[serde(default, rename = "looks")]
    encoded_looks: Option<EncodedLooks>,
    #[serde(skip)]
    pub looks: Option<Looks>,
}

fn image(encoded: &str) -> Result<Image, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|error| error.to_string())?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|error| error.to_string())?.into_rgba8();
    Ok(Image { width: image.width(), height: image.height(), rgba: image.into_raw() })
}

fn looks(encoded: EncodedLooks) -> Result<Looks, String> {
    let layers = |encoded: &[String], what: &str| -> Result<Vec<Image>, String> {
        let layers: Vec<Image> = encoded.iter().map(|encoded| image(encoded)).collect::<Result<_, _>>().map_err(|error| format!("{EXTENSION}: {what}: {error}"))?;
        if layers.iter().any(|layer| (layer.width, layer.height) != (layers[0].width, layers[0].height)) {
            return Err(format!("{EXTENSION}: {what} of differing sizes"));
        }
        Ok(layers)
    };
    let looks = Looks {
        index: image(&encoded.index).map_err(|error| format!("{EXTENSION}: look index: {error}"))?,
        decals: layers(&encoded.decals, "decal layers")?,
        matcaps: layers(&encoded.matcaps, "matcaps")?,
        matcap_masks: encoded.matcap_masks.as_deref().map(image).transpose().map_err(|error| format!("{EXTENSION}: matcap masks: {error}"))?,
        emission: encoded.emission.as_deref().map(image).transpose().map_err(|error| format!("{EXTENSION}: emission: {error}"))?,
        materials: encoded.materials,
    };
    let textures = looks.matcaps.len() as u32;
    if looks.decals.len() > 4 || looks.index.rgba.chunks_exact(4).any(|texel| texel[0] as usize > looks.materials.len()) || looks.materials.iter().flat_map(|look| look.matcaps.iter().flatten()).any(|matcap| matcap.texture.is_some_and(|texture| texture >= textures)) {
        return Err(format!("{EXTENSION}: a look names a decal slot, material or matcap it does not have"));
    }
    Ok(looks)
}

fn icon(encoded: &str) -> Result<Image, String> {
    let mut image = image(encoded)?;
    for pixel in image.rgba.chunks_exact_mut(4) {
        for channel in 0..3 {
            pixel[channel] = (pixel[channel] as u32 * pixel[3] as u32 / 255) as u8;
        }
    }
    Ok(image)
}

impl Expressions {
    pub fn parse(value: &serde_json::Value) -> Result<Expressions, String> {
        let mut expressions = Expressions::deserialize(value).map_err(|error| format!("{EXTENSION}: {error}"))?;
        expressions.source = format!("{:x}", Sha256::digest(value.to_string().as_bytes()));
        expressions.icons = expressions.encoded_icons.iter().map(|encoded| icon(encoded)).collect::<Result<_, _>>().map_err(|error| format!("{EXTENSION}: an icon: {error}"))?;
        expressions.encoded_icons.clear();
        expressions.hue_mask = expressions.encoded_hue_mask.take().map(|encoded| image(&encoded)).transpose().map_err(|error| format!("{EXTENSION}: hue mask: {error}"))?;
        expressions.color_atlas = expressions.encoded_color_atlas.take().map(|encoded| image(&encoded)).transpose().map_err(|error| format!("{EXTENSION}: color atlas: {error}"))?;
        expressions.looks = expressions.encoded_looks.take().map(looks).transpose()?;
        let unread: BTreeSet<&str> = expressions.targets.iter().filter_map(|target| match &target.effect { Effect::Unread(name) => Some(name.as_str()), _ => None }).collect();
        if !unread.is_empty() {
            tracing::warn!("{EXTENSION}: targets of effects the overlay does not read hold still: {unread:?}");
        }
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
        if expressions.clips.iter().flat_map(|clip| &clip.curves).any(|curve| curve.target >= targets) 
            || expressions.shown.iter().flat_map(|shown| &shown.when).chain(expressions.springs.iter().flat_map(|spring| &spring.when)).chain(expressions.colliders.iter().flat_map(|collider| &collider.when)).any(|&(target, _)| target >= targets)
        {
            return Err(format!("{EXTENSION}: a curve or switched node, spring or collider names a target it does not have"));
        }
        Ok(expressions)
    }

    pub(super) fn check(&self, nodes: usize, meshes: &[super::Mesh]) -> Result<(), String> {
        let fits = self.targets.iter().all(|target| match &target.effect {
            Effect::Switch | Effect::Unread(_) => true,
            Effect::Hue(regions) => regions.iter().all(|region| region.node < nodes && region.rect.iter().all(|v| v.is_finite()) && region.rect[0] <= region.rect[2] && region.rect[1] <= region.rect[3]),
            Effect::Color(regions) => regions.iter().all(|region| region.node < nodes && region.channel < 3 && region.color.iter().chain(&region.rect).all(|v| v.is_finite()) && region.rect[0] <= region.rect[2] && region.rect[1] <= region.rect[3]),
            Effect::Tone(regions) => regions.iter().all(|region| region.node < nodes && region.channel < 3 && region.rect.iter().all(|v| v.is_finite()) && region.rect[0] <= region.rect[2] && region.rect[1] <= region.rect[3]),
            Effect::Look(regions) => regions.iter().all(|region| region.node < nodes && region.slot < 4 && region.rect.iter().all(|v| v.is_finite()) && region.rect[0] <= region.rect[2] && region.rect[1] <= region.rect[3]),
            Effect::Morph(binds) => binds.iter().all(|&[mesh, morph]| meshes.get(mesh).is_some_and(|mesh| morph < mesh.weights.len())),
            Effect::Scale { node, axis } => *node < nodes && *axis < 3,
            Effect::Rotation(rotation) => rotation.node < nodes && matches!(rotation.rest.len(), 3 | 4) && rotation.axis < rotation.rest.len() && rotation.rest.iter().all(|v| v.is_finite())
                && self.targets.iter().all(|other| !matches!(&other.effect, Effect::Rotation(other) if other.node == rotation.node && other.rest != rotation.rest)),
            Effect::Translation { node, direction } => *node < nodes && direction.iter().all(|v| v.is_finite()),
            Effect::Material { node, choices } => *node < nodes && !choices.is_empty(),
        });
        if !fits || self.shown.iter().any(|shown| shown.node >= nodes) || self.springs.iter().flat_map(|spring| &spring.joints).any(|&joint| joint >= nodes) {
            return Err(format!("{EXTENSION}: a target, shown node or switched spring names a node, mesh or blend shape the model does not have"));
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

    /// The spring joints and colliders the values switch off.
    pub fn idle(&self, values: &[f32]) -> Idle {
        Idle {
            joints: self.springs.iter().filter(|spring| off(&spring.when, values)).flat_map(|spring| spring.joints.iter().copied()).collect(),
            colliders: self.colliders.iter().filter(|collider| off(&collider.when, values)).map(|collider| collider.collider).collect(),
        }
    }

    pub fn apply(&self, values: &[f32], pose: &mut super::Pose) -> HashSet<usize> {
        let mut turned: HashMap<usize, (&Rotation, Vec<f32>)> = HashMap::new();
        for (target, &value) in self.targets.iter().zip(values) {
            match &target.effect {
                Effect::Switch => {}
                Effect::Morph(binds) => {
                    for &[mesh, morph] in binds {
                        pose.weights[mesh][morph] = value.clamp(0.0, 1.0);
                    }
                }
                Effect::Scale { node, axis } => pose.scales[*node][*axis] *= value,
                Effect::Rotation(rotation) => turned.entry(rotation.node).or_insert_with(|| (rotation, rotation.rest.clone())).1[rotation.axis] = value,
                Effect::Translation { node, direction } => pose.translations[*node] += value * Vec3::from(*direction),
                Effect::Material { .. } | Effect::Hue(_) | Effect::Color(_) | Effect::Tone(_) | Effect::Look(_) | Effect::Unread(_) => {}
            }
        }
        for (node, (rotation, components)) in turned {
            pose.rotations[node] = (pose.rotations[node] * rotation.turn(&components)).normalize();
        }
        self.shown.iter().filter(|shown| off(&shown.when, values)).map(|shown| shown.node).collect()
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Configuration {
    pub parameters: HashMap<String, f32>,
    source: String,
    held: Vec<HashMap<usize, f32>>,
    #[serde(default)]
    playing: Vec<Option<Playing>>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct Playing {
    state: usize,
    time: f32,
}

#[derive(Clone)]
pub struct Controller {
    pub values: Vec<f32>,
    index: HashMap<String, usize>,
    playing: Vec<Option<Playing>>,
    random: u64,
    source: String,
    held: Vec<HashMap<usize, f32>>,
}

impl Controller {
    pub fn new(expressions: &Expressions, remembered: &Configuration) -> Controller {
        let index = expressions.parameters.iter().enumerate().map(|(at, parameter)| (parameter.name.clone(), at)).collect();
        let values = expressions.parameters.iter().map(|parameter| remembered.parameters.get(&parameter.name).copied().unwrap_or(parameter.default)).collect();
        let held = (0..expressions.layers.len()).map(|layer| {
            if remembered.source == expressions.source {
                remembered.held.get(layer).cloned().unwrap_or_default().into_iter().filter(|(target, _)| *target < expressions.targets.len()).collect()
            } else {
                HashMap::new()
            }
        }).collect();
        let playing = expressions.layers.iter().enumerate().map(|(layer, definition)| {
            (remembered.source == expressions.source).then(|| remembered.playing.get(layer).copied().flatten()).flatten()
                .filter(|playing| playing.state < definition.states.len() && playing.time.is_finite())
        }).collect();
        Controller { values, index, playing, random: 0x9e37_79b9_7f4a_7c15, source: expressions.source.clone(), held }
    }

    pub fn configuration(&self, parameters: HashMap<String, f32>) -> Configuration {
        Configuration { parameters, source: self.source.clone(), held: self.held.clone(), playing: self.playing.clone() }
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
        self.playing[layer] = Some(Playing { state, time: 0.0 });
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
            let Some(current) = self.playing[playing] else { continue };
            let state = &layer.states[current.state];
            let mut written = HashMap::new();
            self.sample(expressions, &state.motion, current.time, 1.0, &mut written);
            let weight = layer.weight.clamp(0.0, 1.0);
            let held = &mut self.held[playing];
            let values: Vec<(usize, f32)> = if state.write_defaults {
                written.into_iter().collect()
            } else {
                held.extend(written);
                held.iter().map(|(&target, &value)| (target, value)).collect()
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
        // A child whose motion keys no curve on a target adds nothing to it, as Unity blends: an empty clip in a direct
        // tree's child does not write the target's default over a sibling's value.
        for target in targets {
            let keyed: Vec<(f32, f32)> = animated.iter().zip(&weights).filter(|&(_, &share)| share > 0.0).filter_map(|(part, &share)| part.get(&target).map(|&value| (share, value))).collect();
            if !keyed.is_empty() {
                *into.entry(target).or_insert(0.0) += weight * keyed.iter().map(|(share, value)| share * value).sum::<f32>();
            }
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
    /// Unity's `AnimationCurve.Evaluate`: each key pair is a cubic Bezier through the keys' slopes
    /// and weights (a plain Hermite at the default weights), held at the left key when either facing
    /// slope is infinite, and clamped past the ends.
    fn sample(&self, at: f32) -> f32 {
        let Some(first) = self.keys.first() else { return 0.0 };
        let after = self.keys.partition_point(|key| key.time <= at);
        if after == 0 {
            return first.value;
        }
        let a = self.keys[after - 1];
        let Some(&b) = self.keys.get(after) else { return a.value };
        let span = b.time - a.time;
        if self.step || span <= 0.0 {
            return a.value;
        }
        let p = (at - a.time) / span;
        let (Some([_, out]), Some([into, _])) = (a.slopes, b.slopes) else {
            return a.value + (b.value - a.value) * p;
        };
        if !out.is_finite() || !into.is_finite() {
            return a.value;
        }
        let (wa, wb) = (a.weights[1], b.weights[0]);
        let bezier = |s: f32, p0: f32, p1: f32, p2: f32, p3: f32| {
            let r = 1.0 - s;
            r * r * r * p0 + 3.0 * r * r * s * p1 + 3.0 * r * s * s * p2 + s * s * s * p3
        };
        let (mut low, mut high) = (0.0f32, 1.0f32);
        for _ in 0..32 {
            let mid = 0.5 * (low + high);
            if bezier(mid, 0.0, wa, 1.0 - wb, 1.0) < p {
                low = mid;
            } else {
                high = mid;
            }
        }
        bezier(0.5 * (low + high), a.value, a.value + wa * span * out, b.value - wb * span * into, b.value)
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
    fn rewear_keeps_values_an_animation_leaves_unchanged() {
        let data = serde_json::json!({
            "menu": [], "parameters": [{"name": "Outfit", "type": "int"}],
            "layers": [{"states": [
                {"writeDefaults": false, "transitions": [{"to": 1, "conditions": [{"parameter": "Outfit", "mode": "equals", "threshold": 1}]}]},
                {"writeDefaults": false, "motion": {"clip": 0}, "transitions": [{"to": 0, "conditions": [{"parameter": "Outfit", "mode": "equals", "threshold": 0}]}]}
            ]}],
            "clips": [{"curves": [{"target": 0, "keys": [[0, 1]]}]}],
            "targets": [{"effect": {"morph": [[0, 0]]}}]
        });
        let menu = Expressions::parse(&data).unwrap();
        let mut worn = Controller::new(&menu, &Configuration::default());
        worn.set("Outfit", 1.0);
        worn.step(&menu, 0.1);
        worn.set("Outfit", 0.0);
        let seen = worn.step(&menu, 0.1);
        assert_eq!(seen, vec![1.0]);
        let saved = worn.configuration(HashMap::from([("Outfit".to_owned(), 0.0)]));
        let read = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(Controller::new(&menu, &read).step(&menu, 0.1), seen);
        let mut changed = data;
        changed["targets"][0]["effect"]["morph"][0][1] = serde_json::json!(1);
        let updated = Expressions::parse(&changed).unwrap();
        let mut again = Controller::new(&updated, &read);
        assert_eq!(again.get("Outfit"), 0.0);
        assert_eq!(again.step(&updated, 0.1), vec![0.0]);
    }

    #[test]
    fn rewear_resumes_layers_without_replaying_entry_drivers() {
        let menu = Expressions::parse(&serde_json::json!({
            "menu": [], "parameters": [{"name": "Parent", "type": "bool"}, {"name": "Child", "type": "bool"}],
            "layers": [{"any": [
                {"to": 1, "notToSelf": true, "conditions": [{"parameter": "Parent", "mode": "ifNot"}]},
                {"to": 2, "notToSelf": true, "conditions": [{"parameter": "Parent", "mode": "if"}]}
            ], "states": [{}, {"drivers": [{"parameter": "Child", "drive": "set", "value": 0}]}, {}]}],
            "clips": [], "targets": []
        })).unwrap();
        let mut worn = Controller::new(&menu, &Configuration::default());
        worn.step(&menu, 0.1);
        worn.set("Child", 1.0);
        worn.step(&menu, 0.1);
        let saved = worn.configuration(HashMap::from([("Parent".to_owned(), 0.0), ("Child".to_owned(), 1.0)]));
        let read = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        let mut again = Controller::new(&menu, &read);
        again.step(&menu, 0.1);
        assert_eq!(again.get("Child"), 1.0);
        again.set("Parent", 1.0);
        again.step(&menu, 0.1);
        again.set("Parent", 0.0);
        again.step(&menu, 0.1);
        assert_eq!(again.get("Child"), 0.0, "changing the parent still enters its reset state");
    }

    #[test]
    fn a_toggle_switches_its_target_and_back_with_or_without_write_defaults() {
        for write_defaults in [true, false] {
            let expressions = toggle(write_defaults);
            let mut controller = Controller::new(&expressions, &Configuration::default());
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
        let mut controller = Controller::new(&expressions, &Configuration { parameters: HashMap::from([("Size".to_owned(), 0.75), ("Hat".to_owned(), 1.0)]), ..Default::default() });
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
        let mut controller = Controller::new(&expressions, &Configuration::default());
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
    fn a_direct_child_whose_chosen_motion_keys_nothing_leaves_a_sibling_s_value() {
        let expressions = Expressions::parse(&serde_json::json!({
            "menu": [], "parameters": [{"name": "Weight", "type": "float", "default": 1}, {"name": "Toggle", "type": "bool"}, {"name": "Loop", "type": "bool"}],
            "layers": [{"states": [{"motion": {"blend": "direct", "children": [
                {"parameter": "Weight", "motion": {"blend": "1d", "parameters": ["Toggle"], "children": [{"motion": {"clip": 0}, "threshold": 0}, {"motion": {"clip": 1}, "threshold": 1}]}},
                {"parameter": "Weight", "motion": {"blend": "1d", "parameters": ["Loop"], "children": [{"motion": {"clip": 2}, "threshold": 0}, {"motion": {"clip": 0}, "threshold": 1}]}}
            ]}}]}],
            "clips": [{"length": 1, "curves": [{"target": 0, "keys": [[0, 0]], "step": true}]}, {"length": 1, "curves": [{"target": 0, "keys": [[0, 1]], "step": true}]}, {"length": 1, "curves": []}],
            "targets": [{"effect": "switch", "default": 1}]
        })).unwrap();
        let mut controller = Controller::new(&expressions, &Configuration::default());
        assert_eq!(controller.step(&expressions, 0.1), vec![0.0], "toggle off, loop off: the loop's empty clip adds nothing, so the flag is off");
        controller.set("Toggle", 1.0);
        assert_eq!(controller.step(&expressions, 0.1), vec![1.0], "toggle on, loop off: on");
        controller.set("Loop", 1.0);
        assert_eq!(controller.step(&expressions, 0.1), vec![1.0], "toggle on, loop keys it off: the direct tree adds the two");
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
        let mut controller = Controller::new(&expressions, &Configuration::default());
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

    #[test]
    fn curves_match_unity_on_flat_free_constant_and_weighted_tangents() {
        let unity: serde_json::Value = serde_json::from_str(include_str!("../../golden/unity-curves.json")).unwrap();
        for (name, curve) in unity.as_object().unwrap() {
            let ours: Curve = serde_json::from_value(serde_json::json!({"target": 0, "keys": curve["keys"]})).unwrap();
            let samples = curve["unity"].as_array().unwrap();
            let range = samples.iter().map(|s| s[1].as_f64().unwrap()).fold(f64::NEG_INFINITY, f64::max) - samples.iter().map(|s| s[1].as_f64().unwrap()).fold(f64::INFINITY, f64::min);
            assert_eq!(samples.len(), 20);
            for sample in samples {
                let (at, expected) = (sample[0].as_f64().unwrap() as f32, sample[1].as_f64().unwrap());
                let got = ours.sample(at) as f64;
                assert!((got - expected).abs() <= 0.01 * range, "{name} at {at}: ours {got}, Unity {expected}");
            }
        }
    }

    fn effects(targets: serde_json::Value) -> Expressions {
        Expressions::parse(&serde_json::json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": targets})).unwrap()
    }

    fn unposed(nodes: usize) -> super::super::Pose {
        super::super::Pose { rotations: vec![Quat::IDENTITY; nodes], translations: vec![Vec3::ZERO; nodes], scales: vec![Vec3::ONE; nodes], weights: Vec::new() }
    }

    fn euler(axes: &[usize]) -> Expressions {
        effects(serde_json::Value::Array(axes.iter().map(|&axis| serde_json::json!({"effect": {"rotation": {"node": 0, "axis": axis, "rest": [0, 0, 0]}}})).collect()))
    }

    #[test]
    fn rotation_and_translation_effects_parse() {
        let expressions = effects(serde_json::json!([
            {"effect": {"rotation": {"node": 1, "axis": 2, "rest": [10, 20, 30]}}, "default": 30},
            {"effect": {"rotation": {"node": 2, "axis": 3, "rest": [0, 0, 0, 1]}}, "default": 1},
            {"effect": {"translation": {"node": 1, "direction": [0, 0, -0.01]}}}
        ]));
        let effects: Vec<_> = expressions.targets.iter().map(|target| target.effect.clone()).collect();
        assert_eq!(effects, [
            Effect::Rotation(Rotation { node: 1, axis: 2, rest: vec![10.0, 20.0, 30.0] }),
            Effect::Rotation(Rotation { node: 2, axis: 3, rest: vec![0.0, 0.0, 0.0, 1.0] }),
            Effect::Translation { node: 1, direction: [0.0, 0.0, -0.01] },
        ]);
        let defaults: Vec<f32> = expressions.targets.iter().map(|target| target.default).collect();
        assert_eq!(defaults, [30.0, 1.0, 0.0]);
    }

    #[test]
    fn a_rotation_naming_a_bad_axis_or_two_rests_for_one_node_is_refused() {
        let refused = |targets: serde_json::Value| effects(targets).check(3, &[]).is_err();
        assert!(refused(serde_json::json!([{"effect": {"rotation": {"node": 0, "axis": 3, "rest": [0, 0, 0]}}}])));
        assert!(refused(serde_json::json!([{"effect": {"rotation": {"node": 0, "axis": 0, "rest": [0, 0]}}}])));
        assert!(refused(serde_json::json!([{"effect": {"rotation": {"node": 0, "axis": 0, "rest": [0, 0, 0]}}}, {"effect": {"rotation": {"node": 0, "axis": 1, "rest": [0, 5, 0]}}}])));
        assert!(!refused(serde_json::json!([{"effect": {"rotation": {"node": 0, "axis": 0, "rest": [0, 5, 0]}}}, {"effect": {"rotation": {"node": 0, "axis": 1, "rest": [0, 5, 0]}}}])));
    }

    #[test]
    fn unity_euler_angles_turn_about_z_then_x_then_y_and_mirror_into_gltf() {
        let turned = |axes: &[usize], values: &[f32]| {
            let mut pose = unposed(1);
            euler(axes).apply(values, &mut pose);
            pose.rotations[0]
        };
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!(turned(&[0], &[90.0]).abs_diff_eq(Quat::from_rotation_x(quarter), 1e-6), "a turn about x keeps its sign");
        assert!(turned(&[1], &[90.0]).abs_diff_eq(Quat::from_rotation_y(-quarter), 1e-6), "about y it flips");
        assert!(turned(&[2], &[90.0]).abs_diff_eq(Quat::from_rotation_z(-quarter), 1e-6), "about z it flips");
        let both = turned(&[0, 2], &[90.0, 90.0]);
        assert!((both * Vec3::X).abs_diff_eq(Vec3::NEG_Z, 1e-6), "z first, then x: {}", both * Vec3::X);
        let all = turned(&[0, 1, 2], &[30.0, 40.0, 50.0]);
        let expected = Quat::from_rotation_y(-40f32.to_radians()) * Quat::from_rotation_x(30f32.to_radians()) * Quat::from_rotation_z(-50f32.to_radians());
        assert!(all.abs_diff_eq(expected, 1e-6), "{all} vs {expected}");
    }

    #[test]
    fn a_rotation_at_its_default_leaves_the_pose_and_a_turn_composes_after_it() {
        let expressions = effects(serde_json::json!([
            {"effect": {"rotation": {"node": 0, "axis": 0, "rest": [10, 20, 30]}}, "default": 10},
            {"effect": {"rotation": {"node": 0, "axis": 1, "rest": [10, 20, 30]}}, "default": 20},
            {"effect": {"rotation": {"node": 1, "axis": 0, "rest": [0.1, 0.2, 0.3, 0.9]}}, "default": 0.1},
            {"effect": {"rotation": {"node": 2, "axis": 0, "rest": [0, 0, 0]}}}
        ]));
        let held = Quat::from_rotation_z(0.3);
        let mut pose = unposed(3);
        pose.rotations = vec![held; 3];
        let defaults: Vec<f32> = expressions.targets.iter().map(|target| target.default).collect();
        expressions.apply(&defaults, &mut pose);
        assert!(pose.rotations.iter().all(|rotation| rotation.abs_diff_eq(held, 1e-6)), "{:?}", pose.rotations);
        let mut pose = unposed(3);
        pose.rotations = vec![held; 3];
        expressions.apply(&[10.0, 20.0, 0.1, 90.0], &mut pose);
        let turn = held.inverse() * pose.rotations[2];
        assert!(turn.abs_diff_eq(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), 1e-6), "the turn applies in the node's own frame, after its pose: {turn}");
    }

    #[test]
    fn a_translation_moves_its_node_by_the_value_along_its_direction() {
        let expressions = effects(serde_json::json!([{"effect": {"translation": {"node": 1, "direction": [0, 0.5, -0.01]}}}]));
        let mut pose = unposed(2);
        pose.translations[1] = Vec3::new(1.0, 2.0, 3.0);
        expressions.apply(&[4.0], &mut pose);
        assert!(pose.translations[1].abs_diff_eq(Vec3::new(1.0, 4.0, 2.96), 1e-6));
        assert_eq!(pose.translations[0], Vec3::ZERO);
    }

    fn switched(springs: serde_json::Value, colliders: serde_json::Value) -> Result<Expressions, String> {
        Expressions::parse(&serde_json::json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": [{"effect": "switch", "default": 1}, {"effect": "switch"}], "springs": springs, "colliders": colliders}))
    }

    #[test]
    fn an_avatar_with_cairnfold_look_targets_loads() {
        let look = |field: &str| serde_json::json!({"node": 0, "rect": [0, 0, 1, 1], "field": field, "slot": 1, "multiply": true, "alpha": 0.5, "intensity": 1});
        let parsed = Expressions::parse(&serde_json::json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": [
            {"effect": {"look": [look("decal")]}, "default": 0.5}, {"effect": {"look": [look("matcapIntensity"), look("emissionHue")]}}
        ]}));
        assert!(parsed.as_ref().is_ok_and(|expressions| expressions.check(1, &[]).is_ok()), "{:?}", parsed.err());
    }

    #[test]
    fn every_effect_and_look_field_the_publish_guard_admits_is_one_the_overlay_reads() {
        for name in &read().effects {
            if let Err(error) = Effect::deserialize(serde_json::json!({name: null})) {
                assert!(!error.to_string().contains("unknown variant"), "{name}: {error}");
            }
        }
        let fields: Vec<Field> = read().looks.iter().map(|field| serde_json::from_value(serde_json::json!(field)).unwrap()).collect();
        assert_eq!(fields.iter().map(|&field| field as usize).collect::<BTreeSet<_>>(), (0..FIELDS).collect(), "each field once: {fields:?}");
    }

    #[test]
    fn a_target_or_look_region_the_overlay_does_not_read_holds_still_and_the_rest_load() {
        let expressions = effects(serde_json::json!([
            {"effect": {"glow": [{"node": 0}]}, "default": 1},
            {"effect": "shimmer"},
            {"effect": {"scale": {"node": 0, "axis": 1}}, "default": 1},
            {"effect": {"look": [{"node": 0, "rect": [0, 0, 1, 1], "field": "sparkle", "slot": 0}, {"node": 0, "rect": [0, 0, 1, 1], "field": "decal", "slot": 0}]}}
        ]));
        let effects: Vec<_> = expressions.targets.iter().map(|target| target.effect.clone()).collect();
        assert_eq!(effects, [
            Effect::Unread("glow".into()),
            Effect::Unread("shimmer".into()),
            Effect::Scale { node: 0, axis: 1 },
            Effect::Look(vec![LookRegion { node: 0, rect: [0.0, 0.0, 1.0, 1.0], field: Field::Decal, slot: 0 }]),
        ]);
        assert!(expressions.check(1, &[]).is_ok());
        let mut pose = unposed(1);
        expressions.apply(&[5.0, 5.0, 2.0, 1.0], &mut pose);
        assert_eq!(pose.scales[0], Vec3::new(1.0, 2.0, 1.0));
        assert!(Expressions::parse(&serde_json::json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": [{"effect": {"scale": {"node": 0}}}]})).is_err(), "a known effect stays strict");
    }

    #[test]
    fn a_spring_or_collider_is_idle_while_a_switch_it_waits_on_is_off() {
        let expressions = switched(serde_json::json!([{"joints": [3, 4], "when": [[0, 1]]}, {"joints": [5], "when": [[0, 1], [1, 1]]}]), serde_json::json!([{"collider": 2, "when": [[1, 1]]}])).unwrap();
        assert_eq!(expressions.idle(&[1.0, 1.0]), Idle::default());
        assert_eq!(expressions.idle(&[1.0, 0.0]), Idle { joints: HashSet::from([5]), colliders: HashSet::from([2]) });
        assert_eq!(expressions.idle(&[0.4, 0.6]), Idle { joints: HashSet::from([3, 4, 5]), colliders: HashSet::new() });
        assert!(expressions.check(6, &[]).is_ok());
        assert!(expressions.check(5, &[]).is_err(), "a switched joint past the nodes");
        assert!(switched(serde_json::json!([{"joints": [3], "when": [[2, 1]]}]), serde_json::json!([])).is_err(), "a spring waiting on a target the menu lacks");
        assert!(switched(serde_json::json!([]), serde_json::json!([{"collider": 0, "when": [[2, 1]]}])).is_err(), "a collider waiting on a target the menu lacks");
    }
}
