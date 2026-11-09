use glam::{FloatExt, Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
use serde_json::Value;

use super::expressions::{End, Expressions, ParticleEffect, ParticleField};
use super::{srgb_to_linear, Image};

pub const EXTENSION: &str = "UNITY_particles";
const MAX_SYSTEMS: usize = 64;
/// The most slots an avatar's systems hold together, and slots times the indices each draws.
const MAX_SLOTS: usize = 10000;
const MAX_SLOT_INDICES: usize = 375000;
const MAX_SAMPLES: usize = 64;
const MAX_BURSTS: usize = 8;
const MAX_SUB_EMITTERS: usize = 4;
const MAX_VERTICES: usize = 4096;
const MAX_INDICES: usize = 12288;
const QUAD_INDICES: usize = 6;
/// The longest a particle lives, in seconds, the fastest it starts, in metres a second, and the brightest a linear
/// colour channel is: past them the products the CPU and GPU form would leave f32's range.
const MAX_LIFE: f32 = 1e4;
const MAX_SPEED: f32 = 1e4;
const MAX_LINEAR: f32 = 1e4;
/// Past this a number would lose the precision of the GPU's 32-bit floats.
const LARGEST: f64 = 1e9;
/// Samples across each life curve the shader reads, and its rows: pairs of the low then the high end of each range.
pub const SAMPLES: usize = 16;
pub const ROWS: usize = 16;
const ROW_COLOUR: usize = 0;
const ROW_SIZE: usize = 2;
const ROW_PATH: usize = 4;
const ROW_VELOCITY: usize = 6;
const ROW_SPIN_PATH: usize = 8;
const ROW_NOISE_LIMIT: usize = 10;
const ROW_COLOUR_BY_SPEED: usize = 12;
const ROW_SIZE_BY_SPEED: usize = 14;
const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);
/// The birth time of a slot no particle holds.
pub const UNBORN: f32 = -1e9;
/// Unity's limit dampens speed past it by its share each thirtieth of a second.
const DAMPEN_RATE: f32 = 30.0;
/// The main thread's time each step gives an avatar's births; those past it wait for the next step.
const BIRTHS: std::time::Duration = std::time::Duration::from_micros(500);
/// The draw between a speed limit's two ends the shader and `Particles::position` both take, so they agree.
const LIMIT_DRAW: f32 = 0.5;
/// How many slots past a living one a birth looks for a free one, before it is dropped as a full system's is.
const SLOT_SEARCH: usize = 16;
/// Past this a system's clock starts again, its living particles' births moved with it, so the birth times the GPU
/// reads keep their precision.
const CLOCK_SPAN: f32 = 3600.0;

pub const VELOCITY: u32 = 1;
pub const VELOCITY_WORLD: u32 = 2;
pub const NOISE: u32 = 4;
pub const LIMIT: u32 = 8;
pub const SIZE_BY_SPEED: u32 = 16;
pub const COLOUR_BY_SPEED: u32 = 32;
pub const COLOUR_OVER_LIFE: u32 = 64;
pub const SPIN: u32 = 128;
pub const WORLD: u32 = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    min: Vec<f32>,
    max: Vec<f32>,
    scale: [f32; 2],
}

impl Curve {
    fn constant(low: f32, high: f32) -> Curve {
        Curve { min: vec![1.0], max: vec![1.0], scale: [low, high] }
    }

    fn value(&self, u: f32, k: f32) -> f32 {
        self.end(0, u).lerp(self.end(1, u), k)
    }

    fn end(&self, end: usize, u: f32) -> f32 {
        self.scale[end] * sample(if end == 0 { &self.min } else { &self.max }, u)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    min: Vec<Vec4>,
    max: Vec<Vec4>,
}

impl Gradient {
    fn white() -> Gradient {
        Gradient { min: vec![Vec4::ONE], max: vec![Vec4::ONE] }
    }

    fn value(&self, u: f32, k: f32) -> Vec4 {
        colour_at(&self.min, u).lerp(colour_at(&self.max, u), k)
    }

    fn end(&self, end: usize, u: f32) -> Vec4 {
        colour_at(if end == 0 { &self.min } else { &self.max }, u)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Point,
    Sphere,
    Hemisphere,
    Cone,
    Box,
    Circle,
    Edge,
}

#[derive(Clone, Debug, PartialEq)]
struct Shape {
    kind: Kind,
    radius: f32,
    thickness: f32,
    angle: f32,
    arc: f32,
    length: f32,
    matrix: Mat4,
    random_direction: f32,
    spherical: f32,
    random_position: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct Burst {
    time: f32,
    count: Curve,
    cycles: u32,
    interval: f32,
    probability: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct Velocity {
    axes: [Curve; 3],
    world: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Limit {
    speed: Curve,
    dampen: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct Noise {
    strength: Curve,
    frequency: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum On {
    Birth,
    Death,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Inherit {
    color: bool,
    size: bool,
    rotation: bool,
    lifetime: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sub {
    system: usize,
    on: On,
    inherit: Inherit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Billboard,
    Stretched,
    Horizontal,
    Vertical,
    Mesh(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    View,
    World,
    Local,
    Facing,
    Velocity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Opaque,
    Cutout,
    Alpha,
    Add,
    Premultiplied,
    Multiply,
}

impl Blend {
    pub fn solid(self) -> bool {
        matches!(self, Blend::Opaque | Blend::Cutout)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub color: Vec4,
    pub emission: Vec3,
    pub blend: Blend,
    pub cutoff: f32,
    /// Its place in `Definition::images`.
    pub image: Option<usize>,
    /// Unity's `[scale x, scale y, offset x, offset y]`.
    pub uv: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Renderer {
    pub draw: bool,
    pub mode: Mode,
    pub align: Align,
    pub pivot: Vec3,
    pub length: f32,
    pub velocity_scale: f32,
    /// The share of the view's height a quad may span.
    pub max_size: f32,
    pub materials: Vec<Material>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct System {
    pub node: usize,
    duration: f32,
    looped: bool,
    awake: bool,
    world: bool,
    sub: bool,
    delay: f32,
    pub max: usize,
    lifetime: Curve,
    speed: Curve,
    gravity: Curve,
    rate: Curve,
    distance: Curve,
    size: Vec<Curve>,
    rotation: Vec<Curve>,
    color: Gradient,
    bursts: Vec<Burst>,
    shape: Shape,
    velocity: Option<Velocity>,
    limit: Option<Limit>,
    inherit: Option<Curve>,
    noise: Option<Noise>,
    size_over_life: Option<Vec<Curve>>,
    color_over_life: Option<Gradient>,
    spin: Option<Vec<Curve>>,
    size_by_speed: Option<([f32; 2], Curve)>,
    color_by_speed: Option<([f32; 2], Gradient)>,
    subs: Vec<Sub>,
    pub renderer: Renderer,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub indices: Vec<u32>,
}

/// An avatar's particle systems as its file carries them, every field checked and each missing one given Unity's
/// default.
#[derive(Clone, Debug, Default)]
pub struct Definition {
    pub systems: Vec<System>,
    pub meshes: Vec<Mesh>,
    /// The images the materials name, each once.
    pub images: Vec<Image>,
}

fn finite(value: &Value) -> Option<f32> {
    value.as_f64().filter(|v| v.abs() <= LARGEST).map(|v| v as f32)
}

fn number(value: &Value, fallback: f32) -> f32 {
    finite(value).unwrap_or(fallback)
}

fn whole(value: &Value) -> Option<usize> {
    value.as_f64().filter(|v| *v >= 0.0 && v.fract() == 0.0 && *v <= LARGEST).map(|v| v as usize)
}

fn flag(value: &Value, fallback: bool) -> bool {
    if value.is_null() { fallback } else { value == &Value::Bool(true) }
}

fn floats(value: &Value) -> Option<Vec<f32>> {
    let values = value.as_array().filter(|values| (1..=MAX_SAMPLES).contains(&values.len()))?;
    values.iter().map(finite).collect()
}

fn curve(value: &Value, fallback: &Curve) -> Curve {
    let scale = value["scale"].as_array().filter(|scale| scale.len() == 2).and_then(|scale| Some([finite(&scale[0])?, finite(&scale[1])?]));
    match (floats(&value["min"]), floats(&value["max"]), scale) {
        (Some(min), Some(max), Some(scale)) => Curve { min, max, scale },
        _ => fallback.clone(),
    }
}

fn axes(value: &Value, fallback: &Curve) -> Vec<Curve> {
    match value.as_array() {
        Some(curves) if curves.len() == 3 => curves.iter().map(|c| curve(c, fallback)).collect(),
        _ => vec![curve(value, fallback)],
    }
}

fn colours(value: &Value) -> Option<Vec<Vec4>> {
    let values = value.as_array().filter(|values| (1..=MAX_SAMPLES).contains(&values.len()))?;
    values.iter().map(|colour| vector::<4>(colour).map(|c| linear(Vec4::from(c)))).collect()
}

/// An sRGB colour with linear alpha, as the file carries it, made linear.
fn linear(c: Vec4) -> Vec4 {
    Vec4::new(linear_channel(c.x), linear_channel(c.y), linear_channel(c.z), c.w)
}

fn linear_channel(srgb: f32) -> f32 {
    srgb_to_linear(srgb).min(MAX_LINEAR)
}

fn gradient(value: &Value) -> Gradient {
    match (colours(&value["min"]), colours(&value["max"])) {
        (Some(min), Some(max)) => Gradient { min, max },
        _ => Gradient::white(),
    }
}

fn vector<const N: usize>(value: &Value) -> Option<[f32; N]> {
    let values = value.as_array().filter(|values| values.len() == N)?;
    let mut out = [0.0; N];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = finite(value)?;
    }
    Some(out)
}

fn colour(value: &Value, fallback: Vec4) -> Vec4 {
    vector::<4>(value).map(Vec4::from).or_else(|| vector::<3>(value).map(|c| Vec3::from(c).extend(1.0))).map_or(fallback, linear)
}

fn mesh(value: &Value) -> Result<Mesh, String> {
    let positions = value["positions"].as_array().ok_or("a mesh without positions")?;
    let count = positions.len() / 3;
    if positions.len() % 3 != 0 || count > MAX_VERTICES {
        return Err(format!("a mesh of {} position numbers, not up to {MAX_VERTICES} vertices", positions.len()));
    }
    let positions: Vec<f32> = positions.iter().map(finite).collect::<Option<_>>().ok_or("a mesh position is not a number")?;
    let indices = value["indices"].as_array().ok_or("a mesh without indices")?;
    if indices.len() > MAX_INDICES {
        return Err(format!("a mesh of {} indices, over {MAX_INDICES}", indices.len()));
    }
    let indices: Vec<u32> = indices.iter().map(|i| whole(i).filter(|&i| i < count).map(|i| i as u32)).collect::<Option<_>>().ok_or("a mesh index is not a vertex")?;
    if indices.len() % 3 != 0 {
        return Err("a mesh's indices are not triangles".into());
    }
    let uvs = value["uvs"].as_array().filter(|uvs| uvs.len() == 2 * count).and_then(|uvs| uvs.iter().map(finite).collect::<Option<Vec<f32>>>());
    Ok(Mesh {
        positions: positions.chunks_exact(3).map(|p| Vec3::new(p[0], p[1], p[2])).collect(),
        uvs: uvs.map_or_else(|| vec![Vec2::ZERO; count], |uvs| uvs.chunks_exact(2).map(|uv| Vec2::new(uv[0], uv[1])).collect()),
        indices,
    })
}

/// Reads the extension, from a player's file and so checked throughout: an error drops every system, never the avatar.
/// `image` decodes a glTF image of the `images` the file has.
pub fn parse(value: Option<&Value>, nodes: usize, images: usize, image: impl Fn(usize) -> Result<Image, String>) -> Result<Definition, String> {
    let Some(value) = value else { return Ok(Definition::default()) };
    let systems = value["systems"].as_array().ok_or("systems is not a list")?;
    if systems.len() > MAX_SYSTEMS {
        return Err(format!("{} systems, over {MAX_SYSTEMS}", systems.len()));
    }
    let meshes = match value["meshes"].as_array() {
        Some(meshes) => meshes.iter().map(mesh).collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };
    let mut named = Vec::new();
    let mut parsed = Vec::new();
    for (at, s) in systems.iter().enumerate() {
        parsed.push(system(s, nodes, meshes.len(), systems.len(), images, &mut named).map_err(|error| format!("system {at}: {error}"))?);
    }
    for at in 0..parsed.len() {
        let subs = std::mem::take(&mut parsed[at].subs);
        parsed[at].subs = subs.into_iter().filter(|sub| !leads_to(&parsed, sub.system, at)).collect();
        for sub in parsed[at].subs.clone() {
            parsed[sub.system].sub = true;
        }
    }
    let slots: usize = parsed.iter().map(|s| s.max).sum();
    let indices: usize = parsed.iter().map(|s| s.max * match s.renderer.mode { Mode::Mesh(mesh) => meshes[mesh].indices.len(), _ => QUAD_INDICES }).sum();
    let share = (MAX_SLOTS as f64 / slots.max(1) as f64).min(MAX_SLOT_INDICES as f64 / indices.max(1) as f64).min(1.0);
    for system in &mut parsed {
        system.max = (system.max as f64 * share) as usize;
    }
    let images = named.iter().map(|&at| image(at)).collect::<Result<Vec<_>, _>>()?;
    Ok(Definition { systems: parsed, meshes, images })
}

/// Whether system `from`, or a sub-emitter it names however far down, is system `to`.
fn leads_to(systems: &[System], from: usize, to: usize) -> bool {
    let mut seen = vec![false; systems.len()];
    let mut pending = vec![from];
    while let Some(at) = pending.pop() {
        if at == to {
            return true;
        }
        if !std::mem::replace(&mut seen[at], true) {
            pending.extend(systems[at].subs.iter().map(|sub| sub.system));
        }
    }
    false
}

fn system(s: &Value, nodes: usize, meshes: usize, systems: usize, images: usize, named: &mut Vec<usize>) -> Result<System, String> {
    if !s.is_object() {
        return Err("not an object".into());
    }
    let node = whole(&s["node"]).filter(|&node| node < nodes).ok_or("its node is not in the file")?;
    let zero = Curve::constant(0.0, 0.0);
    let one = Curve::constant(1.0, 1.0);
    let bursts = s["bursts"].as_array().into_iter().flatten().filter(|b| b.is_object()).take(MAX_BURSTS).map(|b| Burst {
        time: number(&b["time"], 0.0).max(0.0),
        count: curve(&b["count"], &zero),
        cycles: number(&b["cycles"], 1.0).clamp(1.0, 1000.0) as u32,
        interval: number(&b["interval"], 0.01).max(0.001),
        probability: number(&b["probability"], 1.0).clamp(0.0, 1.0),
    }).collect();
    let shape = &s["shape"];
    let kind = match shape["type"].as_str() {
        Some("sphere") => Kind::Sphere,
        Some("hemisphere") => Kind::Hemisphere,
        Some("cone") => Kind::Cone,
        Some("box") => Kind::Box,
        Some("circle") => Kind::Circle,
        Some("edge") => Kind::Edge,
        _ => Kind::Point,
    };
    let tau = std::f32::consts::TAU;
    let shape = Shape {
        kind,
        radius: number(&shape["radius"], 1.0).max(0.0),
        thickness: number(&shape["thickness"], 1.0).clamp(0.0, 1.0),
        angle: number(&shape["angle"], 0.0).clamp(0.0, tau / 4.0),
        arc: number(&shape["arc"], tau).clamp(0.0, tau),
        length: number(&shape["length"], 0.0).max(0.0),
        matrix: vector::<16>(&shape["matrix"]).map_or(Mat4::IDENTITY, |m| Mat4::from_cols_array(&m)),
        random_direction: number(&shape["randomDirection"], 0.0).clamp(0.0, 1.0),
        spherical: number(&shape["spherical"], 0.0).clamp(0.0, 1.0),
        random_position: number(&shape["randomPosition"], 0.0).max(0.0),
    };
    let by = |key: &str| -> Option<[f32; 2]> {
        let by = s.get(key).filter(|by| by.is_object())?;
        let [low, high] = by["range"].as_array().filter(|r| r.len() == 2).map_or([0.0, 1.0], |r| [number(&r[0], 0.0), number(&r[1], 1.0)]);
        Some([low, if high > low { high } else { low + (low.abs() * 1e-3).max(1.0) }])
    };
    let subs = s["subEmitters"].as_array().into_iter().flatten().filter_map(|sub| {
        let system = whole(&sub["system"]).filter(|&system| system < systems)?;
        let on = match sub["on"].as_str()? { "birth" => On::Birth, "death" => On::Death, _ => return None };
        let mut inherit = Inherit::default();
        for property in sub["inherit"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            match property {
                "color" => inherit.color = true,
                "size" => inherit.size = true,
                "rotation" => inherit.rotation = true,
                "lifetime" => inherit.lifetime = true,
                _ => {}
            }
        }
        Some(Sub { system, on, inherit })
    }).take(MAX_SUB_EMITTERS).collect();
    let r = &s["renderer"];
    let mut draw = flag(&r["draw"], false);
    let mode = match r["mode"].as_str() {
        Some("stretched") => Mode::Stretched,
        Some("horizontal") => Mode::Horizontal,
        Some("vertical") => Mode::Vertical,
        Some("mesh") => match whole(&r["mesh"]).filter(|&mesh| mesh < meshes) {
            Some(mesh) => Mode::Mesh(mesh),
            None => {
                draw = false;
                Mode::Billboard
            }
        },
        _ => Mode::Billboard,
    };
    let align = match r["align"].as_str() {
        Some("world") => Align::World,
        Some("local") => Align::Local,
        Some("facing") => Align::Facing,
        Some("velocity") => Align::Velocity,
        _ => Align::View,
    };
    let mut materials = Vec::new();
    for m in r["materials"].as_array().into_iter().flatten() {
        if !m.is_object() {
            return Err("a material is not an object".into());
        }
        let blend = match m["blend"].as_str() {
            Some("opaque") => Blend::Opaque,
            Some("cutout") => Blend::Cutout,
            Some("add") => Blend::Add,
            Some("premultiplied") => Blend::Premultiplied,
            Some("multiply") => Blend::Multiply,
            _ => Blend::Alpha,
        };
        let image = match m.get("image") {
            None => None,
            Some(image) => {
                let image = whole(image).filter(|&image| image < images).ok_or("a material's image is not in the file")?;
                Some(named.iter().position(|&known| known == image).unwrap_or_else(|| {
                    named.push(image);
                    named.len() - 1
                }))
            }
        };
        materials.push(Material {
            color: colour(&m["color"], Vec4::ONE),
            emission: colour(&m["emission"], Vec4::W).truncate(),
            blend,
            cutoff: number(&m["cutoff"], 0.5).clamp(0.0, 1.0),
            image,
            uv: vector::<4>(&m["uv"]).unwrap_or([1.0, 1.0, 0.0, 0.0]),
        });
    }
    draw &= !materials.is_empty();
    Ok(System {
        node,
        duration: number(&s["duration"], 5.0).max(0.01),
        looped: flag(&s["loop"], true),
        awake: flag(&s["awake"], true),
        world: flag(&s["world"], false),
        sub: false,
        delay: number(&s["delay"], 0.0).max(0.0),
        max: number(&s["max"], 1000.0).clamp(0.0, MAX_SLOTS as f32) as usize,
        lifetime: curve(&s["lifetime"], &Curve::constant(5.0, 5.0)),
        speed: curve(&s["speed"], &Curve::constant(5.0, 5.0)),
        gravity: curve(&s["gravity"], &zero),
        rate: curve(&s["rate"], &zero),
        distance: curve(&s["distance"], &zero),
        size: axes(&s["size"], &one),
        rotation: axes(&s["rotation"], &zero),
        color: gradient(&s["color"]),
        bursts,
        shape,
        velocity: s.get("velocity").filter(|v| v.is_object()).map(|v| Velocity { axes: [curve(&v["x"], &zero), curve(&v["y"], &zero), curve(&v["z"], &zero)], world: flag(&v["world"], false) }),
        limit: s.get("limit").filter(|l| l.is_object()).map(|l| Limit { speed: curve(&l["speed"], &one), dampen: number(&l["dampen"], 1.0).clamp(0.0, 1.0) }),
        inherit: s.get("inherit").map(|i| curve(i, &zero)),
        noise: s.get("noise").filter(|n| n.is_object()).map(|n| Noise { strength: curve(&n["strength"], &one), frequency: number(&n["frequency"], 0.5).clamp(0.0, 100.0) }),
        size_over_life: s.get("sizeOverLife").map(|v| axes(v, &one)),
        color_over_life: s.get("colorOverLife").map(gradient),
        spin: s.get("spin").map(|v| axes(v, &zero)),
        size_by_speed: by("sizeBySpeed").map(|range| (range, curve(&s["sizeBySpeed"]["size"], &one))),
        color_by_speed: by("colorBySpeed").map(|range| (range, gradient(&s["colorBySpeed"]["color"]))),
        subs,
        renderer: Renderer {
            draw,
            mode,
            align,
            pivot: vector::<3>(&r["pivot"]).map_or(Vec3::ZERO, Vec3::from),
            length: number(&r["length"], 2.0),
            velocity_scale: number(&r["velocityScale"], 0.0),
            max_size: number(&r["maxSize"], 0.5).max(0.0),
            materials,
        },
    })
}

impl System {
    pub fn flags(&self) -> u32 {
        let mut flags = 0;
        if let Some(velocity) = &self.velocity {
            flags |= VELOCITY | if velocity.world { VELOCITY_WORLD } else { 0 };
        }
        for (has, bit) in [(self.noise.is_some(), NOISE), (self.limit.is_some(), LIMIT), (self.size_by_speed.is_some(), SIZE_BY_SPEED), (self.color_by_speed.is_some(), COLOUR_BY_SPEED), (self.color_over_life.is_some(), COLOUR_OVER_LIFE), (self.spin.is_some(), SPIN), (self.world, WORLD)] {
            if has { flags |= bit; }
        }
        flags
    }

    pub fn speed_ranges(&self) -> [f32; 4] {
        let [a, b] = self.size_by_speed.as_ref().map_or([0.0, 1.0], |(range, _)| *range);
        let [c, d] = self.color_by_speed.as_ref().map_or([0.0, 1.0], |(range, _)| *range);
        [a, b, c, d]
    }

    pub fn noise_frequency(&self) -> f32 {
        self.noise.as_ref().map_or(0.0, |noise| noise.frequency)
    }

    /// How fast speed past the limit falls away, per second.
    pub fn damping(&self) -> f32 {
        self.limit.as_ref().map_or(0.0, |limit| -(1.0 - limit.dampen).max(1e-3).ln() * DAMPEN_RATE)
    }

    /// The life curves as the shader reads them: `ROWS` rows of `SAMPLES`.
    pub fn curves(&self) -> Vec<[f32; 4]> {
        let mut out = vec![[1.0; 4]; ROWS * SAMPLES];
        let mut put = |row: usize, values: &dyn Fn(usize, f32) -> Vec4| {
            for end in 0..2 {
                for x in 0..SAMPLES {
                    out[(row + end) * SAMPLES + x] = values(end, x as f32 / (SAMPLES - 1) as f32).to_array();
                }
            }
        };
        let three = |curves: &[Curve], end: usize, u: f32| match curves {
            [lone] => Vec3::splat(lone.end(end, u)),
            curves => Vec3::new(curves[0].end(end, u), curves[1].end(end, u), curves[2].end(end, u)),
        };
        put(ROW_COLOUR, &|end, u| self.color_over_life.as_ref().map_or(Vec4::ONE, |g| g.end(end, u)));
        put(ROW_SIZE, &|end, u| self.size_over_life.as_ref().map_or(Vec3::ONE, |c| three(c, end, u)).extend(1.0));
        let velocity = |end: usize, u: f32| self.velocity.as_ref().map_or(Vec3::ZERO, |v| Vec3::new(v.axes[0].end(end, u), v.axes[1].end(end, u), v.axes[2].end(end, u)));
        put(ROW_VELOCITY, &|end, u| velocity(end, u).extend(1.0));
        put(ROW_PATH, &|end, u| integral(&|t| velocity(end, t), u).extend(1.0));
        let spin = |end: usize, u: f32| match self.spin.as_deref() {
            None => Vec3::ZERO,
            Some([lone]) => Vec3::new(0.0, 0.0, lone.end(end, u)),
            Some(curves) => three(curves, end, u),
        };
        put(ROW_SPIN_PATH, &|end, u| integral(&|t| spin(end, t), u).extend(1.0));
        put(ROW_NOISE_LIMIT, &|end, u| Vec4::new(self.noise.as_ref().map_or(0.0, |n| n.strength.end(end, u)), self.limit.as_ref().map_or(0.0, |l| l.speed.end(end, u)), 0.0, 1.0));
        put(ROW_COLOUR_BY_SPEED, &|end, u| self.color_by_speed.as_ref().map_or(Vec4::ONE, |(_, g)| g.end(end, u)));
        put(ROW_SIZE_BY_SPEED, &|end, u| Vec3::splat(self.size_by_speed.as_ref().map_or(1.0, |(_, c)| c.end(end, u))).extend(1.0));
        out
    }
}

/// The integral of `f` over `[0, u]` by the trapezoid rule, as fine as the samples it is read between.
fn integral(f: &dyn Fn(f32) -> Vec3, u: f32) -> Vec3 {
    let steps = ((u * (SAMPLES - 1) as f32 * 4.0).round() as usize).max(1);
    let h = u / steps as f32;
    (0..steps).map(|k| 0.5 * h * (f(k as f32 * h) + f((k + 1) as f32 * h))).sum()
}

fn sample(samples: &[f32], u: f32) -> f32 {
    if samples.len() == 1 {
        return samples[0];
    }
    let x = u.clamp(0.0, 1.0) * (samples.len() - 1) as f32;
    let i = (x as usize).min(samples.len() - 2);
    samples[i].lerp(samples[i + 1], x - i as f32)
}

fn colour_at(samples: &[Vec4], u: f32) -> Vec4 {
    let x = u.clamp(0.0, 1.0) * (samples.len() - 1) as f32;
    let i = (x as usize).min(samples.len().saturating_sub(2));
    samples[i].lerp(samples[(i + 1).min(samples.len() - 1)], x - i as f32)
}

/// One particle as the shader takes it, written once at its birth: start position, velocity, size and rotation (in
/// world axes or its emitter's) with its birth time, lifetime, random seed and gravity in their fourth components, and
/// its start colour, linear.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub start: [f32; 4],
    pub velocity: [f32; 4],
    pub size: [f32; 4],
    pub turn: [f32; 4],
    pub color: [f32; 4],
}

impl Instance {
    fn birth(&self) -> f32 { self.start[3] }
    fn life(&self) -> f32 { self.velocity[3] }
    fn alive(&self, now: f32) -> bool { self.birth() != UNBORN && now - self.birth() < self.life() }
}

pub const EMPTY: Instance = Instance { start: [0.0, 0.0, 0.0, UNBORN], velocity: [0.0, 0.0, 0.0, 1.0], size: [0.0; 4], turn: [0.0; 4], color: [0.0; 4] };

struct Follow {
    child: usize,
    on: On,
    inherit: Inherit,
    parent: usize,
    born: f32,
    time: f32,
    pending: f32,
    fired: [u32; MAX_BURSTS],
    last: Vec3,
}

struct Run {
    definition: System,
    when: Vec<(usize, f32)>,
    on: bool,
    chosen: usize,
    active: bool,
    playing: bool,
    /// The clock its particles' birth times are read on.
    now: f32,
    time: f32,
    /// The births its rates and bursts owe and the budget has not yet made, a fraction toward the next included.
    pending: f32,
    fired: [u32; MAX_BURSTS],
    frame: Mat4,
    velocity: Vec3,
    placed: bool,
    slot: usize,
    instances: Vec<Instance>,
    following: Vec<Follow>,
    written: Vec<u32>,
    /// When the last particle born dies.
    until: f32,
}

impl Run {
    fn stop(&mut self) {
        self.playing = false;
        self.following.clear();
        for (slot, instance) in self.instances.iter_mut().enumerate() {
            if instance.birth() != UNBORN {
                *instance = EMPTY;
                self.written.push(slot as u32);
            }
        }
        self.until = 0.0;
    }

    /// Starts its clock again at zero, its living particles' births moved with it and every other slot cleared, so
    /// none born on the old clock shows on the new.
    fn rebase(&mut self) {
        let shift = self.now;
        self.now = 0.0;
        self.until -= shift;
        for follow in &mut self.following {
            follow.born -= shift;
        }
        for (slot, instance) in self.instances.iter_mut().enumerate() {
            if instance.alive(shift) {
                instance.start[3] -= shift;
            } else if instance.birth() != UNBORN {
                *instance = EMPTY;
            } else {
                continue;
            }
            self.written.push(slot as u32);
        }
    }
}

/// Runs an avatar's particle systems in a world fixed in the tracking space, at the avatar's own scale. The CPU decides
/// when particles are born, where and with what, and writes each once; the shader works out where a living one is.
pub struct Particles {
    systems: Vec<Run>,
    effects: Vec<(usize, ParticleEffect)>,
    last: Vec<f32>,
    random: fastrand::Rng,
    /// The map from this world into the avatar's model space at the last step.
    to_model: Mat4,
    /// The time a step's births may take, and when this step's began.
    births: std::time::Duration,
    births_from: std::time::Instant,
}

impl Particles {
    pub fn new(definition: &Definition, menu: Option<&Expressions>) -> Particles {
        let mut systems: Vec<Run> = definition.systems.iter().map(|d| Run {
            definition: d.clone(),
            when: Vec::new(),
            on: true,
            chosen: 0,
            active: false,
            playing: false,
            now: 0.0,
            time: 0.0,
            pending: 0.0,
            fired: [0; MAX_BURSTS],
            frame: Mat4::IDENTITY,
            velocity: Vec3::ZERO,
            placed: false,
            slot: 0,
            instances: vec![EMPTY; d.max],
            following: Vec::new(),
            written: Vec::new(),
            until: 0.0,
        }).collect();
        let mut effects = Vec::new();
        if let Some(menu) = menu {
            let count = systems.len();
            for switched in menu.particles.iter().filter(|switched| switched.system < count) {
                systems[switched.system].when = switched.when.clone();
            }
            effects = menu.particle_effects().filter(|(_, effect)| effect.system < count).collect();
        }
        Particles { systems, effects, last: Vec::new(), random: fastrand::Rng::with_seed(0x5eed), to_model: Mat4::IDENTITY, births: BIRTHS, births_from: std::time::Instant::now() }
    }

    pub fn len(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// System `at` as the menu's effects have set it.
    pub fn system(&self, at: usize) -> &System {
        &self.systems[at].definition
    }

    /// Stops every system and clears its particles, as if the avatar were put on again.
    pub fn reset(&mut self) {
        for run in &mut self.systems {
            run.stop();
            (run.active, run.placed) = (false, false);
        }
    }

    /// Takes the menu's `values`: which systems its switches run, and each particle effect's value where it changed. A
    /// tint or emission is set on every material a clip may choose, as Unity's animator sets whichever its renderer draws.
    pub fn apply(&mut self, values: &[f32]) {
        self.last.resize(values.len(), f32::NAN);
        for (target, effect) in &self.effects {
            let Some(&value) = values.get(*target) else { continue };
            if self.last[*target] == value {
                continue;
            }
            let run = &mut self.systems[effect.system];
            let d = &mut run.definition;
            let channel = effect.channel;
            let colour = if channel == 3 { value } else { linear_channel(value) };
            match effect.field {
                ParticleField::Material => run.chosen = (value.round().max(0.0) as usize).min(d.renderer.materials.len().saturating_sub(1)),
                ParticleField::Tint => d.renderer.materials.iter_mut().for_each(|m| m.color[channel] = colour),
                ParticleField::Emission => d.renderer.materials.iter_mut().for_each(|m| m.emission[channel] = colour),
                ParticleField::Color => {
                    for (end, samples) in [(End::Min, &mut d.color.min), (End::Max, &mut d.color.max)] {
                        if effect.end == end || effect.end == End::Both {
                            samples.iter_mut().for_each(|sample| sample[channel] = colour);
                        }
                    }
                }
                field => {
                    let curves = match field {
                        ParticleField::Rate => std::slice::from_mut(&mut d.rate),
                        ParticleField::Distance => std::slice::from_mut(&mut d.distance),
                        ParticleField::Speed => std::slice::from_mut(&mut d.speed),
                        ParticleField::Size => &mut d.size[..],
                        ParticleField::Lifetime => std::slice::from_mut(&mut d.lifetime),
                        _ => std::slice::from_mut(&mut d.gravity),
                    };
                    for c in curves {
                        match effect.end {
                            End::Min => c.scale[0] = value,
                            End::Max => c.scale[1] = value,
                            End::Both => c.scale = [value, value],
                        }
                    }
                }
            }
        }
        self.last.copy_from_slice(values);
        for run in &mut self.systems {
            run.on = !super::expressions::off(&run.when, values);
        }
    }

    /// Runs a step of `delta` seconds with the avatar's nodes at `worlds`, placed in the tracking space by `body`.
    pub fn step(&mut self, worlds: &[Mat4], body: Mat4, delta: f32) {
        self.births_from = std::time::Instant::now();
        let scale = body.x_axis.truncate().length();
        let into = Mat4::from_scale(Vec3::splat(1.0 / scale)) * body;
        self.to_model = into.inverse();
        let mut moved = vec![0.0; self.systems.len()];
        // Every system is placed and switched before any emits, so a parent's first particles find their sub-emitters on.
        for (at, run) in self.systems.iter_mut().enumerate() {
            run.now += delta;
            let frame = rigid(into * worlds[run.definition.node]);
            let (origin, from) = (frame.w_axis.truncate(), run.frame.w_axis.truncate());
            run.velocity = if run.placed && delta > 0.0 { (origin - from) / delta } else { Vec3::ZERO };
            moved[at] = if run.placed { origin.distance(from) } else { 0.0 };
            (run.frame, run.placed) = (frame, true);
            if run.on != run.active {
                run.active = run.on;
                if run.on && run.definition.awake {
                    run.playing = true;
                    (run.time, run.pending) = (0.0, 0.0);
                    run.fired.fill(0);
                } else if !run.on {
                    run.stop();
                }
            }
        }
        // Particles that died this step give their death bursts before births take their slots.
        for at in 0..self.systems.len() {
            self.follow(at, delta);
        }
        for (at, moved) in moved.into_iter().enumerate() {
            let run = &self.systems[at];
            if run.active && run.playing && !run.definition.sub {
                self.run(at, delta, moved);
            }
        }
        for run in &mut self.systems {
            if run.now > CLOCK_SPAN {
                run.rebase();
            }
        }
    }

    fn run(&mut self, at: usize, delta: f32, moved: f32) {
        let run = &mut self.systems[at];
        let d = &run.definition;
        let before = run.time - d.delay;
        run.time += delta;
        let mut time = run.time - d.delay;
        if time < 0.0 {
            return;
        }
        let random = &mut self.random;
        let mut burst = 0;
        if !d.looped && time > d.duration {
            run.playing = false;
            time = d.duration;
        } else if d.looped && (before.max(0.0) / d.duration).floor() != (time / d.duration).floor() {
            burst = bursts(&d.bursts, &mut run.fired, d.duration, d.max, random);
            run.fired.fill(0);
        }
        let cycle = if d.looped { time % d.duration } else { time };
        let u = cycle / d.duration;
        burst = burst.saturating_add(bursts(&d.bursts, &mut run.fired, cycle, d.max, random));
        run.pending = owed(run.pending, [d.rate.value(u, random.f32()), delta], [d.distance.value(u, random.f32()), moved], burst, d.max);
        let (count, frame, velocity) = (run.pending as usize, run.frame, run.velocity);
        let made = self.emit_up_to(count, |particles| particles.emit(at, frame, velocity, u, None));
        let run = &mut self.systems[at];
        run.pending = left(run.pending, made);
    }

    /// Calls `emit` up to `count` times while this step's births have time left: how many it made, or none where a
    /// full system refused one.
    fn emit_up_to(&mut self, count: usize, mut emit: impl FnMut(&mut Particles) -> bool) -> Option<usize> {
        let mut made = 0;
        while made < count && self.births_from.elapsed() < self.births {
            if !emit(self) {
                return None;
            }
            made += 1;
        }
        Some(made)
    }

    /// Emits each child's particles from the living particles of system `at` it follows, and from those that died.
    fn follow(&mut self, at: usize, delta: f32) {
        let mut following = std::mem::take(&mut self.systems[at].following);
        following.retain_mut(|f| {
            let parent = self.systems[at].instances[f.parent];
            let child = f.child;
            if parent.birth() != f.born || !self.systems[child].active {
                return false;
            }
            let age = self.systems[at].now - f.born;
            let position = self.position(at, f.parent, age);
            let died = age >= parent.life();
            let frame = Mat4::from_rotation_translation(self.systems[child].frame.to_scale_rotation_translation().1, position);
            let inherit = f.inherit;
            if f.on == On::Death {
                if died {
                    let d = &self.systems[child].definition;
                    let count = bursts(&d.bursts, &mut [0; MAX_BURSTS], 0.0, d.max, &mut self.random);
                    self.emit_up_to(count, |particles| particles.emit(child, frame, Vec3::ZERO, 0.0, Some((parent, inherit))));
                }
                return !died;
            }
            let d = &self.systems[child].definition;
            f.time += delta;
            if died || (!d.looped && f.time > d.duration) {
                return false;
            }
            let cycle = if d.looped { f.time % d.duration } else { f.time };
            let u = cycle / d.duration;
            let burst = bursts(&d.bursts, &mut f.fired, cycle, d.max, &mut self.random);
            f.pending = owed(f.pending, [d.rate.value(u, self.random.f32()), delta], [d.distance.value(u, self.random.f32()), position.distance(f.last)], burst, d.max);
            f.last = position;
            let made = self.emit_up_to(f.pending as usize, |particles| particles.emit(child, frame, Vec3::ZERO, u, Some((parent, inherit))));
            f.pending = left(f.pending, made);
            true
        });
        following.append(&mut self.systems[at].following);
        self.systems[at].following = following;
    }

    /// Writes one particle of system `at` born now from an emitter at `frame` moving at `velocity`, `u` through its
    /// cycle; a sub-emitter's takes what its `inherit` names from its parent particle. False where no slot near the
    /// next is free, as a full system emits none.
    fn emit(&mut self, at: usize, frame: Mat4, velocity: Vec3, u: f32, from: Option<(Instance, Inherit)>) -> bool {
        let random = &mut self.random;
        let run = &mut self.systems[at];
        let now = run.now;
        let slots = run.instances.len();
        let Some(slot) = (0..SLOT_SEARCH.min(slots)).map(|k| (run.slot + k) % slots).find(|&slot| !run.instances[slot].alive(now)) else { return false };
        run.slot = (slot + 1) % slots;
        let d = &run.definition;
        let (shaped, heading) = shape(&d.shape, random);
        let place = d.shape.matrix;
        let mut start = frame.transform_point3(place.transform_point3(shaped));
        let mut heading = frame.transform_vector3(place.transform_vector3(heading).normalize_or(Vec3::Z)) * d.speed.value(u, random.f32()).clamp(-MAX_SPEED, MAX_SPEED);
        if let Some(inherit) = &d.inherit {
            heading += velocity * inherit.value(u, random.f32());
        }
        heading = heading.clamp_length_max(MAX_SPEED);
        let mut life = d.lifetime.value(u, random.f32());
        let mut size = start_axes(&d.size, u, true, random);
        let mut turn = start_axes(&d.rotation, u, false, random);
        let mut color = d.color.value(u, random.f32());
        if let Some((parent, inherit)) = from {
            if inherit.color { color *= Vec4::from(parent.color); }
            if inherit.size { size *= Vec4::from(parent.size).truncate(); }
            if inherit.rotation { turn += Vec4::from(parent.turn).truncate(); }
            if inherit.lifetime { life *= parent.life(); }
        }
        let life = life.clamp(1e-3, MAX_LIFE);
        if !d.world {
            let back = run.frame.inverse();
            start = back.transform_point3(start);
            heading = back.transform_vector3(heading);
        }
        let seed = random.f32();
        let gravity = d.gravity.value(u, random.f32());
        run.instances[slot] = Instance {
            start: start.extend(now).to_array(),
            velocity: heading.extend(life).to_array(),
            size: size.extend(seed).to_array(),
            turn: turn.extend(gravity).to_array(),
            color: color.to_array(),
        };
        run.written.push(slot as u32);
        run.until = run.until.max(now + life);
        let subs = run.definition.subs.len();
        if subs == 0 {
            return true;
        }
        let origin = self.position(at, slot, 0.0);
        for k in 0..subs {
            let sub = self.systems[at].definition.subs[k];
            if self.systems[sub.system].active {
                self.systems[at].following.push(Follow { child: sub.system, on: sub.on, inherit: sub.inherit, parent: slot, born: now, time: 0.0, pending: 0.0, fired: [0; MAX_BURSTS], last: origin });
            }
        }
        true
    }

    /// Where slot `slot` of system `at` is in the world `age` seconds after its birth: the shader's closed form without
    /// its velocity curves and noise.
    fn position(&self, at: usize, slot: usize, age: f32) -> Vec3 {
        let run = &self.systems[at];
        let d = &run.definition;
        let instance = &run.instances[slot];
        let heading = Vec4::from(instance.velocity).truncate();
        let down = if d.world { GRAVITY } else { run.frame.inverse().transform_vector3(GRAVITY) };
        let mut moved = heading * age;
        let damping = d.damping();
        if let Some(limit) = d.limit.as_ref().filter(|_| damping > 0.0) {
            let limit = limit.speed.value(0.0, LIMIT_DRAW);
            if heading.length() > limit {
                let held = heading.normalize() * limit;
                moved = held * age + (heading - held) * (1.0 - (-damping * age).exp()) / damping;
            }
        }
        let at = Vec4::from(instance.start).truncate() + moved + 0.5 * down * instance.turn[3] * age * age;
        if d.world { at } else { run.frame.transform_point3(at) }
    }

    /// The time on system `at`'s clock.
    pub fn now(&self, at: usize) -> f32 {
        self.systems[at].now
    }

    pub fn to_model(&self) -> Mat4 {
        self.to_model
    }

    #[cfg(test)]
    fn alive(&self, at: usize) -> usize {
        let run = &self.systems[at];
        run.instances.iter().filter(|instance| instance.alive(run.now)).count()
    }

    /// Whether system `at` has a particle to draw.
    pub fn shown(&self, at: usize) -> bool {
        let run = &self.systems[at];
        run.active && run.definition.renderer.draw && run.now < run.until
    }

    pub fn emitter(&self, at: usize) -> Mat4 {
        self.systems[at].frame
    }

    pub fn material(&self, at: usize) -> &Material {
        let run = &self.systems[at];
        &run.definition.renderer.materials[run.chosen]
    }

    /// Hands each particle written since the last call to `write`, by system and slot.
    pub fn written(&mut self, mut write: impl FnMut(usize, usize, &Instance)) {
        for (at, run) in self.systems.iter_mut().enumerate() {
            for slot in run.written.drain(..) {
                write(at, slot as usize, &run.instances[slot as usize]);
            }
        }
    }
}

/// `m`'s rotation and translation without its scale; a node scaled to nothing along an axis keeps only its place.
fn rigid(m: Mat4) -> Mat4 {
    let origin = m.w_axis.truncate();
    let axes = [m.x_axis, m.y_axis, m.z_axis].map(Vec4::truncate);
    if axes.iter().any(|axis| axis.length_squared() < 1e-12) {
        return Mat4::from_translation(origin);
    }
    let [x, y, z] = axes.map(Vec3::normalize);
    let x = if x.cross(y).dot(z) < 0.0 { -x } else { x };
    Mat4::from_rotation_translation(Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize(), origin)
}

/// `pending` births with those a `[rate, seconds]` and a `[per metre, metres]` owe and `burst` more, as many as `most`
/// at most; a negative rate owes none.
fn owed(pending: f32, [rate, seconds]: [f32; 2], [per_metre, metres]: [f32; 2], burst: usize, most: usize) -> f32 {
    let more = |per: f32, over: f32| per.max(0.0) as f64 * over.max(0.0) as f64;
    (pending as f64 + more(rate, seconds) + more(per_metre, metres) + burst as f64).min(most as f64) as f32
}

/// The births still owed of `pending` once `made` are made; where a full system refused one, the rest are dropped as
/// Unity's are.
fn left(pending: f32, made: Option<usize>) -> f32 {
    made.map_or(pending.fract(), |made| pending - made as f32)
}

fn bursts(bursts: &[Burst], fired: &mut [u32], t: f32, most: usize, random: &mut fastrand::Rng) -> usize {
    let mut count = 0usize;
    for (burst, fired) in bursts.iter().zip(fired) {
        while *fired < burst.cycles && burst.time + *fired as f32 * burst.interval <= t {
            *fired += 1;
            if random.f32() < burst.probability {
                count = count.saturating_add((burst.count.value(0.0, random.f32()).round().max(0.0) as usize).min(most));
            }
        }
    }
    count.min(most)
}

/// A start size or rotation: one curve for all three axes of a size and Z alone of a rotation, or one per axis.
fn start_axes(curves: &[Curve], u: f32, size: bool, random: &mut fastrand::Rng) -> Vec3 {
    match curves {
        [lone] => {
            let v = lone.value(u, random.f32());
            if size { Vec3::splat(v) } else { Vec3::new(0.0, 0.0, v) }
        }
        curves => Vec3::new(curves[0].value(u, random.f32()), curves[1].value(u, random.f32()), curves[2].value(u, random.f32())),
    }
}

fn unit(random: &mut fastrand::Rng) -> Vec3 {
    let z = random.f32() * 2.0 - 1.0;
    let turn = random.f32() * std::f32::consts::TAU;
    let across = (1.0 - z * z).sqrt();
    Vec3::new(across * turn.cos(), across * turn.sin(), z)
}

/// A point in a shape and the way a particle leaves it, in the shape's own axes, facing +Z.
fn shape(shape: &Shape, random: &mut fastrand::Rng) -> (Vec3, Vec3) {
    let r = shape.radius;
    let inward = |random: &mut fastrand::Rng| 1.0 - shape.thickness * random.f32();
    let (mut at, mut heading) = (Vec3::ZERO, Vec3::Z);
    match shape.kind {
        Kind::Sphere | Kind::Hemisphere => {
            heading = unit(random);
            if shape.kind == Kind::Hemisphere {
                heading.z = heading.z.abs();
            }
            at = heading * r * inward(random);
        }
        Kind::Cone => {
            let out = inward(random).sqrt();
            let turn = random.f32() * shape.arc;
            let tilt = shape.angle * out;
            heading = Vec3::new(turn.cos() * tilt.sin(), turn.sin() * tilt.sin(), tilt.cos());
            at = Vec3::new(turn.cos(), turn.sin(), 0.0) * r * out + heading * shape.length * random.f32();
        }
        Kind::Box => at = Vec3::new(random.f32() - 0.5, random.f32() - 0.5, random.f32() - 0.5),
        Kind::Circle => {
            let out = inward(random).sqrt();
            let turn = random.f32() * shape.arc;
            heading = Vec3::new(turn.cos(), turn.sin(), 0.0);
            at = heading * r * out;
        }
        Kind::Edge => {
            at = Vec3::new((random.f32() * 2.0 - 1.0) * r, 0.0, 0.0);
            heading = Vec3::Y;
        }
        Kind::Point => {}
    }
    if shape.random_direction > 0.0 {
        heading = heading.lerp(unit(random), shape.random_direction);
    }
    if shape.spherical > 0.0 && at.length() > 0.0 {
        heading = heading.lerp(at.normalize(), shape.spherical);
    }
    if shape.random_position > 0.0 {
        at += unit(random) * shape.random_position * random.f32();
    }
    (at, if heading.length() > 1e-6 { heading } else { Vec3::Z })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(extension: Value) -> Result<Definition, String> {
        parse(Some(&extension), 2, 1, |_| Ok(Image { width: 1, height: 1, rgba: vec![255; 4] }))
    }

    fn system(fields: Value) -> Value {
        let mut system = json!({"node": 1, "renderer": {"draw": true, "materials": [{"blend": "add"}]}, "lifetime": {"min": [1], "max": [1], "scale": [10, 10]}, "loop": false});
        for (key, value) in fields.as_object().unwrap() {
            system[key] = value.clone();
        }
        system
    }

    fn burst(count: f32) -> Value {
        json!([{"time": 0, "count": {"min": [1], "max": [1], "scale": [count, count]}}])
    }

    fn running(systems: Vec<Value>, menu: Option<&Expressions>) -> Particles {
        let definition = read(json!({"systems": systems})).unwrap();
        let mut particles = Particles::new(&definition, menu);
        particles.births = std::time::Duration::MAX;
        if let Some(menu) = menu {
            particles.apply(&menu.targets.iter().map(|target| target.default).collect::<Vec<_>>());
        }
        particles
    }

    fn worlds() -> [Mat4; 2] {
        [Mat4::IDENTITY, Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0))]
    }

    #[test]
    fn a_system_missing_its_fields_takes_unitys_defaults() {
        let definition = read(json!({"systems": [{"node": 0}]})).unwrap();
        let s = &definition.systems[0];
        assert_eq!((s.duration, s.looped, s.awake, s.world, s.sub, s.max), (5.0, true, true, false, false, 1000));
        assert_eq!(s.lifetime, Curve::constant(5.0, 5.0));
        assert_eq!(s.size, vec![Curve::constant(1.0, 1.0)]);
        assert_eq!(s.shape.kind, Kind::Point);
        assert!(!s.renderer.draw, "a system without materials runs unseen");
    }

    #[test]
    fn a_malformed_extension_is_refused_whole() {
        let refused = [
            json!({"systems": {}}),
            json!({"systems": [{"node": 2}]}),
            json!({"systems": [{"node": -1}]}),
            json!({"systems": [{"node": 0.5}]}),
            json!({"systems": [{"node": 0, "renderer": {"materials": [3]}}]}),
            json!({"systems": [{"node": 0, "renderer": {"materials": [{"image": 1}]}}]}),
            json!({"systems": [{"node": 0}], "meshes": [{"positions": [0, 0], "indices": []}]}),
            json!({"systems": [{"node": 0}], "meshes": [{"positions": [0, 0, 0], "indices": [0, 0, 1]}]}),
            json!({"systems": [{"node": 0}], "meshes": [{"positions": vec![0.0; 3 * (MAX_VERTICES + 1)], "indices": []}]}),
            json!({"systems": [{"node": 0}], "meshes": [{"positions": [0, 0, 0], "indices": vec![0; MAX_INDICES + 3]}]}),
            json!({"systems": vec![json!({"node": 0}); MAX_SYSTEMS + 1]}),
        ];
        for extension in refused {
            assert!(read(extension.clone()).is_err(), "{extension}");
        }
    }

    #[test]
    fn out_of_range_fields_are_clamped_or_defaulted_and_counts_capped() {
        let long: Vec<f32> = vec![1.0; MAX_SAMPLES + 1];
        let definition = read(json!({"systems": [system(json!({
            "max": 1e9, "duration": -3, "speed": {"min": long, "max": [1], "scale": [1, 1]}, "rate": {"min": [1], "max": [1], "scale": [1, "x"]},
            "bursts": vec![json!({"time": 0}); MAX_BURSTS + 3], "shape": {"type": "torus", "thickness": 7, "angle": 9},
            "renderer": {"draw": true, "mode": "mesh", "mesh": 0, "materials": [{"blend": "glow", "cutoff": 3, "uv": [1, 2]}]}
        }))]})).unwrap();
        let s = &definition.systems[0];
        assert_eq!(s.max, MAX_SLOTS);
        assert_eq!(s.duration, 0.01);
        assert_eq!(s.speed, Curve::constant(5.0, 5.0), "a curve of too many samples is its default");
        assert_eq!(s.rate, Curve::constant(0.0, 0.0));
        assert_eq!(s.bursts.len(), MAX_BURSTS);
        assert_eq!((s.shape.kind, s.shape.thickness, s.shape.angle), (Kind::Point, 1.0, std::f32::consts::FRAC_PI_2));
        assert!(!s.renderer.draw, "a mesh the file lacks draws nothing");
        assert_eq!((s.renderer.materials[0].blend, s.renderer.materials[0].cutoff, s.renderer.materials[0].uv), (Blend::Alpha, 1.0, [1.0, 1.0, 0.0, 0.0]));
    }

    #[test]
    fn a_system_another_names_is_its_sub_emitter_and_one_leading_back_is_dropped() {
        let sub = |to: usize| json!({"system": to, "on": "birth"});
        let definition = read(json!({"systems": [
            system(json!({"subEmitters": [sub(1), sub(0)]})),
            system(json!({"subEmitters": [sub(2)]})),
            system(json!({"subEmitters": [sub(0), sub(3), sub(3), sub(3), sub(3), sub(3)]})),
            system(json!({}))
        ]})).unwrap();
        let named = |at: usize| definition.systems[at].subs.iter().map(|sub| sub.system).collect::<Vec<_>>();
        assert_eq!(named(0), Vec::<usize>::new(), "itself, and 1, which led back through 2, go");
        assert_eq!((named(1), named(2)), (vec![2], vec![0, 3, 3, 3]), "once 0 names none it closes no cycle; four at most");
        assert_eq!(definition.systems.iter().map(|s| s.sub).collect::<Vec<_>>(), [true, false, true, true]);
    }

    #[test]
    fn slots_past_the_avatars_caps_shrink_every_system_alike_and_huge_numbers_are_defaults() {
        let big = |max: f32| system(json!({"max": max}));
        let definition = read(json!({"systems": [big(8000.0), big(4000.0), big(4000.0)]})).unwrap();
        assert_eq!(definition.systems.iter().map(|s| s.max).collect::<Vec<_>>(), [5000, 2500, 2500]);
        let mesh = json!({"positions": vec![0.0; 3 * 3], "indices": vec![0; 150]});
        let definition = read(json!({"systems": [system(json!({"max": 5000, "renderer": {"draw": true, "mode": "mesh", "mesh": 0, "materials": [{}]}}))], "meshes": [mesh]})).unwrap();
        assert_eq!(definition.systems[0].max, 2500, "a hundred and fifty indices a slot");
        let definition = read(json!({"systems": [system(json!({"max": 8000}))], "meshes": [{"positions": vec![0.0; 3 * MAX_VERTICES], "indices": []}]})).unwrap();
        assert_eq!(definition.systems[0].max, 8000, "a quad's six indices a slot, however many vertices a mesh it does not draw has");
        let definition = read(json!({"systems": [system(json!({"duration": 1e10, "max": 1e12}))]})).unwrap();
        assert_eq!((definition.systems[0].duration, definition.systems[0].max), (5.0, 1000));
    }

    #[test]
    fn huge_values_stay_within_what_the_gpu_reads() {
        let huge = json!({"min": [1], "max": [1], "scale": [1e9, 1e9]});
        let definition = read(json!({"systems": [system(json!({
            "bursts": [{"time": 0, "count": huge}], "max": 5, "lifetime": huge, "speed": huge, "inherit": huge,
            "color": {"min": [[1e9, 1e9, 1e9, 1]], "max": [[1e9, 1e9, 1e9, 1]]}, "sizeBySpeed": {"range": [1e9, 1e9]}
        }))]})).unwrap();
        let [low, high, ..] = definition.systems[0].speed_ranges();
        assert!(high > low, "a range has width where its ends meet: {low} {high}");
        let mut particles = Particles::new(&definition, None);
        particles.births = std::time::Duration::MAX;
        particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        particles.step(&worlds(), Mat4::from_translation(Vec3::new(1e9, 0.0, 0.0)), 0.05);
        assert_eq!(particles.alive(0), 5, "a burst gives at most its system's max");
        let mut written = Vec::new();
        particles.written(|_, _, instance| written.push(*instance));
        for instance in written {
            assert!(instance.life() <= MAX_LIFE && Vec4::from(instance.velocity).truncate().length() <= MAX_SPEED * 1.001 && Vec4::from(instance.color).max_element() <= MAX_LINEAR, "{instance:?}");
        }
    }

    #[test]
    fn an_emitter_scaled_to_nothing_emits_from_its_place_unturned() {
        let shaped = system(json!({"bursts": burst(4.0), "shape": {"type": "cone", "angle": 0.3, "radius": 0.2}, "speed": {"min": [1], "max": [1], "scale": [1, 1]}}));
        for scale in [Vec3::ZERO, Vec3::new(0.0, 1.0, 1.0), Vec3::new(1e-30, 1e-30, 1e-30)] {
            let node = Mat4::from_scale_rotation_translation(scale, Quat::from_rotation_y(1.0), Vec3::new(0.0, 1.0, 0.0));
            let mut particles = running(vec![shaped.clone()], None);
            particles.step(&[Mat4::IDENTITY, node], Mat4::IDENTITY, 0.05);
            assert_eq!(particles.alive(0), 4);
            assert_eq!(particles.emitter(0), Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0)), "{scale}");
            assert!((0..4).all(|slot| particles.position(0, slot, 1.0).is_finite()));
            particles.written(|_, _, instance| assert!(instance.start.iter().chain(&instance.velocity).all(|v| v.is_finite()), "{instance:?}"));
        }
        let mirrored = Mat4::from_scale_rotation_translation(Vec3::new(-2.0, 2.0, 2.0), Quat::from_rotation_y(1.0), Vec3::ZERO);
        let frame = rigid(mirrored);
        assert!((frame.determinant() - 1.0).abs() < 1e-5 && frame.y_axis.abs_diff_eq(mirrored.y_axis / 2.0, 1e-5), "a mirror keeps a turn: {frame}");
    }

    #[test]
    fn a_negative_rate_emits_nothing_and_owes_nothing_after() {
        let mut particles = running(vec![system(json!({"loop": true, "duration": 10, "rate": {"min": [1], "max": [1], "scale": [-50, -50]}, "distance": {"min": [1], "max": [1], "scale": [-50, -50]}}))], None);
        for step in 0..10 {
            particles.step(&worlds(), Mat4::from_translation(Vec3::new(step as f32 * 0.1, 0.0, 0.0)), 0.1);
        }
        assert_eq!((particles.alive(0), particles.systems[0].pending), (0, 0.0));
        particles.systems[0].definition.rate = Curve::constant(20.0, 20.0);
        for _ in 0..10 {
            particles.step(&worlds(), Mat4::IDENTITY, 0.1);
        }
        assert!((19..=20).contains(&particles.alive(0)), "only what a second at twenty owes: {}", particles.alive(0));
    }

    #[test]
    fn a_steady_emitters_clock_starts_again_without_losing_or_reviving_a_particle() {
        let steady = system(json!({"loop": true, "duration": 10, "max": 2000, "world": true, "rate": {"min": [1], "max": [1], "scale": [2, 2]}, "lifetime": {"min": [1], "max": [1], "scale": [300, 300]}}));
        let mut particles = running(vec![steady], None);
        let mut gpu = vec![EMPTY; 2000];
        let mut rebased = false;
        for step in 0..400 {
            let before = particles.now(0);
            particles.step(&worlds(), Mat4::IDENTITY, 10.0);
            rebased |= particles.now(0) < before;
            particles.written(|_, slot, instance| gpu[slot] = *instance);
            let now = particles.now(0);
            assert!(now <= CLOCK_SPAN);
            let drawn = gpu.iter().filter(|i| now - i.birth() >= 0.0 && now - i.birth() < i.life()).count();
            assert_eq!(drawn, particles.alive(0), "the GPU draws exactly the living at step {step}");
            if step > 40 {
                assert!((580..=620).contains(&drawn), "{drawn} at step {step}");
            }
        }
        assert!(rebased);
    }

    #[test]
    fn a_death_burst_gives_at_most_its_childs_max() {
        let parent = system(json!({"bursts": burst(2.0), "subEmitters": [{"system": 1, "on": "death"}], "lifetime": {"min": [1], "max": [1], "scale": [0.1, 0.1]}}));
        let child = system(json!({"bursts": burst(1e8), "max": 5}));
        let mut particles = running(vec![parent, child], None);
        for _ in 0..4 {
            particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        }
        assert_eq!((particles.alive(0), particles.alive(1)), (0, 5));
    }

    #[test]
    fn colours_are_made_linear_once_at_read() {
        let definition = read(json!({"systems": [system(json!({"color": {"min": [[0.5, 0.5, 0.5, 0.5]], "max": [[0.5, 0.5, 0.5, 0.5]]}, "renderer": {"draw": true, "materials": [{"color": [0.5, 1, 1, 0.5], "emission": [0.5, 0, 0]}]}}))]})).unwrap();
        let s = &definition.systems[0];
        let half = srgb_to_linear(0.5);
        assert_eq!(s.color.min[0], Vec4::new(half, half, half, 0.5));
        assert_eq!((s.renderer.materials[0].color, s.renderer.materials[0].emission), (Vec4::new(half, 1.0, 1.0, 0.5), Vec3::new(half, 0.0, 0.0)));
    }

    #[test]
    fn births_past_the_steps_time_wait_for_the_next_and_a_full_ring_drops_them() {
        let mut particles = running(vec![system(json!({"bursts": burst(30.0), "max": 20}))], None);
        particles.births = std::time::Duration::ZERO;
        particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        assert_eq!(particles.alive(0), 0);
        particles.births = std::time::Duration::MAX;
        particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        assert_eq!(particles.alive(0), 20, "the burst waited, as much of it as the system holds");
        assert_eq!(particles.systems[0].pending, 0.0);
    }

    #[test]
    fn a_loop_fires_the_bursts_its_old_cycle_left_before_starting_the_next() {
        let looped = system(json!({"loop": true, "duration": 1, "bursts": [{"time": 0.9, "count": {"min": [1], "max": [1], "scale": [5, 5]}}]}));
        let mut particles = running(vec![looped], None);
        particles.step(&worlds(), Mat4::IDENTITY, 0.8);
        assert_eq!(particles.alive(0), 0);
        particles.step(&worlds(), Mat4::IDENTITY, 0.3);
        assert_eq!(particles.alive(0), 5, "the step past the wrap gives the burst at 0.9");
    }

    #[test]
    fn a_followed_parent_slows_by_its_speed_limit_as_the_shader_has_it() {
        let parent = system(json!({"bursts": burst(1.0), "world": true, "speed": {"min": [1], "max": [1], "scale": [10, 10]}, "limit": {"speed": {"min": [1], "max": [1], "scale": [1, 1]}, "dampen": 0.5}, "subEmitters": [{"system": 1, "on": "birth"}]}));
        let mut particles = running(vec![parent, system(json!({}))], None);
        particles.step(&worlds(), Mat4::IDENTITY, 0.01);
        let free = 10.0 * 2.0;
        let held = particles.position(0, 0, 2.0).distance(particles.position(0, 0, 0.0));
        let damping = -(0.5f32).ln() * 30.0;
        let expected = 2.0 + 9.0 * (1.0 - (-damping * 2.0).exp()) / damping;
        assert!((held - expected).abs() < 1e-3 && held < free / 2.0, "{held} against {expected}");
    }

    #[test]
    fn a_material_image_is_decoded_once_however_many_name_it() {
        let definition = parse(Some(&json!({"systems": [system(json!({"renderer": {"materials": [{"image": 3}, {"image": 1}, {"image": 3}]}}))]})), 2, 4, |at| Ok(Image { width: at as u32, height: 1, rgba: vec![0; 4 * at] })).unwrap();
        let images: Vec<_> = definition.systems[0].renderer.materials.iter().map(|m| m.image).collect();
        assert_eq!(images, [Some(0), Some(1), Some(0)]);
        assert_eq!(definition.images.iter().map(|image| image.width).collect::<Vec<_>>(), [3, 1]);
    }

    #[test]
    fn a_burst_emits_its_count_once_and_rate_emits_over_time() {
        let mut particles = running(vec![system(json!({"bursts": burst(7.0)})), system(json!({"rate": {"min": [1], "max": [1], "scale": [20, 20]}, "duration": 10}))], None);
        for _ in 0..10 {
            particles.step(&worlds(), Mat4::IDENTITY, 0.1);
        }
        assert_eq!(particles.alive(0), 7);
        assert!((19..=20).contains(&particles.alive(1)), "20 a second for a second: {}", particles.alive(1));
    }

    #[test]
    fn distance_emits_per_metre_the_emitter_moves() {
        let mut particles = running(vec![system(json!({"distance": {"min": [1], "max": [1], "scale": [10, 10]}}))], None);
        for step in 0..=20 {
            let body = Mat4::from_translation(Vec3::new(step as f32 * 0.05, 0.0, 0.0));
            particles.step(&worlds(), body, 0.01);
        }
        assert!((9..=10).contains(&particles.alive(0)), "a metre at ten a metre: {}", particles.alive(0));
        let mut still = running(vec![system(json!({"distance": {"min": [1], "max": [1], "scale": [10, 10]}}))], None);
        for _ in 0..20 {
            still.step(&worlds(), Mat4::IDENTITY, 0.01);
        }
        assert_eq!(still.alive(0), 0);
    }

    #[test]
    fn birth_sub_emitters_follow_each_parent_particle() {
        let parent = system(json!({"bursts": burst(3.0), "subEmitters": [{"system": 1, "on": "birth"}], "world": true, "speed": {"min": [1], "max": [1], "scale": [1, 1]}}));
        let child = system(json!({"loop": false, "duration": 1, "bursts": burst(2.0), "world": true}));
        let mut particles = running(vec![parent, child], None);
        particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        assert_eq!(particles.alive(0), 3);
        assert_eq!(particles.alive(1), 6, "each of three parents bursts two");
        let mut placed = Vec::new();
        particles.written(|system, _, instance| if system == 1 { placed.push(Vec4::from(instance.start).truncate()) });
        assert!(placed.iter().all(|at| (at.y - 1.0).abs() < 1e-4 && (at.z - 0.05).abs() < 1e-3), "children are born where their parents went, a twentieth of a second along +Z: {placed:?}");
    }

    #[test]
    fn death_sub_emitters_burst_where_the_parent_died() {
        let parent = system(json!({"bursts": burst(2.0), "subEmitters": [{"system": 1, "on": "death"}], "lifetime": {"min": [1], "max": [1], "scale": [0.2, 0.2]}, "speed": {"min": [1], "max": [1], "scale": [0, 0]}}));
        let child = system(json!({"bursts": burst(3.0)}));
        let mut particles = running(vec![parent, child], None);
        for _ in 0..3 {
            particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        }
        assert_eq!(particles.alive(1), 0, "nothing until the parents die");
        for _ in 0..3 {
            particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        }
        assert_eq!((particles.alive(0), particles.alive(1)), (0, 6));
    }

    fn menu(particles: Value, targets: Value) -> Expressions {
        Expressions::parse(&json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": targets, "particles": particles})).unwrap()
    }

    #[test]
    fn switching_a_system_off_clears_it_and_on_plays_it_again() {
        let menu = menu(json!([{"system": 0, "when": [[0, 1]]}]), json!([{"effect": "switch", "default": 1}]));
        let mut particles = running(vec![system(json!({"bursts": burst(4.0)}))], Some(&menu));
        particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        assert_eq!(particles.alive(0), 4);
        particles.written(|_, _, _| {});
        particles.apply(&[0.0]);
        particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        assert_eq!(particles.alive(0), 0);
        let mut cleared = 0;
        particles.written(|_, _, instance| { assert_eq!(instance.start[3], UNBORN); cleared += 1 });
        assert_eq!(cleared, 4, "the cleared slots reach the GPU");
        assert!(!particles.shown(0));
        particles.apply(&[1.0]);
        particles.step(&worlds(), Mat4::IDENTITY, 0.05);
        assert_eq!(particles.alive(0), 4, "it plays from its start each time it turns on");
    }

    #[test]
    fn a_particle_effect_sets_its_field() {
        let menu = menu(json!([]), json!([
            {"effect": {"particle": {"system": 0, "field": "rate"}}, "default": 0},
            {"effect": {"particle": {"system": 0, "field": "material"}}, "default": 0},
            {"effect": {"particle": {"system": 0, "field": "emission", "channel": 2}}, "default": 0},
            {"effect": {"particle": {"system": 0, "field": "color", "end": "max", "channel": 1}}, "default": 1},
            {"effect": {"particle": {"system": 7, "field": "rate"}}, "default": 0}
        ]));
        let mut particles = running(vec![system(json!({"duration": 10, "renderer": {"draw": true, "materials": [{"blend": "add"}, {"blend": "alpha"}]}}))], Some(&menu));
        for _ in 0..10 {
            particles.step(&worlds(), Mat4::IDENTITY, 0.1);
        }
        assert_eq!(particles.alive(0), 0, "the file's rate of zero");
        particles.apply(&[30.0, 1.2, 0.5, 0.0, 9.0]);
        for _ in 0..10 {
            particles.step(&worlds(), Mat4::IDENTITY, 0.1);
        }
        assert!((29..=30).contains(&particles.alive(0)), "{}", particles.alive(0));
        assert_eq!((particles.material(0).blend, particles.material(0).emission.z), (Blend::Alpha, srgb_to_linear(0.5)), "an sRGB value made linear");
        let d = &particles.systems[0].definition;
        assert_eq!((d.color.min[0].y, d.color.max[0].y), (1.0, 0.0), "only the end it names");
    }

    #[test]
    fn life_curves_reach_the_shader_as_rows_with_integrals() {
        let definition = read(json!({"systems": [system(json!({
            "velocity": {"x": {"min": [1], "max": [1], "scale": [2, 4]}},
            "colorOverLife": {"min": [[1, 1, 1, 1], [1, 1, 1, 0]], "max": [[1, 1, 1, 1], [1, 1, 1, 0]]}
        }))]})).unwrap();
        let rows = definition.systems[0].curves();
        let at = |row: usize, x: usize| rows[row * SAMPLES + x];
        assert_eq!((at(ROW_VELOCITY, 5)[0], at(ROW_VELOCITY + 1, 5)[0]), (2.0, 4.0));
        assert!((at(ROW_PATH, SAMPLES - 1)[0] - 2.0).abs() < 1e-4 && (at(ROW_PATH + 1, SAMPLES - 1)[0] - 4.0).abs() < 1e-4, "a constant velocity's path over a whole life");
        assert_eq!((at(ROW_COLOUR, 0)[3], at(ROW_COLOUR, SAMPLES - 1)[3]), (1.0, 0.0));
        assert_eq!(definition.systems[0].flags(), VELOCITY | COLOUR_OVER_LIFE);
    }
}
