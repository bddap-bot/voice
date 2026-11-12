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
    DecalHue,
    ThemeHue,
    ThemeSaturation,
    ThemeValue,
    ThemeSpeed,
    #[serde(rename = "clearcoatST")]
    ClearCoatSt,
}

pub const FIELDS: usize = Field::ClearCoatSt as usize + 1;

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
pub struct HueShift {
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
    pub hue: Option<HueShift>,
    pub circle: Option<Circle>,
    /// The slot in `Look::themes` of the Poiyomi global theme colour its colour is, where it is one.
    #[serde(default)]
    pub theme: Option<usize>,
}

/// Poiyomi's decal Color Blend Modes, numbered as its `_DecalBlendType`; any other number its shader draws black.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(try_from = "u8")]
pub enum DecalBlend {
    Replace = 0,
    Darken = 1,
    Multiply = 2,
    Lighten = 5,
    Screen = 6,
    Subtract = 7,
    Add = 8,
    Overlay = 9,
    Mixed = 20,
}

impl TryFrom<u8> for DecalBlend {
    type Error = String;

    fn try_from(number: u8) -> Result<Self, String> {
        use DecalBlend::*;
        Ok(match number {
            0 => Replace,
            1 => Darken,
            2 => Multiply,
            5 => Lighten,
            6 => Screen,
            7 => Subtract,
            8 => Add,
            9 => Overlay,
            20 => Mixed,
            _ => return Err(format!("no decal blend mode {number}")),
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Decal {
    pub alpha: f32,
    pub intensity: f32,
    pub color_blend: DecalBlend,
    /// Its place in `Looks::decals`, not a glTF image index.
    pub image: usize,
    /// Set where a clip turns the decal's hue, so its image holds it unturned.
    pub hue: Option<HueShift>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Emission {
    pub color: [f32; 3],
    pub strength: f32,
    pub hue: f32,
    pub shifts_hue: bool,
    /// The slot in `Look::themes` of the Poiyomi global theme colour it glows in, where it glows in one.
    #[serde(default)]
    pub theme: Option<usize>,
}

/// A Poiyomi global theme colour an emission or matcap is drawn in: `hsv`, its colour's hue, saturation and value in
/// linear light below its `exposure`, and the offsets it rests at, which the `Theme*` fields of its slot move. The hue
/// also turns `speed` times every twenty seconds.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Theme {
    pub hsv: [f32; 3],
    pub exposure: f32,
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
    pub speed: f32,
}

/// Poiyomi's clear coat: a dielectric layer over the material whose highlight of the main light and reflection of its
/// surroundings add to its light.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClearCoat {
    pub strength: f32,
    pub smoothness: f32,
    pub reflection: f32,
    pub specular: f32,
    pub fresnel: f32,
    pub specular_tint: [f32; 3],
    pub reflection_tint: [f32; 3],
    pub st: [f32; 4],
    pub inverts: [bool; 4],
    pub gsaa: [f32; 2],
    /// Its place in `Looks::clearcoat_maps`, read through the mesh's second UV; a place past the maps reads as none.
    #[serde(deserialize_with = "read_map")]
    pub map: Option<usize>,
}

fn read_map<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<usize>, D::Error> {
    match i64::deserialize(deserializer)? {
        -1 => Ok(None),
        map if map >= 0 => Ok(Some(map as usize)),
        map => Err(serde::de::Error::custom(format!("a clear coat's map is -1 or a place in clearcoatMaps, got {map}"))),
    }
}

/// How a material takes the light: by the angle it meets each light at, through its `ramp` of brightnesses over the
/// half-Lambert term from 0 to 1 where it has one (VRChat's Toon Standard), its shade lent the albedo by `shadow_tint`;
/// or `Flat`, by the lights' colour alone, turning to its shade colours past the `shadow`'s border where it has one. A
/// ramp-lit or flat one is capped at `cap` where its shader caps it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "model")]
pub enum Lighting {
    Lit {
        #[serde(default)]
        ramp: Option<Vec<f32>>,
        #[serde(default, rename = "shadowTint")]
        shadow_tint: f32,
        #[serde(default)]
        cap: Option<f32>,
    },
    Flat { cap: Option<f32>, shadow: Option<Shadow> },
}

/// lilToon's shadow: past `border` (with `blur`) of the half-Lambert term the albedo turns to `Looks::shades`' first
/// colour, by `strength` under that layer's alpha.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Shadow {
    pub border: f32,
    pub blur: f32,
    pub strength: f32,
    pub second: Option<SecondShadow>,
    pub edge: Option<ShadowEdge>,
}

/// The deeper band, past its own border, turning the first shade towards the second by `weight`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SecondShadow {
    pub border: f32,
    pub blur: f32,
    pub weight: f32,
}

/// A band `range` wide on the lit side of the border, tinted towards the albedo by `color`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ShadowEdge {
    pub color: [f32; 3],
    pub range: f32,
}

/// lilToon's rim light, added where the surface turns from the view: `color` times `strength`, towards the albedo by
/// `main` and the light by `lit`, over the fresnel term raised to `power` past `border` (with `blur`), dimmed in shadow
/// by `shadow_mask`; on back faces too only where `backface` says.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rim {
    pub color: [f32; 3],
    pub strength: f32,
    pub main: f32,
    pub border: f32,
    pub blur: f32,
    pub power: f32,
    pub lit: f32,
    pub shadow_mask: f32,
    pub backface: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Look {
    /// The material's cell of the atlas, `[x0, y0, x1, y1]` in atlas UV; each of its decals spans it corner to corner.
    pub rect: [f32; 4],
    pub decals: [Option<Decal>; 4],
    pub matcaps: [Option<Matcap>; 4],
    pub emission: Option<Emission>,
    pub lighting: Lighting,
    pub rim: Option<Rim>,
    /// The global theme colours its emission and matcaps are drawn in, by slot.
    #[serde(default)]
    pub themes: [Option<Theme>; 4],
    pub clearcoat: Option<ClearCoat>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncodedLooks {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    materials: Vec<Look>,
    #[serde(default)]
    decals: Vec<usize>,
    #[serde(default)]
    matcaps: Vec<usize>,
    #[serde(default)]
    matcap_masks: Option<usize>,
    #[serde(default)]
    emission: Option<usize>,
    #[serde(default)]
    clearcoat_maps: Vec<usize>,
    #[serde(default)]
    outline: Option<usize>,
    #[serde(default)]
    outline_width: Option<f32>,
    #[serde(default)]
    shade: Option<usize>,
    #[serde(default)]
    shade2: Option<usize>,
    #[serde(default)]
    rim_tint: Option<usize>,
}

/// The inverted hull of the materials whose glTF extras say `outline`, over the atlas UV: its RGB the hull's colour,
/// its alpha the hull's width there as a share of `width`, in the model's own units.
#[derive(Clone, Debug)]
pub struct OutlineLayer {
    pub image: Image,
    pub width: f32,
}

/// What each atlas texel's material lays over the albedo: `index`'s red channel names a texel's entry in `materials`,
/// one-based, zero or no index for none. Every layer is a glTF image the extension names by index, each at its own size;
/// a clear coat map holds linear mask, smoothness, reflection and specular levels in its four channels. `shades` are
/// the first shade colours a shadowed look turns each texel to and, where some look has a second shade, the second's,
/// each spanning the atlas at its shape but carried at most as wide as the converter carries a glow map; `rim_tint`,
/// where a rim has a colour texture, the factor each texel's rim takes (white where none tints it), at the shades' size.
#[derive(Clone, Debug)]
pub struct Looks {
    pub index: Option<Image>,
    pub materials: Vec<Look>,
    pub decals: Vec<Image>,
    pub matcaps: Vec<Image>,
    pub matcap_masks: Option<Image>,
    pub emission: Option<Image>,
    pub clearcoat_maps: Vec<Image>,
    pub outline: Option<OutlineLayer>,
    pub shades: Vec<Image>,
    pub rim_tint: Option<Image>,
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
    Constraint { node: usize },
    /// The controller's own parameter, which the animator writes every step from the clips animating it.
    Parameter(String),
    Particle(ParticleEffect),
    /// An effect a newer converter writes: its target holds still rather than refusing the avatar.
    #[serde(skip)]
    Unread(String),
}

/// The value of one particle system's field a target sets: a curve's scale, a channel of its start colour or of its
/// materials' colour or emission, or which of its materials it draws with.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ParticleEffect {
    pub system: usize,
    pub field: ParticleField,
    #[serde(default)]
    pub end: End,
    #[serde(default)]
    pub channel: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ParticleField {
    Rate,
    Distance,
    Speed,
    Size,
    Lifetime,
    Gravity,
    Color,
    Material,
    Tint,
    Emission,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum End {
    Min,
    Max,
    #[default]
    Both,
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

/// A particle system the menu switches: it runs only while every switch in `when` is at its value.
#[derive(Clone, Debug, Deserialize)]
pub struct SwitchedParticles {
    pub system: usize,
    pub when: Vec<(usize, f32)>,
}

pub(super) fn off(when: &[(usize, f32)], values: &[f32]) -> bool {
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
    #[serde(default)]
    pub contacts: Vec<super::contact::Contact>,
    #[serde(default)]
    pub particles: Vec<SwitchedParticles>,
    #[serde(default, rename = "icons")]
    encoded_icons: Vec<usize>,
    #[serde(skip)]
    pub icons: Vec<Image>,
    #[serde(default, rename = "hueMask")]
    encoded_hue_mask: Option<usize>,
    #[serde(skip)]
    pub hue_mask: Option<Image>,
    #[serde(default, rename = "untinted")]
    encoded_untinted: Option<EncodedUntinted>,
    #[serde(skip)]
    pub untinted: Option<Untinted>,
    #[serde(default, rename = "looks")]
    encoded_looks: Option<EncodedLooks>,
    #[serde(skip)]
    pub looks: Option<Looks>,
}

#[derive(Clone, Debug, Deserialize)]
struct EncodedUntinted {
    image: Option<usize>,
    #[serde(default)]
    cells: Vec<[f32; 8]>,
}

/// The untinted colours of the cells a colour control recolours, laid in their own image: each cell its padded rect in
/// atlas UV then its rect in `image`'s UV, both `[x0, y0, x1, y1]`.
#[derive(Clone, Debug)]
pub struct Untinted {
    pub image: Image,
    pub cells: Vec<[f32; 8]>,
}

impl Untinted {
    /// Where the atlas's `uv` lies in `image`, through the first cell holding it; None outside every cell.
    pub fn at(&self, [u, v]: [f32; 2]) -> Option<[f32; 2]> {
        self.cells.iter().find(|cell| u >= cell[0] && v >= cell[1] && u <= cell[2] && v <= cell[3])
            .map(|cell| [cell[4] + (u - cell[0]) * (cell[6] - cell[4]) / (cell[2] - cell[0]), cell[5] + (v - cell[1]) * (cell[7] - cell[5]) / (cell[3] - cell[1])])
    }
}

fn looks(encoded: EncodedLooks, image: &impl Fn(usize) -> Result<Image, String>) -> Result<Looks, String> {
    let matcaps: Vec<Image> = encoded.matcaps.iter().map(|&at| image(at)).collect::<Result<_, _>>().map_err(|error| format!("{EXTENSION}: matcaps: {error}"))?;
    if matcaps.iter().any(|layer| (layer.width, layer.height) != (matcaps[0].width, matcaps[0].height)) {
        return Err(format!("{EXTENSION}: matcaps of differing sizes"));
    }
    let clearcoat_maps: Vec<Image> = encoded.clearcoat_maps.iter().map(|&at| image(at)).collect::<Result<_, _>>().map_err(|error| format!("{EXTENSION}: a clear coat map: {error}"))?;
    let mut materials = encoded.materials;
    for coat in materials.iter_mut().filter_map(|look| look.clearcoat.as_mut()) {
        coat.map = coat.map.filter(|&map| map < clearcoat_maps.len());
    }
    let looks = Looks {
        index: encoded.index.map(|at| image(at)).transpose().map_err(|error| format!("{EXTENSION}: look index: {error}"))?,
        decals: encoded.decals.iter().map(|&at| image(at)).collect::<Result<_, _>>().map_err(|error| format!("{EXTENSION}: a decal: {error}"))?,
        matcaps,
        matcap_masks: encoded.matcap_masks.map(|at| image(at)).transpose().map_err(|error| format!("{EXTENSION}: matcap masks: {error}"))?,
        emission: encoded.emission.map(|at| image(at)).transpose().map_err(|error| format!("{EXTENSION}: emission: {error}"))?,
        clearcoat_maps,
        outline: match (encoded.outline, encoded.outline_width) {
            (None, _) => None,
            (Some(at), Some(width)) if width.is_finite() && width > 0.0 => Some(OutlineLayer { image: image(at).map_err(|error| format!("{EXTENSION}: outline: {error}"))?, width }),
            (Some(_), _) => return Err(format!("{EXTENSION}: an outline layer without a positive outlineWidth")),
        },
        shades: match (encoded.shade, encoded.shade2) {
            (Some(first), second) => std::iter::once(image(first).map_err(|error| format!("{EXTENSION}: shade: {error}"))?).chain(second.map(|second| image(second).map_err(|error| format!("{EXTENSION}: shade2: {error}"))).transpose()?).collect(),
            (None, None) => Vec::new(),
            (None, Some(_)) => return Err(format!("{EXTENSION}: a second shade layer without the first")),
        },
        rim_tint: encoded.rim_tint.map(|at| image(at)).transpose().map_err(|error| format!("{EXTENSION}: rim tint: {error}"))?,
        materials,
    };
    let shadows: Vec<&Shadow> = looks.materials.iter().filter_map(|look| match &look.lighting {
        Lighting::Flat { shadow, .. } => shadow.as_ref(),
        Lighting::Lit { .. } => None,
    }).collect();
    let needed = if shadows.iter().any(|shadow| shadow.second.is_some()) { 2 } else { usize::from(!shadows.is_empty()) };
    if looks.shades.len() < needed {
        return Err(format!("{EXTENSION}: a shadowed look without its shade layers"));
    }
    let over_atlas = |layer: &Image| looks.index.as_ref().is_none_or(|index| (f64::from(layer.height) - f64::from(layer.width) * f64::from(index.height) / f64::from(index.width)).abs() < 1.0);
    if looks.rim_tint.is_some() && looks.materials.iter().all(|look| look.rim.is_none()) {
        return Err(format!("{EXTENSION}: a rim tint without a rim"));
    }
    let toon_layers = || looks.shades.iter().chain(&looks.rim_tint);
    if toon_layers().next().is_some_and(|first| !over_atlas(first) || toon_layers().any(|layer| (layer.width, layer.height) != (first.width, first.height))) {
        return Err(format!("{EXTENSION}: shade layers and rim tint not of one size over the atlas"));
    }
    let textures = looks.matcaps.len() as u32;
    let cell = |look: &Look| {
        let [x0, y0, x1, y1] = look.rect;
        let spanned = if look.decals.iter().any(Option::is_some) { x0 < x1 && y0 < y1 } else { x0 <= x1 && y0 <= y1 };
        spanned && look.rect.iter().all(|v| (0.0..=1.0).contains(v))
    };
    if !looks.materials.iter().all(cell) {
        return Err(format!("{EXTENSION}: a look's cell is not a rectangle of the atlas"));
    }
    let themed = |look: &Look| look.emission.iter().map(|emission| emission.theme).chain(look.matcaps.iter().flatten().map(|matcap| matcap.theme)).flatten().all(|slot| look.themes.get(slot).is_some_and(Option::is_some));
    if !looks.materials.iter().all(themed) {
        return Err(format!("{EXTENSION}: a look's emission or matcap is drawn in a theme colour the look does not carry"));
    }
    if looks.materials.iter().flat_map(|look| look.decals.iter().flatten()).any(|decal| decal.image >= looks.decals.len()) || looks.index.iter().flat_map(|index| index.rgba.chunks_exact(4)).any(|texel| texel[0] as usize > looks.materials.len()) || looks.materials.iter().flat_map(|look| look.matcaps.iter().flatten()).any(|matcap| matcap.texture.is_some_and(|texture| texture >= textures)) {
        return Err(format!("{EXTENSION}: a look names a decal, material or matcap it does not have"));
    }
    Ok(looks)
}

fn untinted(encoded: EncodedUntinted, image: &impl Fn(usize) -> Result<Image, String>) -> Result<Option<Untinted>, String> {
    let Some(at) = encoded.image else { return Ok(None) };
    let spans = |cell: &[f32; 8]| cell.iter().all(|v| v.is_finite()) && cell[0] < cell[2] && cell[1] < cell[3] && cell[4] < cell[6] && cell[5] < cell[7];
    if encoded.cells.len() > 255 || !encoded.cells.iter().all(spans) {
        return Err(format!("{EXTENSION}: untinted cells are more than 255 or not each a rectangle of the atlas and of their image"));
    }
    Ok(Some(Untinted { image: image(at).map_err(|error| format!("{EXTENSION}: untinted: {error}"))?, cells: encoded.cells }))
}

fn icon(at: usize, image: &impl Fn(usize) -> Result<Image, String>) -> Result<Image, String> {
    let mut image = image(at)?;
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
        let unread: BTreeSet<&str> = expressions.targets.iter().filter_map(|target| match &target.effect { Effect::Unread(name) => Some(name.as_str()), _ => None }).collect();
        if !unread.is_empty() {
            tracing::warn!("{EXTENSION}: targets of effects the overlay does not read hold still: {unread:?}");
        }
        fn icons_valid(controls: &[Control], icons: usize) -> bool {
            controls.iter().all(|control| control.icon.is_none_or(|icon| icon < icons) && icons_valid(&control.controls, icons))
        }
        if !icons_valid(&expressions.menu, expressions.encoded_icons.len()) {
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
            || expressions.shown.iter().flat_map(|shown| &shown.when).chain(expressions.springs.iter().flat_map(|spring| &spring.when)).chain(expressions.colliders.iter().flat_map(|collider| &collider.when)).chain(expressions.contacts.iter().flat_map(|contact| &contact.when)).chain(expressions.particles.iter().flat_map(|particles| &particles.when)).any(|&(target, _)| target >= targets)
        {
            return Err(format!("{EXTENSION}: a curve or switched node, spring, collider, contact or particle system names a target it does not have"));
        }
        Ok(expressions)
    }

    /// Decodes each layer from the glTF image the extension names.
    pub(super) fn read_layers(&mut self, image: impl Fn(usize) -> Result<Image, String>) -> Result<(), String> {
        self.icons = std::mem::take(&mut self.encoded_icons).into_iter().map(|at| icon(at, &image)).collect::<Result<_, _>>().map_err(|error| format!("{EXTENSION}: an icon: {error}"))?;
        self.hue_mask = self.encoded_hue_mask.take().map(|at| image(at)).transpose().map_err(|error| format!("{EXTENSION}: hue mask: {error}"))?;
        self.untinted = self.encoded_untinted.take().map(|encoded| untinted(encoded, &image)).transpose()?.flatten();
        self.looks = self.encoded_looks.take().map(|encoded| looks(encoded, &image)).transpose()?;
        Ok(())
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
            Effect::Constraint { node } => *node < nodes,
            Effect::Parameter(_) => true,
            Effect::Material { node, choices } => *node < nodes && !choices.is_empty(),
            Effect::Particle(effect) => effect.channel < match effect.field { ParticleField::Emission => 3, _ => 4 },
        });
        let contacts_fit = self.contacts.iter().all(|contact| contact.node < nodes && contact.shape.radius.is_finite() && contact.shape.radius >= 0.0 && contact.shape.offset.iter().chain(contact.shape.tail.iter().flatten()).all(|v| v.is_finite()));
        if !fits || !contacts_fit || self.shown.iter().any(|shown| shown.node >= nodes) || self.springs.iter().flat_map(|spring| &spring.joints).any(|&joint| joint >= nodes) {
            return Err(format!("{EXTENSION}: a target, shown node, switched spring or contact names a node, mesh or blend shape the model does not have"));
        }
        Ok(())
    }

    pub fn particle_effects(&self) -> impl Iterator<Item = (usize, ParticleEffect)> + '_ {
        self.targets.iter().enumerate().filter_map(|(at, target)| match &target.effect {
            Effect::Particle(effect) => Some((at, effect.clone())),
            _ => None,
        })
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
        let (off_springs, on_springs): (Vec<_>, Vec<_>) = self.springs.iter().partition(|spring| off(&spring.when, values));
        let bones = |springs: Vec<&SwitchedSpring>| springs.into_iter().map(|spring| spring.joints.iter().copied().collect()).collect();
        Idle {
            off: bones(off_springs),
            on: bones(on_springs),
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
                Effect::Constraint { node } => { pose.constraint_weights.insert(*node, value.clamp(0.0, 1.0)); }
                Effect::Parameter(_) | Effect::Material { .. } | Effect::Hue(_) | Effect::Color(_) | Effect::Tone(_) | Effect::Look(_) | Effect::Particle(_) | Effect::Unread(_) => {}
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
        for (target, &value) in expressions.targets.iter().zip(&out) {
            if let Effect::Parameter(name) = &target.effect {
                let integral = self.index.get(name).is_some_and(|&at| expressions.parameters[at].kind != ValueType::Float);
                self.set(name, if integral { value.round() } else { value });
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
        let playing: f32 = weights.iter().filter(|&&share| share > 0.0).sum();
        // A target only children at no weight key is left to the layers below.
        for target in targets {
            let keyed: Vec<(f32, f32)> = animated.iter().zip(&weights).filter(|&(_, &share)| share > 0.0).filter_map(|(part, &share)| part.get(&target).map(|&value| (share, value))).collect();
            if !keyed.is_empty() {
                let rest = if blend == Blend::Direct { 0.0 } else { (playing - keyed.iter().map(|(share, _)| share).sum::<f32>()) * expressions.targets[target].default };
                *into.entry(target).or_insert(0.0) += weight * (keyed.iter().map(|(share, value)| share * value).sum::<f32>() + rest);
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

    #[test]
    fn an_atlas_uv_reaches_the_untinted_colours_through_its_cell() {
        let untinted = Untinted { image: Image { width: 1, height: 1, rgba: vec![0; 4] }, cells: vec![[0.5, 0.0, 1.0, 0.5, 0.0, 0.5, 0.25, 1.0], [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.1, 0.1]] };
        assert_eq!(untinted.at([0.75, 0.25]), Some([0.125, 0.75]), "the first cell holding a texel maps it");
        assert_eq!(untinted.at([0.25, 0.75]), Some([0.025, 0.075]));
        assert_eq!(Untinted { cells: vec![[0.5, 0.0, 1.0, 0.5, 0.0, 0.0, 1.0, 1.0]], ..untinted }.at([0.25, 0.25]), None, "a texel outside every cell has no untinted colour");
    }

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
    fn two_physbones_taking_turns_on_a_bone_each_move_it_to_their_own_end() {
        let expressions = switched(serde_json::json!([{"joints": [3, 4, 6], "when": [[0, 1]]}, {"joints": [3, 5], "when": [[1, 1]]}]), serde_json::json!([])).unwrap();
        let first = expressions.idle(&[1.0, 0.0]);
        assert!(!first.still(3, Some(4)) && first.still(3, Some(5)), "the running one swings the shared bone toward its end, the other's segment stays");
        let second = expressions.idle(&[0.0, 1.0]);
        assert!(second.still(3, Some(4)) && !second.still(3, Some(5)) && second.still(4, Some(6)));
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
    fn a_1d_tree_blends_from_rest_but_leaves_a_target_only_a_child_at_no_weight_keys_to_the_layer_below() {
        let expressions = Expressions::parse(&serde_json::json!({
            "menu": [], "parameters": [{"name": "Green", "type": "float"}],
            "layers": [
                {"states": [{"motion": {"clip": 0}}]},
                {"states": [{"motion": {"blend": "1d", "parameters": ["Green"], "children": [{"motion": {"clip": 2}, "threshold": 0}, {"motion": {"clip": 1}, "threshold": 1}]}}]}
            ],
            "clips": [{"length": 1, "curves": [{"target": 0, "keys": [[0, 1]]}]}, {"length": 1, "curves": [{"target": 0, "keys": [[0, 2]]}]}, {"length": 1, "curves": []}],
            "targets": [{"effect": "switch", "default": 0.5}]
        }))
        .unwrap();
        let mut controller = Controller::new(&expressions, &Configuration::default());
        assert_eq!(controller.step(&expressions, 0.1), vec![1.0], "off: the lower layer's value stands");
        controller.set("Green", 0.5);
        assert_eq!(controller.step(&expressions, 0.1), vec![1.25], "halfway: half the clip's 2, half the default 0.5");
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
        super::super::Pose { rotations: vec![Quat::IDENTITY; nodes], translations: vec![Vec3::ZERO; nodes], scales: vec![Vec3::ONE; nodes], weights: Vec::new(), constraint_weights: HashMap::new() }
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
    fn a_decal_names_one_of_poiyomis_blend_modes() {
        let decal = |mode: &str| serde_json::from_str::<Decal>(&format!(r#"{{"alpha": 1, "intensity": 1, "image": 0, "hue": null, {mode}}}"#));
        assert_eq!(decal(r#""colorBlend": 20"#).unwrap().color_blend, DecalBlend::Mixed);
        assert!(decal(r#""colorBlend": 3"#).is_err(), "a number Poiyomi draws black is refused");
        assert!(decal(r#""multiply": true"#).is_err(), "a decal converted before decals named their mode is refused, not drawn replacing");
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
    fn a_look_drawn_in_a_theme_colour_it_does_not_carry_is_refused() {
        let theme = serde_json::json!({"slot": 1, "hsv": [0.0, 1.0, 1.0], "exposure": 1.0, "hue": 0.0, "saturation": 0.0, "value": 0.0, "speed": 0.0});
        let parse = |slot: usize, themes: serde_json::Value| {
            let mut material = serde_json::json!({"rect": [0, 0, 1, 1], "decals": [null, null, null, null], "matcaps": [null, null, null, null], "lighting": {"model": "lit"}, "themes": themes,
                "emission": {"color": [1, 0, 0], "strength": 1, "hue": 0, "shiftsHue": false, "theme": slot}});
            if material["themes"].is_null() {
                material.as_object_mut().unwrap().remove("themes");
            }
            looks(serde_json::from_value(serde_json::json!({"index": 0, "materials": [material]})).unwrap(), &|_| Ok(Image { width: 1, height: 1, rgba: vec![1, 0, 0, 255] }))
        };
        assert!(parse(1, serde_json::json!([null, theme, null, null])).is_ok_and(|looks| looks.materials[0].themes[1].is_some()), "a look carrying the theme its emission glows in loads");
        for (slot, themes) in [(0, serde_json::json!([null, theme, null, null])), (4, serde_json::json!([null, theme, null, null])), (1, serde_json::json!(null))] {
            assert!(parse(slot, themes.clone()).is_err(), "an emission in theme {slot} of {themes} is refused");
        }
    }

    #[test]
    fn a_shadowed_look_loads_with_the_shade_layers_its_shades_need_and_is_refused_without_them() {
        let sizes = [(4, 4), (2, 2), (4, 2)];
        let shadow = serde_json::json!({"border": 0.5, "blur": 0.1, "strength": 1, "second": {"border": 0.15, "blur": 0.1, "weight": 1}, "edge": {"color": [1, 0.1, 0], "range": 0.08}});
        let rim = serde_json::json!({"color": [0.66, 0.5, 0.48], "strength": 1, "main": 0, "border": 0.5, "blur": 0.65, "power": 3.5, "lit": 1, "shadowMask": 0.5, "backface": false});
        let parse = |lighting: serde_json::Value, layers: serde_json::Value| {
            let material = serde_json::json!({"rect": [0, 0, 1, 1], "decals": [null, null, null, null], "matcaps": [null, null, null, null], "emission": null, "lighting": lighting, "rim": rim});
            let mut encoded = serde_json::json!({"index": 0, "materials": [material]});
            encoded.as_object_mut().unwrap().extend(layers.as_object().unwrap().clone());
            looks(serde_json::from_value(encoded).unwrap(), &|at: usize| {
                let (width, height) = sizes[at];
                Ok(Image { width, height, rgba: [1, 0, 0, 255].repeat((width * height) as usize) })
            })
        };
        let shaded = parse(serde_json::json!({"model": "flat", "cap": 1, "shadow": shadow}), serde_json::json!({"shade": 1, "shade2": 1})).unwrap();
        let Lighting::Flat { shadow: Some(read), .. } = &shaded.materials[0].lighting else { panic!("{:?}", shaded.materials[0].lighting) };
        assert_eq!((read.second.as_ref().map(|second| second.weight), read.edge.as_ref().map(|edge| edge.range)), (Some(1.0), Some(0.08)));
        assert_eq!(shaded.materials[0].rim.as_ref().map(|rim| (rim.shadow_mask, rim.backface)), Some((0.5, false)));
        assert!(shaded.shades.len() == 2 && shaded.shades[0].width == 2, "the shade layers load narrower than the atlas");
        assert!(parse(serde_json::json!({"model": "flat", "cap": 1, "shadow": shadow}), serde_json::json!({"shade": 1, "shade2": 1, "rimTint": 1})).is_ok_and(|looks| looks.rim_tint.is_some_and(|tint| tint.width == 2)), "a rim tint at the shades' size loads");
        let rimless = serde_json::json!({"index": 0, "materials": [{"rect": [0, 0, 1, 1], "decals": [null, null, null, null], "matcaps": [null, null, null, null], "emission": null, "lighting": {"model": "flat", "cap": 1}, "rim": null}], "rimTint": 1});
        assert!(looks(serde_json::from_value(rimless).unwrap(), &|_| Ok(Image { width: 1, height: 1, rgba: vec![255; 4] })).is_err(), "a rim tint where no look draws a rim is refused");
        assert!(parse(serde_json::json!({"model": "flat", "cap": 1}), serde_json::json!({})).is_ok_and(|looks| looks.shades.is_empty()), "an unshadowed flat look needs no shade layers");
        let single = serde_json::json!({"model": "flat", "cap": 1, "shadow": {"border": 0.5, "blur": 0.1, "strength": 1, "second": null, "edge": null}});
        assert!(parse(single.clone(), serde_json::json!({"shade": 1})).is_ok_and(|looks| looks.shades.len() == 1), "a shadow without a second shade loads with the first shade layer alone");
        assert!(parse(single, serde_json::json!({"shade2": 1})).is_err(), "a second shade layer without the first is refused");
        for (layers, why) in [
            (serde_json::json!({}), "no shade layers"),
            (serde_json::json!({"shade": 0}), "the first alone, where it has a second shade"),
            (serde_json::json!({"shade": 0, "shade2": 1}), "layers of two sizes"),
            (serde_json::json!({"shade": 2, "shade2": 2}), "layers not the atlas's shape"),
            (serde_json::json!({"shade": 1, "shade2": 1, "rimTint": 0}), "a rim tint of another size"),
        ] {
            assert!(parse(serde_json::json!({"model": "flat", "cap": 1, "shadow": shadow}), layers).is_err(), "a shadowed look with {why} is refused");
        }
    }

    #[test]
    fn an_avatar_with_cairnfold_look_targets_loads() {
        let look = |field: &str| serde_json::json!({"node": 0, "rect": [0, 0, 1, 1], "field": field, "slot": 1, "colorBlend": 2, "alpha": 0.5, "intensity": 1});
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
        let still = |values: &[f32]| (2..7).filter(|&node| expressions.idle(values).still(node, Some(node + 1))).collect::<Vec<_>>();
        assert_eq!(still(&[1.0, 1.0]), Vec::<usize>::new());
        assert_eq!(expressions.idle(&[1.0, 0.0]).colliders, HashSet::from([2]));
        assert!(expressions.idle(&[1.0, 0.0]).still(5, None) && !expressions.idle(&[1.0, 0.0]).still(3, Some(4)));
        assert_eq!(still(&[0.4, 0.6]), vec![3]);
        assert!(expressions.idle(&[0.4, 0.6]).colliders.is_empty());
        assert!(expressions.check(6, &[]).is_ok());
        assert!(expressions.check(5, &[]).is_err(), "a switched joint past the nodes");
        assert!(switched(serde_json::json!([{"joints": [3], "when": [[2, 1]]}]), serde_json::json!([])).is_err(), "a spring waiting on a target the menu lacks");
        assert!(switched(serde_json::json!([]), serde_json::json!([{"collider": 0, "when": [[2, 1]]}])).is_err(), "a collider waiting on a target the menu lacks");
    }
}
