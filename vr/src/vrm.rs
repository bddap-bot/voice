use std::collections::HashMap;

use glam::{Mat4, Quat, Vec2, Vec3};
use serde_json::Value;

const STANDING: &str = include_str!("../../docs/poses/standing.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Zero,
    One,
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Alpha {
    Opaque,
    Cutout(f32),
    Blend,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutlineWidth {
    World,
    Screen,
}

#[derive(Clone, Debug)]
pub struct Outline {
    pub width: OutlineWidth,
    pub factor: f32,
    pub width_image: Option<usize>,
    pub color: [f32; 3],
    pub lighting_mix: f32,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub base: [f32; 4],
    pub base_image: Option<usize>,
    pub shade: [f32; 3],
    pub shade_image: Option<usize>,
    pub alpha: Alpha,
    pub depth_write: bool,
    pub double_sided: bool,
    pub shading_shift: f32,
    pub shading_toony: f32,
    pub equalization: f32,
    pub queue: i32,
    pub outline: Option<Outline>,
}

struct Node {
    parent: Option<usize>,
    translation: Vec3,
    rotation: Quat,
    scale: Vec3,
    mesh: Option<usize>,
    skin: Option<usize>,
}

struct Skin {
    joints: Vec<usize>,
    inverse_bind: Vec<Mat4>,
}

struct Target {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
}

struct Primitive {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<Vec2>,
    joints: Vec<[usize; 4]>,
    weights: Vec<[f32; 4]>,
    indices: Vec<u32>,
    material: Option<usize>,
    targets: Vec<Target>,
}

struct Mesh {
    primitives: Vec<Primitive>,
    weights: Vec<f32>,
}

struct Expression {
    binds: Vec<(usize, usize, f32)>,
    binary: bool,
}

/// How far a gaze angle (degrees) turns an eye: its bone's degrees, or its expression's weight.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RangeMap {
    input: f32,
    output: f32,
}

impl RangeMap {
    fn map(&self, degrees: f32) -> f32 {
        self.output * (degrees / self.input).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug)]
struct LookAt {
    /// Whether the figure has eyes the settings can move.
    moves: bool,
    expression: bool,
    offset: Vec3,
    inner: RangeMap,
    outer: RangeMap,
    down: RangeMap,
    up: RangeMap,
}

pub struct Model {
    pub version: Version,
    nodes: Vec<Node>,
    meshes: Vec<Mesh>,
    skins: Vec<Skin>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    humanoid: HashMap<String, usize>,
    expressions: HashMap<String, Expression>,
    look_at: LookAt,
    rest_turns: Vec<Quat>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Draw {
    pub first: u32,
    pub count: u32,
    pub material: usize,
}

struct Delta {
    slot: u32,
    position: Vec3,
    normal: Vec3,
}

struct Morph {
    mesh: usize,
    target: usize,
    deltas: Vec<Delta>,
}

pub struct Skinned {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<Draw>,
    joints: Vec<(usize, Mat4)>,
    morphs: Vec<Morph>,
    applied: Vec<f32>,
    touched: Vec<u32>,
    base: Vec<(Vec3, Vec3)>,
    offsets: Vec<u32>,
    entries: Vec<(u32, u32)>,
    stamps: Vec<u32>,
    stamp: u32,
}

#[derive(Clone)]
pub struct Pose {
    rotations: Vec<Quat>,
    translations: Vec<Vec3>,
    pub weights: Vec<Vec<f32>>,
}

#[derive(Clone, Default)]
pub struct Humanoid {
    pub rotations: HashMap<String, Quat>,
    pub hips: Option<Vec3>,
}

struct Document<'a> {
    json: Value,
    binary: &'a [u8],
}

fn chunks(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).ok_or("truncated glTF binary");
    if bytes.get(..4) != Some(b"glTF") || word(4)? != 2 || word(8)? != bytes.len() {
        return Err("not a binary glTF 2 file".into());
    }
    let json_length = word(12)?;
    if bytes.get(16..20) != Some(b"JSON") {
        return Err("glTF binary has no JSON chunk".into());
    }
    let json = serde_json::from_slice(bytes.get(20..20 + json_length).ok_or("truncated JSON chunk")?).map_err(|error| error.to_string())?;
    let at = 20 + json_length;
    if at == bytes.len() {
        return Ok((json, &[]));
    }
    let binary_length = word(at)?;
    if bytes.get(at + 4..at + 8) != Some(b"BIN\0") {
        return Err("second glTF chunk is not BIN".into());
    }
    Ok((json, bytes.get(at + 8..at + 8 + binary_length).ok_or("truncated BIN chunk")?))
}

fn number(value: &Value, fallback: f32) -> f32 {
    value.as_f64().map_or(fallback, |value| value as f32)
}

fn floats<const N: usize>(value: &Value, fallback: [f32; N]) -> [f32; N] {
    let mut out = fallback;
    if let Some(array) = value.as_array() {
        for (slot, item) in out.iter_mut().zip(array) {
            *slot = number(item, *slot);
        }
    }
    out
}

fn index(value: &Value) -> Option<usize> {
    value.as_u64().map(|value| value as usize)
}

fn expression_one(name: &str) -> &str {
    match name {
        "a" => "aa",
        "i" => "ih",
        "u" => "ou",
        "e" => "ee",
        "o" => "oh",
        "joy" => "happy",
        "sorrow" => "sad",
        "fun" => "relaxed",
        "blink_l" => "blinkLeft",
        "blink_r" => "blinkRight",
        "lookleft" => "lookLeft",
        "lookright" => "lookRight",
        "lookup" => "lookUp",
        "lookdown" => "lookDown",
        name => name,
    }
}

/// Read as three-vrm reads it, raising a tiny input to its minimum.
fn range_map(input: Option<&Value>, output: Option<&Value>, fallback: f32) -> RangeMap {
    RangeMap { input: input.map_or(90.0, |value| number(value, 90.0)).max(0.01), output: output.map_or(fallback, |value| number(value, fallback)) }
}

fn look_at(version: Version, extensions: &Value) -> LookAt {
    match version {
        Version::One => {
            let look = &extensions["VRMC_vrm"]["lookAt"];
            let expression = look["type"].as_str() == Some("expression");
            let map = |key: &str| range_map(look[key].get("inputMaxValue"), look[key].get("outputScale"), if expression { 1.0 } else { 10.0 });
            LookAt { moves: look.is_object(), expression, offset: Vec3::from(floats(&look["offsetFromHeadBone"], [0.0, 0.06, 0.0])), inner: map("rangeMapHorizontalInner"), outer: map("rangeMapHorizontalOuter"), down: map("rangeMapVerticalDown"), up: map("rangeMapVerticalUp") }
        }
        Version::Zero => {
            let look = &extensions["VRM"]["firstPerson"];
            let expression = look["lookAtTypeName"].as_str() == Some("BlendShape");
            let map = |key: &str| range_map(look[key].get("xRange"), look[key].get("yRange"), if expression { 1.0 } else { 10.0 });
            let offset = &look["firstPersonBoneOffset"];
            let offset = if offset.is_object() { Vec3::new(number(&offset["x"], 0.0), number(&offset["y"], 0.06), -number(&offset["z"], 0.0)) } else { Vec3::new(0.0, 0.06, 0.0) };
            LookAt { moves: look.is_object(), expression, offset, inner: map("lookAtHorizontalInner"), outer: map("lookAtHorizontalOuter"), down: map("lookAtVerticalDown"), up: map("lookAtVerticalUp") }
        }
    }
}

fn rotation(world: &Mat4) -> Quat {
    world.to_scale_rotation_translation().1
}

fn bone_one(name: &str) -> String {
    name.replace("ThumbProximal", "ThumbMetacarpal").replace("ThumbIntermediate", "ThumbProximal")
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

impl Document<'_> {
    fn component_size(kind: u64) -> Result<usize, String> {
        match kind {
            5120 | 5121 => Ok(1),
            5122 | 5123 => Ok(2),
            5125 | 5126 => Ok(4),
            other => Err(format!("accessor component type {other}")),
        }
    }

    fn read_component(bytes: &[u8], kind: u64) -> f64 {
        match kind {
            5120 => bytes[0] as i8 as f64,
            5121 => bytes[0] as f64,
            5122 => i16::from_le_bytes([bytes[0], bytes[1]]) as f64,
            5123 => u16::from_le_bytes([bytes[0], bytes[1]]) as f64,
            5125 => u32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
            _ => f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
        }
    }

    fn view(&self, view: usize) -> Result<(&[u8], Option<usize>), String> {
        let view = &self.json["bufferViews"][view];
        if index(&view["buffer"]) != Some(0) || self.json["buffers"][0].get("uri").is_some() {
            return Err("only the embedded binary buffer is supported".into());
        }
        let offset = index(&view["byteOffset"]).unwrap_or(0);
        let length = index(&view["byteLength"]).ok_or("bufferView without byteLength")?;
        Ok((self.binary.get(offset..offset + length).ok_or("bufferView outside the buffer")?, index(&view["byteStride"])))
    }

    fn elements(&self, view: usize, offset: usize, kind: u64, components: usize, count: usize) -> Result<Vec<f64>, String> {
        let size = Self::component_size(kind)?;
        let (bytes, stride) = self.view(view)?;
        let stride = stride.unwrap_or(size * components);
        let mut out = Vec::with_capacity(count * components);
        for element in 0..count {
            for component in 0..components {
                let at = offset + element * stride + component * size;
                out.push(Self::read_component(bytes.get(at..at + size).ok_or("accessor outside its bufferView")?, kind));
            }
        }
        Ok(out)
    }

    fn accessor(&self, accessor: usize) -> Result<(Vec<f64>, usize), String> {
        let accessor = &self.json["accessors"][accessor];
        let components = match accessor["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") | Some("MAT2") => 4,
            Some("MAT3") => 9,
            Some("MAT4") => 16,
            other => return Err(format!("accessor type {other:?}")),
        };
        let kind = accessor["componentType"].as_u64().ok_or("accessor without componentType")?;
        let count = index(&accessor["count"]).ok_or("accessor without count")?;
        let mut values = match index(&accessor["bufferView"]) {
            Some(view) => self.elements(view, index(&accessor["byteOffset"]).unwrap_or(0), kind, components, count)?,
            None => vec![0.0; count * components],
        };
        if let Some(sparse) = accessor.get("sparse") {
            let changed = index(&sparse["count"]).ok_or("sparse without count")?;
            let indices = &sparse["indices"];
            let at = self.elements(index(&indices["bufferView"]).ok_or("sparse indices without bufferView")?, index(&indices["byteOffset"]).unwrap_or(0), indices["componentType"].as_u64().unwrap_or(5125), 1, changed)?;
            let replaced = self.elements(index(&sparse["values"]["bufferView"]).ok_or("sparse values without bufferView")?, index(&sparse["values"]["byteOffset"]).unwrap_or(0), kind, components, changed)?;
            for (slot, value) in at.iter().zip(replaced.chunks_exact(components)) {
                let slot = *slot as usize;
                values.get_mut(slot * components..(slot + 1) * components).ok_or("sparse index outside the accessor")?.copy_from_slice(value);
            }
        }
        if accessor["normalized"].as_bool() == Some(true) {
            let scale = match kind {
                5120 => 127.0,
                5121 => 255.0,
                5122 => 32767.0,
                5123 => 65535.0,
                _ => 1.0,
            };
            values.iter_mut().for_each(|value| *value = (*value / scale).max(-1.0));
        }
        Ok((values, components))
    }

    fn vec3s(&self, accessor: usize) -> Result<Vec<Vec3>, String> {
        let (values, components) = self.accessor(accessor)?;
        if components != 3 {
            return Err("expected a VEC3 accessor".into());
        }
        Ok(values.chunks_exact(3).map(|v| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)).collect())
    }

    fn image(&self, image: usize) -> Result<Image, String> {
        let entry = &self.json["images"][image];
        if entry["mimeType"].as_str() != Some("image/webp") {
            return Err(format!("image {image} is {}, but rendered textures are packed as lossless WebP", entry["mimeType"]));
        }
        let (bytes, _) = self.view(index(&entry["bufferView"]).ok_or("image without bufferView")?)?;
        let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).map_err(|error| format!("image {image}: {error}"))?;
        let (width, height) = decoder.dimensions();
        let mut pixels = vec![0; decoder.output_buffer_size().ok_or("image too large")?];
        decoder.read_image(&mut pixels).map_err(|error| format!("image {image}: {error}"))?;
        let rgba = if decoder.has_alpha() { pixels } else { pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect() };
        Ok(Image { width, height, rgba })
    }

    fn texture_image(&self, texture: &Value) -> Option<usize> {
        let texture = &self.json["textures"][index(texture)?];
        index(&texture["extensions"]["EXT_texture_webp"]["source"]).or_else(|| index(&texture["source"]))
    }
}

fn material_zero(document: &Document, material: &Value, properties: &Value) -> Material {
    if properties["shader"].as_str() != Some("VRM/MToon") {
        return material_one(document, material);
    }
    let float = |name: &str, fallback| number(&properties["floatProperties"][name], fallback);
    let color = |name: &str| {
        let fallback = if name == "_OutlineColor" { [0.0, 0.0, 0.0, 1.0] } else { [1.0; 4] };
        let [r, g, b, a] = floats(&properties["vectorProperties"][name], fallback);
        [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
    };
    let texture = |name: &str| document.texture_image(&properties["textureProperties"][name]);
    let shade_shift = float("_ShadeShift", 0.0);
    let shading_toony = shade_shift.mul_add(0.5, 0.5) * (1.0 - float("_ShadeToony", 0.9)) + float("_ShadeToony", 0.9);
    let keyword = |name: &str| properties["keywordMap"][name].as_bool() == Some(true);
    let alpha = if keyword("_ALPHABLEND_ON") {
        Alpha::Blend
    } else if keyword("_ALPHATEST_ON") {
        Alpha::Cutout(float("_Cutoff", 0.5))
    } else {
        Alpha::Opaque
    };
    let [r, g, b, _] = color("_ShadeColor");
    let width = match float("_OutlineWidthMode", 0.0) as i32 {
        1 => Some(OutlineWidth::World),
        2 => Some(OutlineWidth::Screen),
        _ => None,
    };
    let [outline_r, outline_g, outline_b, _] = color("_OutlineColor");
    Material {
        base: color("_Color"),
        base_image: texture("_MainTex"),
        shade: [r, g, b],
        shade_image: texture("_ShadeTexture"),
        alpha,
        depth_write: alpha != Alpha::Blend || float("_ZWrite", 0.0) == 1.0,
        double_sided: float("_CullMode", 2.0) as i32 == 0,
        shading_shift: -shade_shift - (1.0 - shading_toony),
        shading_toony,
        equalization: 1.0 - float("_IndirectLightIntensity", 0.1),
        queue: properties["renderQueue"].as_i64().map_or(2000, |queue| queue as i32),
        outline: width.map(|width| Outline {
            width,
            factor: 0.01 * float("_OutlineWidth", 0.0),
            width_image: texture("_OutlineWidthTexture"),
            color: [outline_r, outline_g, outline_b],
            lighting_mix: if float("_OutlineColorMode", 0.0) as i32 == 1 { float("_OutlineLightingMix", 1.0) } else { 0.0 },
        }),
    }
}

fn material_one(document: &Document, material: &Value) -> Material {
    let pbr = &material["pbrMetallicRoughness"];
    let base = floats(&pbr["baseColorFactor"], [1.0; 4]);
    let base_image = document.texture_image(&pbr["baseColorTexture"]["index"]);
    let alpha = match material["alphaMode"].as_str() {
        Some("MASK") => Alpha::Cutout(number(&material["alphaCutoff"], 0.5)),
        Some("BLEND") => Alpha::Blend,
        _ => Alpha::Opaque,
    };
    let toon = &material["extensions"]["VRMC_materials_mtoon"];
    let lit = toon.is_null();
    let depth_write = alpha != Alpha::Blend || toon["transparentWithZWrite"].as_bool() == Some(true);
    let queue = match alpha {
        Alpha::Opaque => 2000,
        Alpha::Cutout(_) => 2450,
        Alpha::Blend => (if depth_write { 2501 } else { 3000 }) + toon["renderQueueOffsetNumber"].as_i64().unwrap_or(0) as i32,
    };
    let width = match toon["outlineWidthMode"].as_str() {
        Some("worldCoordinates") => Some(OutlineWidth::World),
        Some("screenCoordinates") => Some(OutlineWidth::Screen),
        _ => None,
    };
    Material {
        base,
        base_image,
        shade: if lit { [base[0], base[1], base[2]] } else { floats(&toon["shadeColorFactor"], [0.0; 3]) },
        shade_image: if lit { base_image } else { document.texture_image(&toon["shadeMultiplyTexture"]["index"]) },
        alpha,
        depth_write,
        double_sided: material["doubleSided"].as_bool() == Some(true),
        shading_shift: number(&toon["shadingShiftFactor"], 0.0),
        shading_toony: number(&toon["shadingToonyFactor"], 0.9),
        equalization: number(&toon["giEqualizationFactor"], 0.9),
        queue,
        outline: width.map(|width| Outline {
            width,
            factor: number(&toon["outlineWidthFactor"], 0.0),
            width_image: document.texture_image(&toon["outlineWidthMultiplyTexture"]["index"]),
            color: floats(&toon["outlineColorFactor"], [0.0; 3]),
            lighting_mix: number(&toon["outlineLightingMixFactor"], 1.0),
        }),
    }
}

impl Model {
    pub fn parse(bytes: &[u8]) -> Result<Model, String> {
        let (json, binary) = chunks(bytes)?;
        let document = Document { json, binary };
        let json = &document.json;
        let extensions = &json["extensions"];
        let (version, humanoid): (Version, HashMap<String, usize>) = if let Some(vrm) = extensions.get("VRMC_vrm") {
            let bones = vrm["humanoid"]["humanBones"].as_object().ok_or("VRMC_vrm without humanBones")?;
            (Version::One, bones.iter().filter_map(|(name, bone)| Some((name.clone(), index(&bone["node"])?))).collect())
        } else if let Some(vrm) = extensions.get("VRM") {
            let bones = vrm["humanoid"]["humanBones"].as_array().ok_or("VRM without humanBones")?;
            (Version::Zero, bones.iter().filter_map(|bone| Some((bone_one(bone["bone"].as_str()?), index(&bone["node"])?))).collect())
        } else {
            return Err("file is not a VRM puppet".into());
        };

        let empty = Vec::new();
        let list = |key: &str| json[key].as_array().unwrap_or(&empty);
        let mut nodes: Vec<Node> = list("nodes")
            .iter()
            .map(|node| {
                let (scale, rotation, translation) = match node.get("matrix") {
                    Some(matrix) => Mat4::from_cols_array(&floats(matrix, Mat4::IDENTITY.to_cols_array())).to_scale_rotation_translation(),
                    None => (Vec3::from(floats(&node["scale"], [1.0; 3])), Quat::from_array(floats(&node["rotation"], [0.0, 0.0, 0.0, 1.0])), Vec3::from(floats(&node["translation"], [0.0; 3]))),
                };
                Node { parent: None, translation, rotation, scale, mesh: index(&node["mesh"]), skin: index(&node["skin"]) }
            })
            .collect();
        for (parent, node) in list("nodes").iter().enumerate() {
            for child in node["children"].as_array().unwrap_or(&empty).iter().filter_map(index) {
                let node = nodes.get_mut(child).ok_or("child node out of range")?;
                if node.parent.replace(parent).is_some() {
                    return Err("a node has two parents".into());
                }
            }
        }

        let skins = list("skins")
            .iter()
            .map(|skin| {
                let joints: Vec<usize> = skin["joints"].as_array().ok_or("skin without joints")?.iter().filter_map(index).collect();
                let inverse_bind = match index(&skin["inverseBindMatrices"]) {
                    Some(accessor) => document.accessor(accessor)?.0.chunks_exact(16).map(|m| Mat4::from_cols_array(&std::array::from_fn(|i| m[i] as f32))).collect(),
                    None => vec![Mat4::IDENTITY; joints.len()],
                };
                Ok(Skin { joints, inverse_bind })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let meshes = list("meshes")
            .iter()
            .map(|mesh| {
                let primitives = mesh["primitives"]
                    .as_array()
                    .ok_or("mesh without primitives")?
                    .iter()
                    .map(|primitive| {
                        if primitive["mode"].as_u64().is_some_and(|mode| mode != 4) {
                            return Err("only triangle lists are supported".to_owned());
                        }
                        let attributes = &primitive["attributes"];
                        let positions = document.vec3s(index(&attributes["POSITION"]).ok_or("primitive without POSITION")?)?;
                        let count = positions.len();
                        let normals = match index(&attributes["NORMAL"]) {
                            Some(accessor) => document.vec3s(accessor)?,
                            None => vec![Vec3::Z; count],
                        };
                        let uvs = match index(&attributes["TEXCOORD_0"]) {
                            Some(accessor) => document.accessor(accessor)?.0.chunks_exact(2).map(|v| Vec2::new(v[0] as f32, v[1] as f32)).collect(),
                            None => vec![Vec2::ZERO; count],
                        };
                        let joints = match index(&attributes["JOINTS_0"]) {
                            Some(accessor) => document.accessor(accessor)?.0.chunks_exact(4).map(|v| std::array::from_fn(|i| v[i] as usize)).collect(),
                            None => Vec::new(),
                        };
                        let weights = match index(&attributes["WEIGHTS_0"]) {
                            Some(accessor) => document.accessor(accessor)?.0.chunks_exact(4).map(|v| std::array::from_fn(|i| v[i] as f32)).collect(),
                            None => Vec::new(),
                        };
                        let indices: Vec<u32> = match index(&primitive["indices"]) {
                            Some(accessor) => document.accessor(accessor)?.0.into_iter().map(|value| value as u32).collect(),
                            None => (0..count as u32).collect(),
                        };
                        if indices.iter().any(|&vertex| vertex as usize >= count) || [normals.len(), uvs.len()].iter().any(|&length| length != count) {
                            return Err("primitive attributes disagree in length".to_owned());
                        }
                        let targets = primitive["targets"]
                            .as_array()
                            .unwrap_or(&empty)
                            .iter()
                            .map(|target| {
                                let positions = match index(&target["POSITION"]) {
                                    Some(accessor) => document.vec3s(accessor)?,
                                    None => vec![Vec3::ZERO; count],
                                };
                                let normals = match index(&target["NORMAL"]) {
                                    Some(accessor) => document.vec3s(accessor)?,
                                    None => vec![Vec3::ZERO; count],
                                };
                                Ok(Target { positions, normals })
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        Ok(Primitive { positions, normals, uvs, joints, weights, indices, material: index(&primitive["material"]), targets })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let targets = primitives.iter().map(|primitive| primitive.targets.len()).max().unwrap_or(0);
                let mut weights: Vec<f32> = mesh["weights"].as_array().map(|weights| weights.iter().map(|weight| number(weight, 0.0)).collect()).unwrap_or_default();
                weights.resize(targets, 0.0);
                Ok(Mesh { primitives, weights })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let properties = &extensions["VRM"]["materialProperties"];
        let mut materials: Vec<Material> = list("materials")
            .iter()
            .enumerate()
            .map(|(at, material)| match version {
                Version::Zero => material_zero(&document, material, &properties[at]),
                Version::One => material_one(&document, material),
            })
            .collect();
        let fallback = materials.len();
        materials.push(material_one(&document, &Value::Null));
        for primitive in meshes.iter().flat_map(|mesh| &mesh.primitives) {
            if primitive.material.is_some_and(|material| material >= fallback) {
                return Err("primitive material out of range".into());
            }
        }
        for skin in &skins {
            if skin.inverse_bind.len() != skin.joints.len() || skin.joints.iter().any(|&joint| joint >= nodes.len()) {
                return Err("skin joints disagree with their nodes or bind matrices".into());
            }
        }
        for primitive in meshes.iter().flat_map(|mesh| &mesh.primitives) {
            let count = primitive.positions.len();
            if primitive.targets.iter().any(|target| target.positions.len() != count || target.normals.len() != count) || [primitive.joints.len(), primitive.weights.len()].iter().any(|&length| length != 0 && length != count) {
                return Err("primitive attributes disagree in length".into());
            }
        }
        for node in &nodes {
            let (Some(mesh), Some(skin)) = (node.mesh, node.skin) else { continue };
            let joints = skins.get(skin).ok_or("node skin out of range")?.joints.len();
            if meshes.get(mesh).ok_or("node mesh out of range")?.primitives.iter().any(|primitive| primitive.joints.iter().flatten().any(|&slot| slot >= joints)) {
                return Err("vertex joint outside its skin".into());
            }
        }
        for start in 0..nodes.len() {
            let mut node = start;
            for _ in 0..=nodes.len() {
                match nodes[node].parent {
                    Some(parent) => node = parent,
                    None => break,
                }
            }
            if nodes[node].parent.is_some() {
                return Err("the node hierarchy has a cycle".into());
            }
        }
        if nodes.iter().any(|node| node.mesh.is_some_and(|mesh| mesh >= meshes.len())) || humanoid.values().any(|&node| node >= nodes.len()) {
            return Err("a node or bone index is out of range".into());
        }
        if !nodes.iter().any(|node| node.mesh.is_some()) {
            return Err("the puppet has no mesh".into());
        }

        let mut used: Vec<usize> = materials.iter().flat_map(|material| [material.base_image, material.shade_image, material.outline.as_ref().and_then(|outline| outline.width_image)]).flatten().collect();
        used.sort_unstable();
        used.dedup();
        let mut decoded: HashMap<usize, usize> = HashMap::new();
        let mut images = Vec::new();
        for image in used {
            decoded.insert(image, images.len());
            images.push(document.image(image)?);
        }
        for material in &mut materials {
            material.base_image = material.base_image.map(|image| decoded[&image]);
            material.shade_image = material.shade_image.map(|image| decoded[&image]);
            if let Some(outline) = &mut material.outline {
                outline.width_image = outline.width_image.map(|image| decoded[&image]);
            }
        }
        let mut expressions = HashMap::new();
        match version {
            Version::One => {
                for (name, expression) in extensions["VRMC_vrm"]["expressions"]["preset"].as_object().into_iter().flatten() {
                    let binds = expression["morphTargetBinds"].as_array().unwrap_or(&empty).iter().filter_map(|bind| Some((nodes.get(index(&bind["node"])?)?.mesh?, index(&bind["index"])?, number(&bind["weight"], 0.0)))).collect();
                    expressions.insert(name.clone(), Expression { binds, binary: expression["isBinary"].as_bool() == Some(true) });
                }
            }
            Version::Zero => {
                for group in extensions["VRM"]["blendShapeMaster"]["blendShapeGroups"].as_array().into_iter().flatten() {
                    let Some(name) = group["presetName"].as_str().filter(|name| !name.is_empty() && *name != "unknown") else { continue };
                    let binds = group["binds"].as_array().unwrap_or(&empty).iter().filter_map(|bind| Some((index(&bind["mesh"])?, index(&bind["index"])?, number(&bind["weight"], 0.0) / 100.0))).collect();
                    expressions.insert(expression_one(name).to_owned(), Expression { binds, binary: group["isBinary"].as_bool() == Some(true) });
                }
            }
        }
        for expression in expressions.values_mut() {
            expression.binds.retain(|&(mesh, target, _)| meshes.get(mesh).is_some_and(|mesh| target < mesh.weights.len()));
        }
        let mut look_at = look_at(version, extensions);
        look_at.moves &= if look_at.expression {
            ["lookLeft", "lookRight", "lookUp", "lookDown"].iter().any(|name| expressions.get(*name).is_some_and(|expression| !expression.binds.is_empty()))
        } else {
            humanoid.contains_key("leftEye") || humanoid.contains_key("rightEye")
        };
        let mut model = Model { version, nodes, meshes, skins, materials, images, humanoid, expressions, look_at, rest_turns: Vec::new() };
        model.rest_turns = model.worlds(&model.rest()).iter().map(rotation).collect();
        Ok(model)
    }

    pub fn worlds(&self, pose: &Pose) -> Vec<Mat4> {
        let mut worlds: Vec<Option<Mat4>> = vec![None; self.nodes.len()];
        fn resolve(model: &Model, pose: &Pose, worlds: &mut [Option<Mat4>], node: usize) -> Mat4 {
            if let Some(world) = worlds[node] {
                return world;
            }
            let local = Mat4::from_scale_rotation_translation(model.nodes[node].scale, pose.rotations[node], pose.translations[node]);
            let world = model.nodes[node].parent.map_or(local, |parent| resolve(model, pose, worlds, parent) * local);
            worlds[node] = Some(world);
            world
        }
        (0..self.nodes.len()).map(|node| resolve(self, pose, &mut worlds, node)).collect()
    }

    pub fn rest(&self) -> Pose {
        Pose { rotations: self.nodes.iter().map(|node| node.rotation).collect(), translations: self.nodes.iter().map(|node| node.translation).collect(), weights: self.meshes.iter().map(|mesh| mesh.weights.clone()).collect() }
    }

    pub fn standing(&self) -> Humanoid {
        let preset: Value = serde_json::from_str(STANDING).expect("standing.json");
        let rotations = preset["data"]
            .as_object()
            .expect("standing.json data")
            .iter()
            .map(|(name, bone)| {
                let [x, y, z, w] = floats(&bone["rotation"], [0.0, 0.0, 0.0, 1.0]);
                let rotation = match self.version {
                    Version::Zero => Quat::from_xyzw(x, y, z, w),
                    Version::One => Quat::from_xyzw(-x, y, -z, w),
                };
                (bone_one(name), rotation)
            })
            .collect();
        Humanoid { rotations, hips: None }
    }

    pub fn rest_hips(&self) -> Option<Vec3> {
        let worlds = self.worlds(&self.rest());
        Some(worlds[*self.humanoid.get("hips")?].w_axis.truncate())
    }

    pub fn pose(&self, humanoid: &Humanoid) -> Pose {
        let mut pose = self.rest();
        let parent_rotation = |node: usize| self.nodes[node].parent.map_or(Quat::IDENTITY, |parent| self.rest_turns[parent]);
        for (name, normalized) in &humanoid.rotations {
            let Some(&node) = self.humanoid.get(name) else { continue };
            let parent = parent_rotation(node);
            pose.rotations[node] = (parent.inverse() * *normalized * parent * self.nodes[node].rotation).normalize();
        }
        if let (Some(position), Some(&hips)) = (humanoid.hips, self.humanoid.get("hips")) {
            let parent = self.nodes[hips].parent.map_or(Mat4::IDENTITY, |parent| self.worlds(&pose)[parent]);
            pose.translations[hips] = parent.inverse().transform_point3(position);
        }
        pose
    }

    pub fn express(&self, pose: &mut Pose, name: &str, weight: f32) {
        let Some(expression) = self.expressions.get(name) else { return };
        let weight = if expression.binary { if weight > 0.5 { 1.0 } else { 0.0 } } else { weight };
        for &(mesh, target, bind) in &expression.binds {
            pose.weights[mesh][target] += weight * bind;
        }
    }

    /// VRM 0 figures face -Z.
    pub fn facing(&self) -> Quat {
        match self.version {
            Version::Zero => Quat::from_rotation_y(std::f32::consts::PI),
            Version::One => Quat::IDENTITY,
        }
    }

    /// Where the eyes look from, and the head's frame (+Z ahead, +Y up, +X to the figure's left), in model space.
    pub fn face(&self, worlds: &[Mat4]) -> Option<(Vec3, Quat)> {
        let head = *self.humanoid.get("head")?;
        Some((worlds[head].transform_point3(self.look_at.offset), rotation(&worlds[head]) * self.rest_turns[head].inverse() * self.facing()))
    }

    /// The gaze angles, radians, at which the eyes stop following: sideways, up and down.
    pub fn eye_reach(&self) -> [f32; 3] {
        let look = &self.look_at;
        if !look.moves {
            return [0.0; 3];
        }
        let sideways = if look.expression { look.outer.input } else { look.inner.input.min(look.outer.input) };
        [sideways, look.up.input, look.down.input].map(f32::to_radians)
    }

    /// Turns the head by `turn` (a model-space rotation), shared between the neck and the head.
    pub fn turn_head(&self, pose: &mut Pose, worlds: &[Mat4], turn: Quat) {
        let bones: Vec<usize> = ["neck", "head"].iter().filter_map(|bone| self.humanoid.get(*bone).copied()).collect();
        let part = Quat::IDENTITY.slerp(turn, 1.0 / bones.len().max(1) as f32);
        for node in bones {
            let parent = self.nodes[node].parent.map_or(Quat::IDENTITY, |parent| rotation(&worlds[parent]));
            pose.rotations[node] = (parent.inverse() * part * parent * pose.rotations[node]).normalize();
        }
    }

    /// Turns the eyes `yaw` degrees toward the figure's left and `pitch` degrees up through the look-at range maps, the up map for an upward gaze as the maps are named (three-vrm's bone applier swaps them).
    pub fn turn_eyes(&self, pose: &mut Pose, yaw: f32, pitch: f32) {
        let look = &self.look_at;
        if look.expression {
            self.express(pose, "lookLeft", look.outer.map(yaw));
            self.express(pose, "lookRight", look.outer.map(-yaw));
            self.express(pose, "lookUp", look.up.map(pitch));
            self.express(pose, "lookDown", look.down.map(-pitch));
            return;
        }
        let vertical = if pitch >= 0.0 { -look.up.map(pitch) } else { look.down.map(-pitch) };
        for (bone, leftward, rightward) in [("leftEye", look.outer, look.inner), ("rightEye", look.inner, look.outer)] {
            let Some(&node) = self.humanoid.get(bone) else { continue };
            let horizontal = if yaw >= 0.0 { leftward.map(yaw) } else { -rightward.map(-yaw) };
            let turn = Quat::from_rotation_y(horizontal.to_radians()) * Quat::from_rotation_x(vertical.to_radians());
            let normalized = self.facing() * turn * self.facing().inverse();
            let parent = self.nodes[node].parent.map_or(Quat::IDENTITY, |parent| self.rest_turns[parent]);
            pose.rotations[node] = (parent.inverse() * normalized * parent * self.nodes[node].rotation).normalize();
        }
    }

    pub fn feet(&self, worlds: &[Mat4]) -> Option<[Vec3; 2]> {
        Some([*self.humanoid.get("leftFoot")?, *self.humanoid.get("rightFoot")?].map(|node| worlds[node].w_axis.truncate()))
    }

    pub fn skinned(&self) -> Result<Skinned, String> {
        let palette_index = |index: usize| u16::try_from(index).map_err(|_| "the model has 65536 or more joints".to_owned());
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut draws = Vec::new();
        let mut joints = Vec::new();
        let mut morphs = Vec::new();
        for (node, entry) in self.nodes.iter().enumerate() {
            let Some(mesh_index) = entry.mesh else { continue };
            let mesh = &self.meshes[mesh_index];
            let own = palette_index(joints.len())?;
            joints.push((node, Mat4::IDENTITY));
            let skin = match entry.skin {
                Some(skin) => {
                    let skin = &self.skins[skin];
                    let first = joints.len();
                    joints.extend(skin.joints.iter().copied().zip(skin.inverse_bind.iter().copied()));
                    palette_index(joints.len())?;
                    Some(first as u16)
                }
                None => None,
            };
            for primitive in &mesh.primitives {
                let base = vertices.len() as u32;
                for vertex in 0..primitive.positions.len() {
                    let (slots, weights) = match (skin, primitive.joints.get(vertex), primitive.weights.get(vertex)) {
                        (Some(first), Some(slots), Some(weights)) if weights.iter().any(|&weight| weight > 0.0) => (slots.map(|slot| first + slot as u16), *weights),
                        _ => ([own; 4], [1.0, 0.0, 0.0, 0.0]),
                    };
                    vertices.push(Vertex { position: primitive.positions[vertex].into(), normal: primitive.normals[vertex].into(), uv: primitive.uvs[vertex].into(), joints: slots, weights });
                }
                for (target, data) in primitive.targets.iter().enumerate() {
                    let deltas: Vec<Delta> = (0..primitive.positions.len()).filter(|&vertex| data.positions[vertex] != Vec3::ZERO || data.normals[vertex] != Vec3::ZERO).map(|vertex| Delta { slot: base + vertex as u32, position: data.positions[vertex], normal: data.normals[vertex] }).collect();
                    if !deltas.is_empty() {
                        morphs.push(Morph { mesh: mesh_index, target, deltas });
                    }
                }
                draws.push(Draw { first: indices.len() as u32, count: primitive.indices.len() as u32, material: primitive.material.unwrap_or(self.materials.len() - 1) });
                indices.extend(primitive.indices.iter().map(|&vertex| vertex + base));
            }
        }
        let queue = |draw: &Draw| self.materials[draw.material].queue;
        draws.sort_by_key(queue);
        let mut touched: Vec<u32> = morphs.iter().flat_map(|morph| morph.deltas.iter().map(|delta| delta.slot)).collect();
        touched.sort_unstable();
        touched.dedup();
        let mut counts = vec![0u32; touched.len() + 1];
        for morph in &mut morphs {
            for delta in &mut morph.deltas {
                delta.slot = touched.binary_search(&delta.slot).unwrap() as u32;
                counts[delta.slot as usize + 1] += 1;
            }
        }
        let offsets: Vec<u32> = counts.iter().scan(0, |sum, &count| { *sum += count; Some(*sum) }).collect();
        let mut entries = vec![(0, 0); *offsets.last().unwrap() as usize];
        let mut filled = offsets.clone();
        for (at, morph) in morphs.iter().enumerate() {
            for (index, delta) in morph.deltas.iter().enumerate() {
                entries[filled[delta.slot as usize] as usize] = (at as u32, index as u32);
                filled[delta.slot as usize] += 1;
            }
        }
        let base = touched.iter().map(|&vertex| (Vec3::from(vertices[vertex as usize].position), Vec3::from(vertices[vertex as usize].normal))).collect();
        let mut skinned = Skinned { vertices, indices, draws, joints, applied: vec![0.0; morphs.len()], stamps: vec![0; touched.len()], morphs, touched, base, offsets, entries, stamp: 0 };
        skinned.morph(&self.rest().weights);
        Ok(skinned)
    }
}

impl Skinned {
    pub fn palette(&self, worlds: &[Mat4], placement: Mat4) -> Vec<Mat4> {
        self.joints.iter().map(|&(node, inverse)| placement * worlds[node] * inverse).collect()
    }

    pub fn positions(&self, palette: &[Mat4]) -> Vec<Vec3> {
        self.vertices
            .iter()
            .map(|vertex| {
                let matrix = vertex.joints.iter().zip(vertex.weights).filter(|(_, weight)| *weight > 0.0).fold(Mat4::ZERO, |sum, (&joint, weight)| sum + palette[joint as usize] * weight);
                matrix.transform_point3(vertex.position.into())
            })
            .collect()
    }

    pub fn morph(&mut self, weights: &[Vec<f32>]) -> Vec<u32> {
        let current: Vec<f32> = self.morphs.iter().map(|morph| weights[morph.mesh][morph.target]).collect();
        self.stamp += 1;
        let mut dirty = Vec::new();
        for (morph, (now, applied)) in self.morphs.iter().zip(current.iter().zip(&self.applied)) {
            if now == applied {
                continue;
            }
            for delta in &morph.deltas {
                let slot = delta.slot as usize;
                if self.stamps[slot] != self.stamp {
                    self.stamps[slot] = self.stamp;
                    dirty.push(delta.slot);
                }
            }
        }
        let mut updated = Vec::with_capacity(dirty.len());
        for slot in dirty {
            let (mut position, mut normal) = self.base[slot as usize];
            for &(morph, delta) in &self.entries[self.offsets[slot as usize] as usize..self.offsets[slot as usize + 1] as usize] {
                let weight = current[morph as usize];
                if weight != 0.0 {
                    let delta = &self.morphs[morph as usize].deltas[delta as usize];
                    position += delta.position * weight;
                    normal += delta.normal * weight;
                }
            }
            let vertex = self.touched[slot as usize];
            let target = &mut self.vertices[vertex as usize];
            target.position = position.into();
            target.normal = normal.into();
            updated.push(vertex);
        }
        self.applied = current;
        updated
    }

    pub fn joints(&self) -> usize {
        self.joints.len()
    }
}

pub struct Fit {
    pub scale: f32,
    pub ground: f32,
    pub ankle: f32,
}

impl Fit {
    pub fn new(model: &Model, skinned: &Skinned, height: f32) -> Fit {
        let worlds = model.worlds(&model.rest());
        let points = skinned.positions(&skinned.palette(&worlds, Mat4::IDENTITY));
        let (low, high) = bounds(&points);
        let size = high.y - low.y;
        let feet = model.feet(&worlds).map_or(low.y, |feet| feet[0].y.min(feet[1].y));
        Fit { scale: if size > 0.0 { height / size } else { 1.0 }, ground: low.y, ankle: feet - low.y }
    }

    pub fn placement(&self, model: &Model, worlds: &[Mat4], floor: f32) -> Mat4 {
        let oriented = Mat4::from_quat(model.facing()) * Mat4::from_scale(Vec3::splat(self.scale));
        let shift = match model.feet(worlds) {
            Some(feet) => {
                let [left, right] = feet.map(|foot| oriented.transform_point3(foot));
                Vec3::new(-(left.x + right.x) / 2.0, floor + self.ankle * self.scale - left.y.min(right.y), -(left.z + right.z) / 2.0)
            }
            None => Vec3::new(0.0, floor - self.ground * self.scale, 0.0),
        };
        Mat4::from_translation(shift) * oriented
    }
}

pub fn bounds(points: &[Vec3]) -> (Vec3, Vec3) {
    points.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(low, high), &point| (low.min(point), high.max(point)))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub struct Builder {
        json: Value,
        binary: Vec<u8>,
    }

    impl Builder {
        pub fn new(extensions: Value) -> Builder {
            Builder { json: serde_json::json!({ "asset": { "version": "2.0" }, "extensions": extensions, "buffers": [{}], "bufferViews": [], "accessors": [] }), binary: Vec::new() }
        }

        pub fn view(&mut self, bytes: &[u8]) -> usize {
            while self.binary.len() % 4 != 0 {
                self.binary.push(0);
            }
            let views = self.json["bufferViews"].as_array_mut().unwrap();
            views.push(serde_json::json!({ "buffer": 0, "byteOffset": self.binary.len(), "byteLength": bytes.len() }));
            self.binary.extend_from_slice(bytes);
            views.len() - 1
        }

        pub fn accessor(&mut self, kind: &str, values: &[f32]) -> usize {
            let bytes: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
            let components = match kind { "SCALAR" => 1, "VEC2" => 2, "VEC3" => 3, "VEC4" => 4, _ => 16 };
            let view = self.view(&bytes);
            self.push_accessor(serde_json::json!({ "bufferView": view, "componentType": 5126, "count": values.len() / components, "type": kind }))
        }

        pub fn indices(&mut self, values: &[u16]) -> usize {
            let bytes: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
            let view = self.view(&bytes);
            self.push_accessor(serde_json::json!({ "bufferView": view, "componentType": 5123, "count": values.len(), "type": "SCALAR" }))
        }

        pub fn push_accessor(&mut self, accessor: Value) -> usize {
            let accessors = self.json["accessors"].as_array_mut().unwrap();
            accessors.push(accessor);
            accessors.len() - 1
        }

        pub fn set(&mut self, key: &str, value: Value) {
            self.json[key] = value;
        }

        pub fn webp(&mut self, width: u32, height: u32, rgba: &[u8]) -> usize {
            let mut encoded = Vec::new();
            image_webp::WebPEncoder::new(&mut encoded).encode(rgba, width, height, image_webp::ColorType::Rgba8).unwrap();
            let view = self.view(&encoded);
            let images = self.json.as_object_mut().unwrap().entry("images").or_insert(serde_json::json!([]));
            images.as_array_mut().unwrap().push(serde_json::json!({ "bufferView": view, "mimeType": "image/webp" }));
            images.as_array().unwrap().len() - 1
        }

        pub fn glb(mut self) -> Vec<u8> {
            self.json["buffers"][0]["byteLength"] = self.binary.len().into();
            let mut json = serde_json::to_vec(&self.json).unwrap();
            while json.len() % 4 != 0 {
                json.push(b' ');
            }
            while self.binary.len() % 4 != 0 {
                self.binary.push(0);
            }
            let total = 12 + 8 + json.len() + 8 + self.binary.len();
            let mut out = Vec::with_capacity(total);
            out.extend_from_slice(b"glTF");
            out.extend_from_slice(&2u32.to_le_bytes());
            out.extend_from_slice(&(total as u32).to_le_bytes());
            out.extend_from_slice(&(json.len() as u32).to_le_bytes());
            out.extend_from_slice(b"JSON");
            out.extend_from_slice(&json);
            out.extend_from_slice(&(self.binary.len() as u32).to_le_bytes());
            out.extend_from_slice(b"BIN\0");
            out.extend_from_slice(&self.binary);
            out
        }
    }

    pub fn checker() -> Vec<u8> {
        [[230, 40, 40, 255], [40, 200, 60, 255], [40, 60, 230, 255], [240, 240, 240, 200]].concat()
    }

    pub fn figure(version: Version) -> Vec<u8> {
        let mut builder = Builder::new(match version {
            Version::One => serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 }, "rightUpperArm": { "node": 2 } } } } }),
            Version::Zero => serde_json::json!({ "VRM": { "humanoid": { "humanBones": [{ "bone": "hips", "node": 1 }, { "bone": "rightUpperArm", "node": 2 }] }, "materialProperties": [{ "shader": "VRM/MToon", "keywordMap": { "_ALPHATEST_ON": true }, "floatProperties": { "_BlendMode": 1, "_Cutoff": 0.5 }, "vectorProperties": { "_Color": [1, 1, 1, 1], "_ShadeColor": [0.5, 0.5, 0.5, 1] }, "textureProperties": { "_MainTex": 0, "_ShadeTexture": 0 }, "renderQueue": 2450 }] } }),
        });
        let body = [-0.2f32, 0.0, 0.0, 0.2, 0.0, 0.0, 0.2, 1.0, 0.0, -0.2, 1.0, 0.0];
        let arm = [0.2f32, 0.9, 0.0, 0.6, 0.9, 0.0, 0.6, 1.0, 0.0, 0.2, 1.0, 0.0];
        let positions: Vec<f32> = body.iter().chain(&arm).copied().collect();
        let position = builder.accessor("VEC3", &positions);
        let facing = if version == Version::Zero { -1.0 } else { 1.0 };
        let normal = builder.accessor("VEC3", &[0.0, 0.0, facing].repeat(8));
        let uv = builder.accessor("VEC2", &[0.25, 0.75, 0.75, 0.75, 0.75, 0.25, 0.25, 0.25].repeat(2));
        let joints = builder.accessor("VEC4", &[[0.0, 0.0, 0.0, 0.0].repeat(4), [1.0, 0.0, 0.0, 0.0].repeat(4)].concat());
        let weights = builder.accessor("VEC4", &[1.0, 0.0, 0.0, 0.0].repeat(8));
        let mut triangles = [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        if version == Version::Zero {
            triangles.chunks_exact_mut(3).for_each(|triangle| triangle.swap(1, 2));
        }
        let indices = builder.indices(&triangles);
        let inverse = builder.accessor("MAT4", &[Mat4::IDENTITY.to_cols_array(), Mat4::from_translation(Vec3::new(-0.2, -0.95, 0.0)).to_cols_array()].concat());
        let image = builder.webp(2, 2, &checker());
        builder.set("nodes", serde_json::json!([
            { "children": [1], "mesh": 0, "skin": 0 },
            { "children": [2] },
            { "translation": [0.2, 0.95, 0.0] },
        ]));
        builder.set("skins", serde_json::json!([{ "joints": [1, 2], "inverseBindMatrices": inverse }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv, "JOINTS_0": joints, "WEIGHTS_0": weights }, "indices": indices, "material": 0 }] }]));
        builder.set("textures", serde_json::json!([{ "extensions": { "EXT_texture_webp": { "source": image } } }]));
        builder.set("materials", serde_json::json!([{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } }, "alphaMode": "MASK", "extensions": { "VRMC_materials_mtoon": { "shadeColorFactor": [0.5, 0.5, 0.5] } } }]));
        builder.glb()
    }

    #[test]
    fn a_packed_figure_loads_with_its_lossless_texture_exactly() {
        for version in [Version::Zero, Version::One] {
            let model = Model::parse(&figure(version)).unwrap();
            assert_eq!(model.version, version);
            assert_eq!(model.images.len(), 1);
            assert_eq!((model.images[0].width, model.images[0].height), (2, 2));
            assert_eq!(model.images[0].rgba, checker(), "lossless WebP decodes to the packed pixels");
            assert_eq!(model.materials[0].base_image, Some(0));
            assert!(matches!(model.materials[0].alpha, Alpha::Cutout(cutoff) if (cutoff - 0.5).abs() < 1e-6));
        }
    }

    #[test]
    fn a_hierarchy_with_a_cycle_or_a_stray_index_is_refused() {
        let parse = |nodes: Value, bones: Value| {
            let mut builder = Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": bones } } }));
            let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
            builder.set("nodes", nodes);
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
            Model::parse(&builder.glb()).err()
        };
        assert!(parse(serde_json::json!([{ "children": [2] }, { "children": [2] }, { "mesh": 0 }]), serde_json::json!({})).unwrap().contains("two parents"));
        assert!(parse(serde_json::json!([{ "mesh": 0, "children": [1] }, { "children": [0] }]), serde_json::json!({})).unwrap().contains("cycle"));
        assert!(parse(serde_json::json!([{ "mesh": 0 }, { "children": [2] }, { "children": [1] }]), serde_json::json!({})).unwrap().contains("cycle"));
        assert!(parse(serde_json::json!([{ "mesh": 0 }, { "mesh": 5 }]), serde_json::json!({})).unwrap().contains("out of range"));
        assert!(parse(serde_json::json!([{ "mesh": 0 }]), serde_json::json!({ "hips": { "node": 9 } })).unwrap().contains("out of range"));
        assert!(parse(serde_json::json!([{ "mesh": 0 }]), serde_json::json!({ "hips": { "node": 0 } })).is_none());
    }

    #[test]
    fn a_rendered_texture_that_is_not_webp_is_refused() {
        let mut bytes = figure(Version::One);
        let at = bytes.windows(10).position(|window| window == b"image/webp").unwrap();
        bytes[at..at + 10].copy_from_slice(b"image/jpeg");
        assert!(Model::parse(&bytes).err().unwrap().contains("lossless WebP"));
    }

    #[test]
    fn a_primitive_naming_a_missing_material_is_refused() {
        let mut bytes = figure(Version::One);
        let at = bytes.windows(12).position(|window| window == b"\"material\":0").unwrap();
        bytes[at + 11] = b'7';
        assert!(Model::parse(&bytes).err().unwrap().contains("material out of range"));
    }

    fn positions(model: &Model, pose: &Pose) -> Vec<Vec3> {
        let skinned = model.skinned().unwrap();
        skinned.positions(&skinned.palette(&model.worlds(pose), Mat4::IDENTITY))
    }

    #[test]
    fn the_rest_pose_skins_vertices_where_the_file_put_them() {
        let model = Model::parse(&figure(Version::One)).unwrap();
        let skinned = model.skinned().unwrap();
        assert_eq!((skinned.vertices.len(), skinned.indices.len(), skinned.joints()), (8, 12, 3));
        for (vertex, expected) in positions(&model, &model.rest()).iter().zip([[-0.2, 0.0], [0.2, 0.0], [0.2, 1.0], [-0.2, 1.0], [0.2, 0.9], [0.6, 0.9], [0.6, 1.0], [0.2, 1.0]]) {
            assert!((vertex.x - expected[0]).abs() < 1e-5 && (vertex.y - expected[1]).abs() < 1e-5, "{vertex}");
        }
    }

    #[test]
    fn the_standing_preset_turns_a_bone_through_its_normalized_frame() {
        let model = Model::parse(&figure(Version::One)).unwrap();
        let pose = model.pose(&model.standing());
        let preset: Value = serde_json::from_str(STANDING).unwrap();
        let [x, y, z, w] = floats(&preset["data"]["rightUpperArm"]["rotation"], [0.0; 4]);
        assert!(pose.rotations[2].abs_diff_eq(Quat::from_xyzw(-x, y, -z, w), 1e-5), "with an unrotated parent the raw rotation is the normalized one");
        let points = positions(&model, &pose);
        let hips = Mat4::from_quat(pose.rotations[1]);
        let expected = hips * Mat4::from_translation(Vec3::new(0.2, 0.95, 0.0)) * Mat4::from_quat(pose.rotations[2]) * Mat4::from_translation(Vec3::new(-0.2, -0.95, 0.0));
        assert!(points[5].abs_diff_eq(expected.transform_point3(Vec3::new(0.6, 0.9, 0.0)), 1e-5));
        assert!(points[0].abs_diff_eq(hips.transform_point3(Vec3::new(-0.2, 0.0, 0.0)), 1e-5), "the body follows the hips");
    }

    #[test]
    fn under_a_turned_parent_the_normalized_rotation_is_conjugated() {
        let mut model = Model::parse(&figure(Version::Zero)).unwrap();
        let turn = Quat::from_rotation_z(0.5);
        model.nodes[1].rotation = turn;
        model.nodes[0].scale = Vec3::splat(0.01);
        model.rest_turns = model.worlds(&model.rest()).iter().map(rotation).collect();
        let pose = model.pose(&model.standing());
        let preset: Value = serde_json::from_str(STANDING).unwrap();
        let normalized = Quat::from_array(floats(&preset["data"]["rightUpperArm"]["rotation"], [0.0; 4]));
        assert!(pose.rotations[2].abs_diff_eq(turn.inverse() * normalized * turn, 1e-5));
    }

    #[test]
    fn a_normalized_hips_position_moves_the_hips_in_model_space() {
        let mut model = Model::parse(&figure(Version::One)).unwrap();
        model.nodes[0].translation = Vec3::new(0.0, 0.5, 0.0);
        assert_eq!(model.rest_hips(), Some(Vec3::new(0.0, 0.5, 0.0)));
        let pose = model.pose(&Humanoid { rotations: HashMap::new(), hips: Some(Vec3::new(0.1, 0.7, 0.0)) });
        assert!(pose.translations[1].abs_diff_eq(Vec3::new(0.1, 0.2, 0.0), 1e-6), "the hips' parent offset is taken out");
        assert!(positions(&model, &pose)[0].abs_diff_eq(Vec3::new(-0.1, 0.7, 0.0), 1e-5));
    }

    #[test]
    fn fitting_scales_to_the_height_and_stands_on_the_floor_facing_the_viewer() {
        for version in [Version::Zero, Version::One] {
            let model = Model::parse(&figure(version)).unwrap();
            let skinned = model.skinned().unwrap();
            let fit = Fit::new(&model, &skinned, 0.3);
            let worlds = model.worlds(&model.rest());
            let placement = fit.placement(&model, &worlds, -0.17);
            let (low, high) = bounds(&skinned.positions(&skinned.palette(&worlds, placement)));
            assert!((low.y + 0.17).abs() < 1e-5 && (high.y - 0.13).abs() < 1e-5, "{low} {high}");
            let facing = placement.transform_vector3(Vec3::from(skinned.vertices[0].normal));
            assert!(facing.z > 0.0, "a VRM 0 figure faces -Z and is turned toward the viewer ({version:?})");
        }
    }

    #[test]
    fn feet_are_planted_where_the_rest_pose_stood_them() {
        let mut builder = Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": { "hips": { "node": 1 }, "leftFoot": { "node": 2 }, "rightFoot": { "node": 3 } } } } }));
        let position = builder.accessor("VEC3", &[-0.2, 0.0, 0.0, 0.2, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "children": [1], "mesh": 0 }, { "translation": [0.0, 0.5, 0.0], "children": [2, 3] }, { "translation": [-0.1, -0.4, 0.0] }, { "translation": [0.1, -0.4, 0.0] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let skinned = model.skinned().unwrap();
        let fit = Fit::new(&model, &skinned, 1.0);
        assert!((fit.ankle - 0.1).abs() < 1e-6);
        let mut lifted = model.pose(&Humanoid { rotations: HashMap::new(), hips: Some(Vec3::new(0.3, 0.8, 0.2)) });
        lifted.rotations[1] = Quat::from_rotation_z(0.2);
        let worlds = model.worlds(&lifted);
        let feet = model.feet(&worlds).unwrap().map(|foot| fit.placement(&model, &worlds, -0.5).transform_point3(foot));
        assert!(((feet[0].x + feet[1].x) / 2.0).abs() < 1e-5 && ((feet[0].z + feet[1].z) / 2.0).abs() < 1e-5, "the feet are centred: {feet:?}");
        assert!((feet[0].y.min(feet[1].y) - (-0.5 + 0.1)).abs() < 1e-5, "the lower foot stands at its ankle height: {feet:?}");
    }

    #[test]
    fn an_expression_drives_its_morph_targets_and_only_their_vertices_change() {
        let mut builder = Builder::new(serde_json::json!({ "VRM": { "humanoid": { "humanBones": [] }, "blendShapeMaster": { "blendShapeGroups": [{ "presetName": "blink", "binds": [{ "mesh": 0, "index": 0, "weight": 100 }] }, { "presetName": "a", "isBinary": true, "binds": [{ "mesh": 0, "index": 1, "weight": 50 }] }] } } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let blink = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0]);
        let open = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position }, "targets": [{ "POSITION": blink }, { "POSITION": open }] }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let mut skinned = model.skinned().unwrap();
        let mut pose = model.rest();
        model.express(&mut pose, "blink", 0.5);
        model.express(&mut pose, "aa", 0.6);
        let mut changed = skinned.morph(&pose.weights);
        changed.sort();
        assert_eq!(changed, vec![1, 2]);
        assert_eq!(skinned.vertices[2].position, [0.0, 0.5, 0.0]);
        assert_eq!(skinned.vertices[1].position, [1.0, 0.25, 0.0], "a binary expression applies fully, at its bind weight");
        assert!(skinned.morph(&pose.weights).is_empty(), "unchanged weights touch nothing");
        let rest = model.rest();
        skinned.morph(&rest.weights);
        assert_eq!(skinned.vertices[2].position, [0.0, 1.0, 0.0]);
        assert_eq!(skinned.vertices[1].position, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_morph_update_keeps_the_unchanged_targets_on_a_shared_vertex() {
        let mut builder = Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": {} } } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let up = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0]);
        let right = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.25, 0.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position }, "targets": [{ "POSITION": up }, { "POSITION": right }] }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let mut skinned = model.skinned().unwrap();
        skinned.morph(&[vec![1.0, 0.0]]);
        assert_eq!(skinned.morph(&[vec![1.0, 1.0]]), vec![2]);
        assert_eq!(skinned.vertices[2].position, [0.25, 1.5, 0.0]);
    }

    #[test]
    fn outline_and_depth_write_settings_read_as_three_vrm_reads_them() {
        let mut builder = Builder::new(serde_json::json!({ "VRM": { "humanoid": { "humanBones": [] }, "materialProperties": [
            { "shader": "VRM/MToon", "keywordMap": { "_ALPHABLEND_ON": true }, "floatProperties": { "_BlendMode": 3, "_ZWrite": 1, "_OutlineWidthMode": 1, "_OutlineWidth": 0.08, "_OutlineColorMode": 1, "_OutlineLightingMix": 0.5 }, "vectorProperties": {}, "textureProperties": {}, "renderQueue": 2501 },
            { "shader": "VRM/MToon", "keywordMap": { "_ALPHABLEND_ON": true }, "floatProperties": { "_BlendMode": 2, "_ZWrite": 0, "_OutlineWidthMode": 0, "_OutlineWidth": 0.08 }, "vectorProperties": {}, "textureProperties": {}, "renderQueue": 3000 },
            { "shader": "VRM/MToon", "keywordMap": { "_ALPHATEST_ON": true }, "floatProperties": { "_BlendMode": 1, "_OutlineWidthMode": 2, "_OutlineWidth": 0.5 }, "vectorProperties": { "_OutlineColor": [1.0, 0.5, 0.0, 1.0] }, "textureProperties": {}, "renderQueue": 2450 },
            { "shader": "VRM/MToon", "keywordMap": { "_ALPHATEST_ON": true }, "floatProperties": { "_BlendMode": 3, "_Cutoff": 0.3 }, "vectorProperties": {}, "textureProperties": {}, "renderQueue": 2450 }
        ] } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position }, "material": 0 }] }]));
        builder.set("materials", serde_json::json!([{}, {}, {}, {}]));
        let model = Model::parse(&builder.glb()).unwrap();
        assert!(matches!(model.materials[3].alpha, Alpha::Cutout(cutoff) if (cutoff - 0.3).abs() < 1e-6), "the keywords decide the alpha mode, not _BlendMode");
        let [zwrite, plain, cutout] = [0, 1, 2].map(|at| model.materials[at].clone());
        assert!(zwrite.alpha == Alpha::Blend && zwrite.depth_write);
        assert!(plain.alpha == Alpha::Blend && !plain.depth_write && plain.outline.is_none());
        let outline = zwrite.outline.unwrap();
        assert!(outline.width == OutlineWidth::World && (outline.factor - 0.0008).abs() < 1e-7 && outline.lighting_mix == 0.5 && outline.color == [0.0; 3]);
        let outline = cutout.outline.unwrap();
        assert!(cutout.depth_write && outline.width == OutlineWidth::Screen && outline.lighting_mix == 0.0);
        assert!((outline.color[1] - srgb_to_linear(0.5)).abs() < 1e-6, "the v0 outline colour is sRGB");
    }

    #[test]
    fn sparse_morph_targets_expand_and_blend_by_weight() {
        let mut builder = Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": {} } } }));
        let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let changed = builder.indices(&[2]);
        let changed_view = builder.json["accessors"][changed]["bufferView"].clone();
        let delta = builder.view(&[0.0f32, 0.5, 0.0].iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<u8>>());
        let target = builder.push_accessor(serde_json::json!({ "componentType": 5126, "count": 3, "type": "VEC3", "sparse": { "count": 1, "indices": { "bufferView": changed_view, "componentType": 5123 }, "values": { "bufferView": delta } } }));
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "weights": [0.5], "primitives": [{ "attributes": { "POSITION": position }, "targets": [{ "POSITION": target }] }] }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let skinned = model.skinned().unwrap();
        assert_eq!(skinned.vertices[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(skinned.vertices[2].position, [0.0, 1.25, 0.0], "the mesh's default weight applies at load");
    }
}
