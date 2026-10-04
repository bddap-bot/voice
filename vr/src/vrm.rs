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

#[derive(Clone, Debug)]
pub struct Material {
    pub base: [f32; 4],
    pub base_image: Option<usize>,
    pub shade: [f32; 3],
    pub shade_image: Option<usize>,
    pub alpha: Alpha,
    pub double_sided: bool,
    pub shading_shift: f32,
    pub shading_toony: f32,
    pub equalization: f32,
    pub queue: i32,
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

pub struct Model {
    pub version: Version,
    nodes: Vec<Node>,
    meshes: Vec<Mesh>,
    skins: Vec<Skin>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    humanoid: HashMap<String, usize>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Draw {
    pub first: u32,
    pub count: u32,
    pub material: usize,
}

pub struct Posed {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<Draw>,
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
        let [r, g, b, a] = floats(&properties["vectorProperties"][name], [1.0; 4]);
        [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
    };
    let texture = |name: &str| document.texture_image(&properties["textureProperties"][name]);
    let shade_shift = float("_ShadeShift", 0.0);
    let shading_toony = shade_shift.mul_add(0.5, 0.5) * (1.0 - float("_ShadeToony", 0.9)) + float("_ShadeToony", 0.9);
    let alpha = match float("_BlendMode", 0.0) as i32 {
        0 => Alpha::Opaque,
        1 => Alpha::Cutout(float("_Cutoff", 0.5)),
        _ => Alpha::Blend,
    };
    let [r, g, b, _] = color("_ShadeColor");
    Material {
        base: color("_Color"),
        base_image: texture("_MainTex"),
        shade: [r, g, b],
        shade_image: texture("_ShadeTexture"),
        alpha,
        double_sided: float("_CullMode", 2.0) as i32 == 0,
        shading_shift: -shade_shift - (1.0 - shading_toony),
        shading_toony,
        equalization: 1.0 - float("_IndirectLightIntensity", 0.1),
        queue: properties["renderQueue"].as_i64().map_or(2000, |queue| queue as i32),
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
    let queue = match alpha {
        Alpha::Opaque => 2000,
        Alpha::Cutout(_) => 2450,
        Alpha::Blend => 3000 + toon["renderQueueOffsetNumber"].as_i64().unwrap_or(0) as i32,
    };
    Material {
        base,
        base_image,
        shade: if lit { [base[0], base[1], base[2]] } else { floats(&toon["shadeColorFactor"], [0.0; 3]) },
        shade_image: if lit { base_image } else { document.texture_image(&toon["shadeMultiplyTexture"]["index"]) },
        alpha,
        double_sided: material["doubleSided"].as_bool() == Some(true),
        shading_shift: number(&toon["shadingShiftFactor"], 0.0),
        shading_toony: number(&toon["shadingToonyFactor"], 0.9),
        equalization: number(&toon["giEqualizationFactor"], 0.9),
        queue,
    }
}

impl Model {
    pub fn parse(bytes: &[u8]) -> Result<Model, String> {
        let (json, binary) = chunks(bytes)?;
        let document = Document { json, binary };
        let json = &document.json;
        let extensions = &json["extensions"];
        let (version, humanoid) = if let Some(vrm) = extensions.get("VRMC_vrm") {
            let bones = vrm["humanoid"]["humanBones"].as_object().ok_or("VRMC_vrm without humanBones")?;
            (Version::One, bones.iter().filter_map(|(name, bone)| Some((name.clone(), index(&bone["node"])?))).collect())
        } else if let Some(vrm) = extensions.get("VRM") {
            let bones = vrm["humanoid"]["humanBones"].as_array().ok_or("VRM without humanBones")?;
            (Version::Zero, bones.iter().filter_map(|bone| Some((bone["bone"].as_str()?.to_owned(), index(&bone["node"])?))).collect())
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
                nodes.get_mut(child).ok_or("child node out of range")?.parent = Some(parent);
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
        if !nodes.iter().any(|node| node.mesh.is_some()) {
            return Err("the puppet has no mesh".into());
        }

        let mut used: Vec<usize> = materials.iter().flat_map(|material| [material.base_image, material.shade_image]).flatten().collect();
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
        }
        Ok(Model { version, nodes, meshes, skins, materials, images, humanoid })
    }

    fn worlds(&self, rotations: &[Quat]) -> Vec<Mat4> {
        let mut worlds: Vec<Option<Mat4>> = vec![None; self.nodes.len()];
        fn resolve(model: &Model, rotations: &[Quat], worlds: &mut [Option<Mat4>], node: usize) -> Mat4 {
            if let Some(world) = worlds[node] {
                return world;
            }
            let entry = &model.nodes[node];
            let local = Mat4::from_scale_rotation_translation(entry.scale, rotations[node], entry.translation);
            let world = entry.parent.map_or(local, |parent| resolve(model, rotations, worlds, parent) * local);
            worlds[node] = Some(world);
            world
        }
        (0..self.nodes.len()).map(|node| resolve(self, rotations, &mut worlds, node)).collect()
    }

    pub fn rest(&self) -> Vec<Quat> {
        self.nodes.iter().map(|node| node.rotation).collect()
    }

    pub fn standing(&self) -> Vec<Quat> {
        let preset: Value = serde_json::from_str(STANDING).expect("standing.json");
        let mut pose = self.rest();
        let rest_worlds = self.worlds(&pose);
        for (name, bone) in preset["data"].as_object().expect("standing.json data") {
            let name = match self.version {
                Version::Zero => name.clone(),
                Version::One => name.replace("ThumbProximal", "ThumbMetacarpal").replace("ThumbIntermediate", "ThumbProximal"),
            };
            let Some(&node) = self.humanoid.get(&name) else { continue };
            let [x, y, z, w] = floats(&bone["rotation"], [0.0, 0.0, 0.0, 1.0]);
            let normalized = match self.version {
                Version::Zero => Quat::from_xyzw(x, y, z, w),
                Version::One => Quat::from_xyzw(-x, y, -z, w),
            };
            let parent = self.nodes[node].parent.map_or(Quat::IDENTITY, |parent| rest_worlds[parent].to_scale_rotation_translation().1);
            pose[node] = (parent.inverse() * normalized * parent * self.nodes[node].rotation).normalize();
        }
        pose
    }

    pub fn pose(&self, rotations: &[Quat]) -> Posed {
        let worlds = self.worlds(rotations);
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut draws = Vec::new();
        for (node, entry) in self.nodes.iter().enumerate() {
            let Some(mesh) = entry.mesh.map(|mesh| &self.meshes[mesh]) else { continue };
            let joints: Option<Vec<Mat4>> = entry.skin.map(|skin| {
                let skin = &self.skins[skin];
                skin.joints.iter().zip(&skin.inverse_bind).map(|(&joint, inverse)| worlds[joint] * *inverse).collect()
            });
            for primitive in &mesh.primitives {
                let base = vertices.len() as u32;
                for vertex in 0..primitive.positions.len() {
                    let mut position = primitive.positions[vertex];
                    let mut normal = primitive.normals[vertex];
                    for (target, &weight) in primitive.targets.iter().zip(&mesh.weights) {
                        if weight != 0.0 {
                            position += target.positions[vertex] * weight;
                            normal += target.normals[vertex] * weight;
                        }
                    }
                    let matrix = match (&joints, primitive.joints.get(vertex), primitive.weights.get(vertex)) {
                        (Some(joints), Some(slots), Some(weights)) if weights.iter().any(|&weight| weight > 0.0) => slots.iter().zip(weights).filter(|(_, &weight)| weight > 0.0).fold(Mat4::ZERO, |sum, (&slot, &weight)| sum + joints[slot] * weight),
                        _ => worlds[node],
                    };
                    vertices.push(Vertex { position: matrix.transform_point3(position).into(), normal: matrix.transform_vector3(normal).normalize_or_zero().into(), uv: primitive.uvs[vertex].into() });
                }
                draws.push(Draw { first: indices.len() as u32, count: primitive.indices.len() as u32, material: primitive.material.unwrap_or(self.materials.len() - 1) });
                indices.extend(primitive.indices.iter().map(|&vertex| vertex + base));
            }
        }
        let queue = |draw: &Draw| self.materials[draw.material].queue;
        draws.sort_by_key(queue);
        Posed { vertices, indices, draws }
    }
}

pub struct Fit {
    pub scale: f32,
    pub ground: f32,
}

impl Posed {
    pub fn bounds(&self) -> (Vec3, Vec3) {
        self.vertices.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(low, high), vertex| {
            let point = Vec3::from(vertex.position);
            (low.min(point), high.max(point))
        })
    }

    pub fn fit(&self, height: f32) -> Fit {
        let (low, high) = self.bounds();
        let size = high.y - low.y;
        Fit { scale: if size > 0.0 { height / size } else { 1.0 }, ground: low.y }
    }

    pub fn place(&mut self, fit: &Fit, version: Version, floor: f32) {
        let turn = match version {
            Version::Zero => Quat::from_rotation_y(std::f32::consts::PI),
            Version::One => Quat::IDENTITY,
        };
        let placement = Mat4::from_translation(Vec3::new(0.0, floor - fit.ground * fit.scale, 0.0)) * Mat4::from_quat(turn) * Mat4::from_scale(Vec3::splat(fit.scale));
        for vertex in &mut self.vertices {
            vertex.position = placement.transform_point3(vertex.position.into()).into();
            vertex.normal = (turn * Vec3::from(vertex.normal)).into();
        }
    }
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
            Version::Zero => serde_json::json!({ "VRM": { "humanoid": { "humanBones": [{ "bone": "hips", "node": 1 }, { "bone": "rightUpperArm", "node": 2 }] }, "materialProperties": [{ "shader": "VRM/MToon", "floatProperties": { "_BlendMode": 1, "_Cutoff": 0.5 }, "vectorProperties": { "_Color": [1, 1, 1, 1], "_ShadeColor": [0.5, 0.5, 0.5, 1] }, "textureProperties": { "_MainTex": 0, "_ShadeTexture": 0 }, "renderQueue": 2450 }] } }),
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

    #[test]
    fn the_rest_pose_skins_vertices_where_the_file_put_them() {
        let model = Model::parse(&figure(Version::One)).unwrap();
        let posed = model.pose(&model.rest());
        assert_eq!(posed.vertices.len(), 8);
        assert_eq!(posed.indices.len(), 12);
        for (vertex, expected) in posed.vertices.iter().zip([[-0.2, 0.0], [0.2, 0.0], [0.2, 1.0], [-0.2, 1.0], [0.2, 0.9], [0.6, 0.9], [0.6, 1.0], [0.2, 1.0]]) {
            assert!((vertex.position[0] - expected[0]).abs() < 1e-5 && (vertex.position[1] - expected[1]).abs() < 1e-5, "{vertex:?}");
        }
    }

    #[test]
    fn the_standing_preset_turns_a_bone_through_its_normalized_frame() {
        let model = Model::parse(&figure(Version::One)).unwrap();
        let pose = model.standing();
        let preset: Value = serde_json::from_str(STANDING).unwrap();
        let [x, y, z, w] = floats(&preset["data"]["rightUpperArm"]["rotation"], [0.0; 4]);
        assert!(pose[2].abs_diff_eq(Quat::from_xyzw(-x, y, -z, w), 1e-5), "with an unrotated parent the raw rotation is the normalized one");
        let posed = model.pose(&pose);
        let moved = Vec3::from(posed.vertices[5].position);
        let hips = Mat4::from_quat(pose[1]);
        let expected = hips * Mat4::from_translation(Vec3::new(0.2, 0.95, 0.0)) * Mat4::from_quat(pose[2]) * Mat4::from_translation(Vec3::new(-0.2, -0.95, 0.0));
        assert!(moved.abs_diff_eq(expected.transform_point3(Vec3::new(0.6, 0.9, 0.0)), 1e-5));
        assert!(Vec3::from(posed.vertices[0].position).abs_diff_eq(hips.transform_point3(Vec3::new(-0.2, 0.0, 0.0)), 1e-5), "the body follows the hips");
    }

    #[test]
    fn under_a_turned_parent_the_normalized_rotation_is_conjugated() {
        let mut model = Model::parse(&figure(Version::Zero)).unwrap();
        let turn = Quat::from_rotation_z(0.5);
        model.nodes[1].rotation = turn;
        model.nodes[0].scale = Vec3::splat(0.01);
        let pose = model.standing();
        let preset: Value = serde_json::from_str(STANDING).unwrap();
        let normalized = Quat::from_array(floats(&preset["data"]["rightUpperArm"]["rotation"], [0.0; 4]));
        assert!(pose[2].abs_diff_eq(turn.inverse() * normalized * turn, 1e-5));
    }

    #[test]
    fn fitting_scales_to_the_height_and_stands_on_the_floor_facing_the_viewer() {
        for version in [Version::Zero, Version::One] {
            let model = Model::parse(&figure(version)).unwrap();
            let mut posed = model.pose(&model.rest());
            let fit = posed.fit(0.3);
            posed.place(&fit, version, -0.17);
            let (low, high) = posed.bounds();
            assert!((low.y + 0.17).abs() < 1e-5 && (high.y - 0.13).abs() < 1e-5, "{low} {high}");
            assert!(posed.vertices[0].normal[2] > 0.0, "a VRM 0 figure faces -Z and is turned toward the viewer ({version:?})");
        }
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
        let posed = model.pose(&model.rest());
        assert_eq!(posed.vertices[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(posed.vertices[2].position, [0.0, 1.25, 0.0]);
    }
}
