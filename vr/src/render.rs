use ash::vk;
use glam::{Mat3, Mat4, Vec3};

use std::collections::HashMap;
use std::rc::Rc;

use crate::vrm::particles::{Blend, Instance, Mode, Particles};
use crate::vrm::{Alpha, Features, Model, OutlineWidth, Skinned, Vertex};
use crate::vulkan::{vk_error, Gpu};

pub const FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;
const DEPTH: vk::Format = vk::Format::D32_SFLOAT;
const RING: usize = 3;
const NEAR: f32 = 0.01;
pub const PAGE_HEIGHT: f32 = 2.7;
const FAR: f32 = 10.0;
/// Samples per pixel, resolved into the overlay's texture: one sample breaks outlines thinner than a texel into dots.
const SAMPLES: vk::SampleCountFlags = vk::SampleCountFlags::TYPE_4;
/// Pixels per radian a render with no headset is sized for: 2160 pixels across a 110° field.
pub const OFF_HEADSET_DENSITY: f32 = 2160.0 / (110.0 * std::f32::consts::PI / 180.0);
/// The widest anisotropic filtering the avatar's textures take: on the Frame 16x cost about 0.3 ms a frame more than 4x.
const ANISOTROPY: f32 = 4.0;
/// The closest a window is seen from: the puppet held up on the hand.
pub const NEAREST: f32 = 0.35;

/// The most one decal page takes with its mipmaps, unless one decal alone takes more: drivers refuse a single allocation of a
/// whole avatar's decals.
const PAGE_BYTES: u64 = 512 << 20;

/// A WGSL shader parsed and validated once, compiled to SPIR-V for each set of override constants it is used with.
struct Shader {
    module: naga::Module,
    info: naga::valid::ModuleInfo,
}

impl Shader {
    fn parse(text: &str) -> Shader {
        let module = naga::front::wgsl::parse_str(text).unwrap_or_else(|error| panic!("{}", error.emit_to_string(text)));
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::PUSH_CONSTANT).validate(&module).unwrap_or_else(|error| panic!("{error:?}"));
        Shader { module, info }
    }

    fn spirv(&self, stage: naga::ShaderStage, entry: &str, constants: &naga::back::PipelineConstants) -> Result<Vec<u32>, String> {
        let (module, info) = naga::back::pipeline_constants::process_overrides(&self.module, &self.info, Some((stage, entry)), constants).map_err(|error| format!("{entry}: {error:?}"))?;
        let options = naga::back::spv::Options { flags: naga::back::spv::Options::default().flags - naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE, ..Default::default() };
        let pipeline = naga::back::spv::PipelineOptions { shader_stage: stage, entry_point: entry.into() };
        naga::back::spv::write_vec(&module, &info, &options, Some(&pipeline)).map_err(|error| format!("{entry}: {error:?}"))
    }

    /// The length of the binding array `name`.
    fn binding_array(&self, name: &str) -> usize {
        self.module.global_variables.iter().find(|(_, global)| global.name.as_deref() == Some(name)).map(|(_, global)| match self.module.types[global.ty].inner {
            naga::TypeInner::BindingArray { size: naga::ArraySize::Constant(size), .. } => size.get() as usize,
            _ => panic!("{name} is not a binding array of a fixed size"),
        }).unwrap_or_else(|| panic!("the shader has no {name}"))
    }
}

static AVATAR_SHADER: std::sync::LazyLock<Shader> = std::sync::LazyLock::new(|| Shader::parse(include_str!("../shaders/avatar.wgsl")));
static PARTICLE_SHADER: std::sync::LazyLock<Shader> = std::sync::LazyLock::new(|| Shader::parse(include_str!("../shaders/particles.wgsl")));
static DECAL_PAGES: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| AVATAR_SHADER.binding_array("decal_pages"));

/// The override constants a figure pipeline's shaders are compiled with.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
struct Shading {
    pages: usize,
    features: Features,
    untinted: bool,
    outline_unlit: bool,
}

impl Shading {
    fn constants(self) -> naga::back::PipelineConstants {
        let Shading { pages, features, untinted, outline_unlit } = self;
        [
            ("decal_pages_used", pages as f64),
            ("decal_slots", features.decals as f64),
            ("matcap_slots", features.matcaps as f64),
            ("shadow_reached", features.shadow as u8 as f64),
            ("second_shadow_reached", features.second_shadow as u8 as f64),
            ("rim_reached", features.rim as u8 as f64),
            ("rim_shade_reached", features.rim_shade as u8 as f64),
            ("emission_reached", features.emission as u8 as f64),
            ("flat_reached", features.flat as u8 as f64),
            ("ramp_reached", features.ramp as u8 as f64),
            ("untinted_reached", untinted as u8 as f64),
            ("outline_unlit", outline_unlit as u8 as f64),
        ].into_iter().map(|(name, value)| (name.to_owned(), value)).collect()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    view_projection: [f32; 16],
    camera: [f32; 3],
    material: u32,
    outline: u32,
    outline_scale: f32,
    focal: f32,
    /// Seconds the figure's expressions have run, wrapped at THEME_WRAP, which turns a theme colour's hue at its speed.
    seconds: f32,
}

/// The seconds a figure has shown wrap at this before the shader reads them as an f32, which past a day would step them
/// too coarsely to turn a theme colour's hue; a whole turn at any speed in thousandths.
const THEME_WRAP: f64 = 20_000.0;

#[repr(C)]
#[derive(Clone, Copy)]
struct ParticlePush {
    view_projection: [f32; 16],
    eye: [f32; 3],
    now: f32,
    right: [f32; 3],
    system: u32,
    up: [f32; 3],
    focal: f32,
}

/// One particle system's values the shader reads, in `particles.wgsl`'s `System` layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct SystemData {
    emitter: [f32; 16],
    tint: [f32; 4],
    emission: [f32; 3],
    cutoff: f32,
    uv: [f32; 4],
    pivot: [f32; 3],
    length_scale: f32,
    speed_ranges: [f32; 4],
    velocity_scale: f32,
    noise_frequency: f32,
    damping: f32,
    flags: u32,
    mode: u32,
    align: u32,
    blend: u32,
    max_size: f32,
}

/// The view's right and up axes in the space `view_projection` maps from, and its focal length up: a perspective's
/// clip x runs along the view's right and its w against the view's forward.
fn view_axes(view_projection: Mat4) -> (Vec3, Vec3, f32) {
    let ahead = view_projection.row(3).truncate();
    let back = -ahead.normalize();
    let across = view_projection.row(0).truncate();
    let right = (across - across.dot(back) * back).normalize();
    let up = back.cross(right);
    (right, up, view_projection.row(1).truncate().dot(up).abs() / ahead.length())
}

/// Where the eye whose view and projection make `view_projection` sits: the point every clip coordinate but depth
/// leaves at zero.
fn camera(view_projection: Mat4) -> Vec3 {
    let rows = view_projection.transpose();
    let solve = Mat3::from_cols(rows.x_axis.truncate(), rows.y_axis.truncate(), rows.w_axis.truncate()).transpose();
    solve.inverse() * -Vec3::new(rows.x_axis.w, rows.y_axis.w, rows.w_axis.w)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MatcapData {
    color: [f32; 4],
    hue: [f32; 4],
    texture: i32,
    reflection: u32,
    on: u32,
    circled: u32,
    border: f32,
    intensity: f32,
    base_mix: f32,
    light_mask: f32,
    replace: f32,
    multiply: f32,
    add: f32,
    screen: f32,
    mixed: f32,
    light_add: f32,
    emission: f32,
    circle_border: f32,
    circle_blur: f32,
    theme: i32,
    pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CoatData {
    levels: [f32; 4],
    inverts: [f32; 4],
    st: [f32; 4],
    specular_tint: [f32; 3],
    fresnel: f32,
    reflection_tint: [f32; 3],
    map: i32,
    gsaa: [f32; 2],
    on: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LookData {
    decal: [f32; 4],
    intensity: [f32; 4],
    blend: [f32; 4],
    rect: [f32; 4],
    page: [i32; 4],
    layer: [u32; 4],
    decal_hue: [[f32; 4]; 4],
    emission_color: [f32; 3],
    emission_strength: f32,
    emission_hue: f32,
    shifts_hue: f32,
    flat: u32,
    light_cap: f32,
    /// Each global theme colour the look's emission or matcaps are drawn in, by slot.
    themes: [ThemeData; 4],
    /// The slot of the theme colour the emission is drawn in, or -1 for its own colour.
    emission_theme: i32,
    shadowed: u32,
    shadow_border: f32,
    shadow_blur: f32,
    shadow_strength: f32,
    second_border: f32,
    second_blur: f32,
    second_weight: f32,
    edge_color: [f32; 3],
    edge_range: f32,
    rim_color: [f32; 3],
    rim_strength: f32,
    rim_main: f32,
    rim_border: f32,
    rim_blur: f32,
    rim_power: f32,
    rim_lit: f32,
    rim_shadow_mask: f32,
    /// Whether the rim draws on back faces.
    rim_backface: u32,
    /// The layer of the toon layers holding the rim tint, or -1 for none.
    rim_tint_layer: i32,
    rim_shade_color: [f32; 3],
    rim_shade_strength: f32,
    rim_shade_border: f32,
    rim_shade_blur: f32,
    rim_shade_power: f32,
    rim_shade_pad: f32,
    matcaps: [MatcapData; 4],
    ramp: [[f32; 4]; RAMP / 4],
    ramped: u32,
    shadow_tint: f32,
    ramp_pad: [f32; 2],
    coat: CoatData,
}

const _: () = assert!(std::mem::size_of::<LookData>() % 16 == 0 && std::mem::offset_of!(LookData, edge_color) % 16 == 0 && std::mem::offset_of!(LookData, rim_color) % 16 == 0 && std::mem::offset_of!(LookData, matcaps) % 16 == 0 && std::mem::offset_of!(LookData, coat) % 16 == 0 && std::mem::offset_of!(CoatData, gsaa) % 8 == 0);

/// The points a ramp-lit look's ramp is held at, as many as the avatar shader reads.
const RAMP: usize = 16;

/// The look's ramp at RAMP evenly spaced points, read linearly between its own.
fn ramp_points(ramp: &[f32]) -> [[f32; 4]; RAMP / 4] {
    let mut out = [[1.0; 4]; RAMP / 4];
    if ramp.len() < 2 {
        return out;
    }
    for i in 0..RAMP {
        let at = i as f32 * (ramp.len() - 1) as f32 / (RAMP - 1) as f32;
        let (low, high) = (ramp[at.floor() as usize], ramp[at.ceil() as usize]);
        out[i / 4][i % 4] = low + (high - low) * at.fract();
    }
    out
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ThemeData {
    offsets: [f32; 4],
    hsv: [f32; 3],
    exposure: f32,
}

fn look_data(look: &crate::vrm::expressions::Look, places: &[Place], rim_tint_layer: i32) -> LookData {
    use crate::vrm::expressions::{Lighting, Mapping};
    let decal = |value: fn(&crate::vrm::expressions::Decal) -> f32| look.decals.each_ref().map(|decal| decal.as_ref().map_or(0.0, value));
    let emission = look.emission.as_ref();
    let shadow = match &look.lighting {
        Lighting::Flat { shadow, .. } => shadow.as_ref(),
        Lighting::Lit { .. } => None,
    };
    let second = shadow.and_then(|shadow| shadow.second.as_ref());
    let edge = shadow.and_then(|shadow| shadow.edge.as_ref());
    let rim = look.rim.as_ref();
    let rim_shade = look.rim_shade.as_ref();
    let features = Features::of(look);
    LookData {
        decal: decal(|decal| decal.alpha),
        intensity: decal(|decal| decal.intensity),
        blend: decal(|decal| decal.color_blend as u8 as f32),
        rect: look.rect,
        page: look.decals.each_ref().map(|decal| decal.as_ref().map_or(-1, |decal| places[decal.image].page as i32)),
        layer: look.decals.each_ref().map(|decal| decal.as_ref().map_or(0, |decal| places[decal.image].layer)),
        decal_hue: look.decals.each_ref().map(|decal| decal.as_ref().and_then(|decal| decal.hue.as_ref()).map_or([0.0; 4], |hue| [hue.shift, hue.space, hue.select, 1.0])),
        emission_color: emission.map_or([0.0; 3], |emission| emission.color),
        emission_strength: emission.map_or(0.0, |emission| emission.strength),
        emission_hue: emission.map_or(0.0, |emission| emission.hue),
        shifts_hue: emission.is_some_and(|emission| emission.shifts_hue) as u32 as f32,
        themes: look.themes.each_ref().map(|theme| theme.as_ref().map_or_else(ThemeData::default, |theme| ThemeData { offsets: [theme.hue, theme.saturation, theme.value, theme.speed], hsv: theme.hsv, exposure: theme.exposure })),
        emission_theme: emission.and_then(|emission| emission.theme).map_or(-1, |slot| slot as i32),
        flat: features.flat as u32,
        light_cap: match look.lighting {
            Lighting::Flat { cap: Some(cap), .. } | Lighting::Lit { ramp: Some(_), cap: Some(cap), .. } => cap,
            _ => f32::MAX,
        },
        ramp: match &look.lighting {
            Lighting::Lit { ramp: Some(ramp), .. } => ramp_points(ramp),
            _ => [[1.0; 4]; RAMP / 4],
        },
        ramped: features.ramp as u32,
        shadow_tint: match look.lighting {
            Lighting::Lit { shadow_tint, .. } => shadow_tint,
            _ => 0.0,
        },
        ramp_pad: [0.0; 2],
        shadowed: features.shadow as u32,
        shadow_border: shadow.map_or(0.0, |shadow| shadow.border),
        shadow_blur: shadow.map_or(0.0, |shadow| shadow.blur),
        shadow_strength: shadow.map_or(0.0, |shadow| shadow.strength),
        second_border: second.map_or(0.0, |second| second.border),
        second_blur: second.map_or(0.0, |second| second.blur),
        second_weight: second.map_or(0.0, |second| second.weight),
        edge_color: edge.map_or([0.0; 3], |edge| edge.color),
        edge_range: edge.map_or(0.0, |edge| edge.range),
        rim_color: rim.map_or([0.0; 3], |rim| rim.color),
        rim_strength: rim.map_or(0.0, |rim| rim.strength),
        rim_main: rim.map_or(0.0, |rim| rim.main),
        rim_border: rim.map_or(0.0, |rim| rim.border),
        rim_blur: rim.map_or(0.0, |rim| rim.blur),
        rim_power: rim.map_or(0.0, |rim| rim.power),
        rim_lit: rim.map_or(0.0, |rim| rim.lit),
        rim_shadow_mask: rim.map_or(0.0, |rim| rim.shadow_mask),
        rim_backface: rim.is_some_and(|rim| rim.backface) as u32,
        rim_tint_layer,
        rim_shade_color: rim_shade.map_or([1.0; 3], |shade| shade.color),
        rim_shade_strength: rim_shade.map_or(0.0, |shade| shade.strength),
        rim_shade_border: rim_shade.map_or(0.0, |shade| shade.border),
        rim_shade_blur: rim_shade.map_or(0.0, |shade| shade.blur),
        rim_shade_power: rim_shade.map_or(1.0, |shade| shade.power),
        rim_shade_pad: 0.0,
        matcaps: look.matcaps.each_ref().map(|matcap| matcap.as_ref().map_or_else(MatcapData::default, |m| MatcapData {
            color: m.color,
            hue: m.hue.as_ref().map_or([0.0; 4], |hue| [hue.shift, hue.space, hue.select, 1.0]),
            texture: m.texture.map_or(-1, |texture| texture as i32),
            reflection: (m.mode == Mapping::Reflection) as u32,
            on: 1,
            circled: m.circle.is_some() as u32,
            border: m.border,
            intensity: m.intensity,
            base_mix: m.base_mix,
            light_mask: m.light_mask,
            replace: m.replace,
            multiply: m.multiply,
            add: m.add,
            screen: m.screen,
            mixed: m.mixed,
            light_add: m.light_add,
            emission: m.emission,
            circle_border: m.circle.as_ref().map_or(0.0, |circle| circle.border),
            circle_blur: m.circle.as_ref().map_or(0.0, |circle| circle.blur),
            theme: m.theme.map_or(-1, |slot| slot as i32),
            pad: [0.0; 2],
        })),
        coat: look.clearcoat.as_ref().map_or_else(CoatData::default, |coat| CoatData {
            levels: [coat.strength, coat.smoothness, coat.reflection, coat.specular],
            inverts: coat.inverts.map(|invert| invert as u32 as f32),
            st: coat.st,
            specular_tint: coat.specular_tint,
            fresnel: coat.fresnel,
            reflection_tint: coat.reflection_tint,
            map: coat.map.map_or(-1, |map| map as i32),
            gsaa: coat.gsaa,
            on: 1,
            pad: 0,
        }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    page: usize,
    layer: u32,
}

/// Every Vulkan device takes at least this many layers in one image.
const PAGE_LAYERS: u64 = 256;

fn decal_pages(decals: &[crate::vrm::Image]) -> (Vec<Vec<usize>>, Vec<Place>) {
    let mut sizes: std::collections::BTreeMap<(u32, u32), Vec<usize>> = Default::default();
    for (at, decal) in decals.iter().enumerate() {
        sizes.entry((decal.width, decal.height)).or_default().push(at);
    }
    let mut pages = Vec::new();
    let mut places = vec![Place { page: 0, layer: 0 }; decals.len()];
    for ((width, height), same) in sizes {
        let layer = width as u64 * height as u64 * 4 * 4 / 3;
        for page in same.chunks((PAGE_BYTES / layer).clamp(1, PAGE_LAYERS) as usize) {
            for (layer, &at) in page.iter().enumerate() {
                places[at] = Place { page: pages.len(), layer: layer as u32 };
            }
            pages.push(page.to_vec());
        }
    }
    (pages, places)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MaterialData {
    base: [f32; 4],
    shade: [f32; 3],
    blend: f32,
    shading_shift: f32,
    shading_toony: f32,
    cutoff: f32,
    equalization: f32,
    outline_color: [f32; 3],
    outline_lighting_mix: f32,
    outline_width: f32,
    outline_screen: f32,
    outline_layered: f32,
    padding: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipelineKey {
    blend: bool,
    cull: Cull,
    depth_write: bool,
    pass: Pass,
    shading: Shading,
}

/// How a draw meets the depth buffer. A fragment shader that may discard is shaded before its depth test, so only the
/// depth pass and outline hulls discard, and a depth-writing surface is shaded only where it is the nearest.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Pass {
    /// Lays down the depth of what passes the alpha test, before anything is shaded.
    Depth,
    /// Shades where the depth pass left this surface's own depth. An alpha-tested surface still discards here, where
    /// another surface coplanar with its holes left that depth; with depth writes off a discard keeps the early test.
    Equal { cutout: bool },
    /// Shades whatever passes the depth test: a blended surface, whose cutoff is zero.
    Shade,
    /// Shades what passes the alpha test: an outline hull, or an alpha-tested surface that writes no depth.
    Cutout,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Cull {
    None,
    Back,
    Front,
}

/// Texels across one eye's image of a window `width` metres wide seen from `distance` on a display of `density` pixels per radian: the compositor samples a window without mipmaps, so a denser image aliases.
pub fn eye_texels(density: f32, width: f32, distance: f32) -> u32 {
    (density * width / distance.max(NEAREST)).ceil() as u32
}

pub fn eye_projection(eye: Vec3, half_width: f32, half_height: f32) -> Option<Mat4> {
    if eye.z <= NEAR * 2.0 {
        return None;
    }
    let scale = NEAR / eye.z;
    let (left, right) = ((-half_width - eye.x) * scale, (half_width - eye.x) * scale);
    let (bottom, top) = ((-half_height - eye.y) * scale, (half_height - eye.y) * scale);
    let projection = Mat4::from_cols_array_2d(&[
        [2.0 * NEAR / (right - left), 0.0, 0.0, 0.0],
        [0.0, -2.0 * NEAR / (top - bottom), 0.0, 0.0],
        [(right + left) / (right - left), -(top + bottom) / (top - bottom), FAR / (NEAR - FAR), -1.0],
        [0.0, 0.0, NEAR * FAR / (NEAR - FAR), 0.0],
    ]);
    Some(projection * Mat4::from_translation(-eye))
}

struct Texture {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

struct Target {
    texture: Texture,
    framebuffer: vk::Framebuffer,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
}

/// Good until `RING - 1` more drawings are submitted: the one after reuses its target and fence.
pub struct Drawing {
    gpu: Rc<Gpu>,
    fence: vk::Fence,
    texture: openvr_sys::VRVulkanTextureData_t,
}

impl Drawing {
    pub fn drawn(self) -> Result<openvr_sys::VRVulkanTextureData_t, String> {
        unsafe { self.gpu.device.wait_for_fences(&[self.fence], true, u64::MAX) }.map_err(vk_error("vkWaitForFences"))?;
        Ok(self.texture)
    }
}

struct DrawCall {
    first: u32,
    count: u32,
    set: vk::DescriptorSet,
    depth: Option<vk::Pipeline>,
    pipeline: vk::Pipeline,
    material: u32,
    outline: bool,
    switched: Option<usize>,
}

type Buffer = (vk::Buffer, vk::DeviceMemory, *mut u8);

struct SetLayout {
    layout: vk::DescriptorSetLayout,
    sizes: Vec<vk::DescriptorPoolSize>,
}

impl SetLayout {
    fn new(device: &ash::Device, bindings: &[vk::DescriptorSetLayoutBinding]) -> Result<SetLayout, String> {
        let layout = unsafe { device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(bindings), None) }.map_err(vk_error("vkCreateDescriptorSetLayout"))?;
        let mut sizes: Vec<vk::DescriptorPoolSize> = Vec::new();
        for binding in bindings {
            match sizes.iter_mut().find(|size| size.ty == binding.descriptor_type) {
                Some(size) => size.descriptor_count += binding.descriptor_count,
                None => sizes.push(vk::DescriptorPoolSize { ty: binding.descriptor_type, descriptor_count: binding.descriptor_count }),
            }
        }
        Ok(SetLayout { layout, sizes })
    }

    fn pool(&self, device: &ash::Device, sets: u32) -> Result<vk::DescriptorPool, String> {
        let sizes: Vec<vk::DescriptorPoolSize> = self.sizes.iter().map(|size| vk::DescriptorPoolSize { descriptor_count: size.descriptor_count * sets, ..*size }).collect();
        unsafe { device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(sets).pool_sizes(&sizes), None) }.map_err(vk_error("vkCreateDescriptorPool"))
    }
}

pub struct Renderer {
    gpu: Rc<Gpu>,
    pub eye: [u32; 2],
    targets: Vec<Target>,
    depth: Texture,
    samples: Texture,
    render_pass: vk::RenderPass,
    set_layout: SetLayout,
    layout: vk::PipelineLayout,
    pipelines: HashMap<PipelineKey, vk::Pipeline>,
    sampler: vk::Sampler,
    edge_sampler: vk::Sampler,
    pub outline_scale: f32,
    next: usize,
    last: Option<(usize, u32)>,
    /// Two timestamps a target, bracketing its drawing; None where the queue keeps no time.
    timestamps: Option<vk::QueryPool>,
    timed: [bool; RING],
    pub density: Density,
    /// How long the GPU took over the last drawing it finished.
    pub drawing: Option<std::time::Duration>,
    particle_set_layout: SetLayout,
    particle_layout: vk::PipelineLayout,
    particle_pipelines: HashMap<Blend, vk::Pipeline>,
}

/// The ratio between neighbouring densities. A window's texels move in whole steps, because each change of them
/// reaches the headset late through a second overlay (`openvr::Handoff`): texels that followed every
/// millimetre of the viewer's sway and every scatter of the drawing time would change on every frame.
const STEP: f32 = 0.9;
/// The most steps the budget lowers an image, to about half its density.
const LOWEST: u32 = 6;
/// How far under a step the wanted texels fall before it is left; it is taken again once they reach it.
const SLACK: f32 = 1.06;
/// The drawings a density's time is averaged over before the budget moves it again.
const SETTLED: u32 = 32;
/// The share of the budget a drawing one step denser must fit for the density to rise a step.
const HEADROOM: f32 = 0.85;

/// The texels each eye's image is drawn at, in steps of `STEP` down from the target's size.
pub struct Density {
    most: u32,
    /// Steps down for the viewer's distance, and further steps down for the budget.
    far: u32,
    lowered: u32,
    /// The GPU time a drawing may take, None to draw every image as dense as its distance shows.
    pub budget: Option<std::time::Duration>,
    /// The GPU's time over the drawings at this density, the last `SETTLED` of them at most, and their count.
    typical: f32,
    timed: u32,
    /// What a drawing one step denser takes of this density's time, as the last step measured it: a figure's
    /// vertices cost the same at any density, so its time falls by less than its texels.
    dearer: f32,
    /// The time the density took that the last step left, and whether that step fell, until this one's has settled.
    left: Option<(f32, bool)>,
}

impl Density {
    pub fn new(most: u32) -> Density {
        Density { most, far: 0, lowered: 0, budget: None, typical: 0.0, timed: 0, dearer: 1.0 / (STEP * STEP), left: None }
    }

    fn step(&self, steps: u32) -> f32 {
        self.most as f32 * STEP.powi(steps as i32)
    }

    /// Settles on the texels to draw where the display shows `wanted` of them: the step at or under that, kept
    /// until `wanted` falls `SLACK` under it or reaches the next, less the budget's steps.
    pub fn settle(&mut self, wanted: f32) -> u32 {
        while self.far > 0 && self.step(self.far - 1) <= wanted {
            self.far -= 1;
        }
        while self.step(self.far) > wanted * SLACK && self.step(self.far + self.lowered) > 1.0 {
            self.far += 1;
        }
        (self.step(self.far + self.lowered).round() as u32).max(1)
    }

    /// What the budget draws of the density the distance shows.
    pub fn scale(&self) -> f32 {
        STEP.powi(self.lowered as i32)
    }

    /// Takes the GPU time of a finished drawing. Once a density's time has settled, it falls a step if that is
    /// over the budget, and rises one if a drawing a step denser would fit with `HEADROOM`. One drawing counts
    /// for at most twice the budget, so a lone stall moves nothing.
    fn time(&mut self, took: std::time::Duration) {
        let Some(budget) = self.budget.map(|budget| budget.as_secs_f32()) else {
            *self = Density { far: self.far, ..Density::new(self.most) };
            return;
        };
        self.timed = (self.timed + 1).min(SETTLED);
        self.typical += (took.as_secs_f32().min(2.0 * budget) - self.typical) / self.timed as f32;
        if self.timed < SETTLED {
            return;
        }
        if let Some((before, fell)) = self.left.take() {
            let measured = if fell { before / self.typical } else { self.typical / before };
            if measured.is_finite() {
                self.dearer = measured.clamp(1.0, 1.0 / (STEP * STEP));
            }
        }
        let fell = if self.typical > budget && self.lowered < LOWEST {
            true
        } else if self.lowered > 0 && self.typical * self.dearer < HEADROOM * budget {
            false
        } else {
            return;
        };
        self.left = Some((self.typical, fell));
        self.lowered = if fell { self.lowered + 1 } else { self.lowered - 1 };
        (self.typical, self.timed) = (0.0, 0);
    }
}

pub struct Appearance {
    gpu: Rc<Gpu>,
    descriptors: vk::DescriptorPool,
    textures: Vec<Texture>,
    vertices: Buffer,
    coat_uvs: Option<Buffer>,
    indices: Buffer,
    palette: Buffer,
    joints: usize,
    materials: Buffer,
    surfaces: Buffer,
    looks: Buffer,
    untinted_cells: Buffer,
    drawn: Vec<DrawCall>,
    pub hidden: std::collections::HashSet<usize>,
    /// Seconds the figure's expressions have run, which turns a theme colour's hue at its speed.
    pub seconds: f64,
    /// The fence of the last frame drawn from these buffers, which `update` waits on before it rewrites them.
    drawing: std::cell::Cell<Option<vk::Fence>>,
    particles: Option<ParticleBuffers>,
}

/// Where a particle system's mesh and instances lie in its appearance's buffers.
struct SystemDraw {
    first: u32,
    count: u32,
    vertex_offset: i32,
    instances: u64,
    max: u32,
}

struct ParticleBuffers {
    gpu: Rc<Gpu>,
    vertices: Buffer,
    indices: Buffer,
    instances: Buffer,
    systems: Buffer,
    curves: Buffer,
    descriptors: vk::DescriptorPool,
    /// A set for untextured materials, then one per image the materials name.
    sets: Vec<vk::DescriptorSet>,
    draws: Vec<SystemDraw>,
    /// What the next frame draws: each system with a particle, solid ones first, by its set, blend and clock.
    shown: Vec<(usize, vk::DescriptorSet, Blend, f32)>,
    /// The map from the particles' world into the frame the figure is skinned into.
    space: Mat4,
}

impl Drop for ParticleBuffers {
    fn drop(&mut self) {
        for buffer in [self.vertices, self.indices, self.instances, self.systems, self.curves] {
            if buffer.0 != vk::Buffer::null() {
                self.gpu.free_buffer(buffer);
            }
        }
        unsafe { self.gpu.device.destroy_descriptor_pool(self.descriptors, None) };
    }
}

fn upload(gpu: &Gpu, bytes: &[u8], usage: vk::BufferUsageFlags) -> Result<Buffer, String> {
    let buffer = gpu.host_buffer(bytes.len() as u64, usage)?;
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.2, bytes.len()) };
    Ok(buffer)
}

/// A shader module, destroyed however the creation of the pipelines it serves ends.
struct Module<'a> {
    device: &'a ash::Device,
    module: vk::ShaderModule,
}

impl<'a> Module<'a> {
    fn new(device: &'a ash::Device, words: &[u32]) -> Result<Module<'a>, String> {
        let module = unsafe { device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None) }.map_err(vk_error("vkCreateShaderModule"))?;
        Ok(Module { device, module })
    }
}

impl Drop for Module<'_> {
    fn drop(&mut self) {
        unsafe { self.device.destroy_shader_module(self.module, None) };
    }
}

fn color_range(levels: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(levels).layer_count(vk::REMAINING_ARRAY_LAYERS)
}

fn barrier(image: vk::Image, range: vk::ImageSubresourceRange, from: vk::ImageLayout, to: vk::ImageLayout, src: vk::AccessFlags, dst: vk::AccessFlags) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default().old_layout(from).new_layout(to).src_access_mask(src).dst_access_mask(dst).image(image).subresource_range(range)
}

impl Renderer {
    /// A renderer whose target holds a window `width` metres wide from the nearest it is seen.
    pub fn for_window(gpu: Rc<Gpu>, width: f32, outline_scale: f32) -> Result<Renderer, String> {
        let texels = eye_texels(gpu.density, width, NEAREST);
        Renderer::new(gpu, [texels; 2], outline_scale)
    }

    pub fn texels(&self, width: f32, distance: f32) -> u32 {
        eye_texels(self.gpu.density, width, distance)
    }

    pub fn new(gpu: Rc<Gpu>, eye: [u32; 2], outline_scale: f32) -> Result<Renderer, String> {
        let device = &gpu.device;
        let attachments = [
            vk::AttachmentDescription::default()
                .format(FORMAT)
                .samples(SAMPLES)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
            vk::AttachmentDescription::default()
                .format(DEPTH)
                .samples(SAMPLES)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            vk::AttachmentDescription::default()
                .format(FORMAT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::DONT_CARE)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
        ];
        let color_reference = [vk::AttachmentReference::default().attachment(0).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth_reference = vk::AttachmentReference::default().attachment(1).layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let resolve_reference = [vk::AttachmentReference::default().attachment(2).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpasses = [vk::SubpassDescription::default().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&color_reference).resolve_attachments(&resolve_reference).depth_stencil_attachment(&depth_reference)];
        let render_pass = unsafe { device.create_render_pass(&vk::RenderPassCreateInfo::default().attachments(&attachments).subpasses(&subpasses), None) }.map_err(vk_error("vkCreateRenderPass"))?;

        let both = vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT;
        let bindings = [
            vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(2).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(1).stage_flags(both),
            vk::DescriptorSetLayoutBinding::default().binding(3).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(both),
            vk::DescriptorSetLayoutBinding::default().binding(4).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::VERTEX),
            vk::DescriptorSetLayoutBinding::default().binding(5).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(both),
            vk::DescriptorSetLayoutBinding::default().binding(6).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(7).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(8).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(9).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(*DECAL_PAGES as u32).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(10).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(11).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(12).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(13).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(14).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(15).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(16).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(17).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(18).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_layout = SetLayout::new(device, &bindings)?;
        let set_layouts = [set_layout.layout];
        let push_ranges = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT).size(std::mem::size_of::<Push>() as u32)];
        let layout = unsafe { device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts).push_constant_ranges(&push_ranges), None) }.map_err(vk_error("vkCreatePipelineLayout"))?;
        let sampler = |address| unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(address)
                    .address_mode_v(address)
                    .anisotropy_enable(gpu.anisotropy.is_some())
                    .max_anisotropy(gpu.anisotropy.unwrap_or(1.0).min(ANISOTROPY))
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .map_err(vk_error("vkCreateSampler"));
        let (sampler, edge_sampler) = (sampler(vk::SamplerAddressMode::REPEAT)?, sampler(vk::SamplerAddressMode::CLAMP_TO_EDGE)?);
        let particle_bindings = [
            vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(both),
            vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::VERTEX),
            vk::DescriptorSetLayoutBinding::default().binding(2).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(3).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let particle_set_layout = SetLayout::new(device, &particle_bindings)?;
        let particle_set_layouts = [particle_set_layout.layout];
        let particle_push = [vk::PushConstantRange::default().stage_flags(both).size(std::mem::size_of::<ParticlePush>() as u32)];
        let particle_layout = unsafe { device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&particle_set_layouts).push_constant_ranges(&particle_push), None) }.map_err(vk_error("vkCreatePipelineLayout"))?;
        let mut renderer = Renderer {
            depth: Texture { image: vk::Image::null(), memory: vk::DeviceMemory::null(), view: vk::ImageView::null() },
            samples: Texture { image: vk::Image::null(), memory: vk::DeviceMemory::null(), view: vk::ImageView::null() },
            gpu,
            eye,
            targets: Vec::new(),
            render_pass,
            set_layout,
            layout,
            pipelines: HashMap::new(),
            sampler,
            edge_sampler,
            outline_scale,
            next: 0,
            last: None,
            timestamps: None,
            timed: [false; RING],
            density: Density::new(eye[0].min(eye[1])),
            drawing: None,
            particle_set_layout,
            particle_layout,
            particle_pipelines: HashMap::new(),
        };
        if renderer.gpu.timestamp.is_some() {
            renderer.timestamps = Some(unsafe { renderer.gpu.device.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(2 * RING as u32), None) }.map_err(vk_error("vkCreateQueryPool"))?);
        }
        renderer.depth = renderer.attachment(DEPTH, SAMPLES, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT, vk::ImageAspectFlags::DEPTH)?;
        renderer.samples = renderer.attachment(FORMAT, SAMPLES, vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT, vk::ImageAspectFlags::COLOR)?;
        for _ in 0..RING {
            let target = renderer.target()?;
            renderer.targets.push(target);
        }
        Ok(renderer)
    }

    pub fn appearance(&mut self, model: &Model, skinned: &Skinned) -> Result<Appearance, String> {
        let looks = model.menu.as_ref().and_then(|menu| menu.looks.as_ref());
        let decals = looks.map(|looks| &looks.decals[..]).unwrap_or_default();
        let (pages, places) = decal_pages(decals);
        if pages.len() > *DECAL_PAGES {
            return Err(format!("the decals take {} pages, over {}", pages.len(), *DECAL_PAGES));
        }
        let prepassed = |material: &crate::vrm::Material| material.alpha != Alpha::Blend && material.depth_write;
        let untinted = model.menu.as_ref().and_then(|menu| menu.untinted.as_ref()).is_some_and(|untinted| !untinted.cells.is_empty());
        let key = |material: &crate::vrm::Material, features: Features, outline: bool, depth: bool| {
            let outline_unlit = outline && material.outline.as_ref().is_some_and(|outline| outline.layered || outline.lighting_mix == 0.0);
            let shading = if depth {
                Shading::default()
            } else if outline_unlit {
                Shading { outline_unlit, ..Shading::default() }
            } else {
                let features = Features { coat: features.coat && !outline, ..features };
                Shading { pages: if features.decals == 0 { 0 } else { pages.len() }, features, untinted, outline_unlit }
            };
            PipelineKey {
            blend: material.alpha == Alpha::Blend,
            cull: if outline { Cull::Front } else if material.double_sided { Cull::None } else { Cull::Back },
            depth_write: material.depth_write,
            shading,
            pass: match (outline, depth) {
                (true, _) => Pass::Cutout,
                (false, true) => Pass::Depth,
                (false, false) if prepassed(material) => Pass::Equal { cutout: matches!(material.alpha, Alpha::Cutout(_)) },
                (false, false) if matches!(material.alpha, Alpha::Cutout(_)) => Pass::Cutout,
                (false, false) => Pass::Shade,
            },
            }
        };
        let keys: Vec<PipelineKey> = skinned.draws.iter().flat_map(|draw| {
            let material = &model.materials[draw.material];
            [Some(key(material, draw.features, false, false)), prepassed(material).then(|| key(material, draw.features, false, true)), material.outline.as_ref().map(|_| key(material, draw.features, true, false))]
        }).flatten().filter(|key| !self.pipelines.contains_key(key)).collect();
        self.pipelines(&keys)?;

        let gpu = &self.gpu;
        let device = &gpu.device;
        let set_count = model.materials.len() as u32;
        let descriptors = self.set_layout.pool(device, set_count)?;
        let joints = skinned.joints();
        let material_data: Vec<MaterialData> = model.materials.iter().map(material_data).collect();
        let look_data: Vec<LookData> = std::iter::once(LookData { page: [-1; 4], rim_tint_layer: -1, ..LookData::default() }).chain(looks.into_iter().flat_map(|looks| looks.materials.iter().map(|look| look_data(look, &places, if looks.rim_tint.is_some() { looks.shades.len() as i32 } else { -1 })))).collect();
        let untinted = model.menu.as_ref().and_then(|menu| menu.untinted.as_ref());
        let untinted_cells: Vec<[f32; 8]> = match untinted.map(|untinted| &untinted.cells[..]) {
            Some(cells) if !cells.is_empty() => cells.to_vec(),
            _ => vec![[1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0]],
        };
        let mut appearance = Appearance {
            gpu: gpu.clone(),
            descriptors,
            textures: Vec::new(),
            vertices: upload(gpu, bytes_of(&skinned.vertices), vk::BufferUsageFlags::VERTEX_BUFFER)?,
            coat_uvs: if skinned.coat_uvs.is_empty() { None } else { Some(upload(gpu, bytes_of(&skinned.coat_uvs), vk::BufferUsageFlags::VERTEX_BUFFER)?) },
            indices: upload(gpu, bytes_of(&skinned.indices), vk::BufferUsageFlags::INDEX_BUFFER)?,
            palette: gpu.host_buffer((joints.max(1) * std::mem::size_of::<Mat4>()) as u64, vk::BufferUsageFlags::STORAGE_BUFFER)?,
            joints,
            materials: upload(gpu, bytes_of(&material_data), vk::BufferUsageFlags::STORAGE_BUFFER)?,
            surfaces: upload(gpu, bytes_of(&skinned.surfaces), vk::BufferUsageFlags::STORAGE_BUFFER)?,
            looks: upload(gpu, bytes_of(&look_data), vk::BufferUsageFlags::STORAGE_BUFFER)?,
            untinted_cells: upload(gpu, bytes_of(&untinted_cells), vk::BufferUsageFlags::STORAGE_BUFFER)?,
            drawn: Vec::new(),
            hidden: Default::default(),
            seconds: 0.0,
            drawing: Default::default(),
            particles: None,
        };
        for image in &model.images {
            appearance.textures.push(texture(gpu, image.width, image.height, &image.rgba, FORMAT)?);
        }
        let hue_mask = if let Some(mask) = model.menu.as_ref().and_then(|menu| menu.hue_mask.as_ref()) {
            let index = appearance.textures.len();
            appearance.textures.push(texture(gpu, mask.width, mask.height, &mask.rgba, vk::Format::R8G8B8A8_UNORM)?);
            Some(index)
        } else { None };
        let untinted = if let Some(untinted) = untinted {
            let index = appearance.textures.len();
            appearance.textures.push(texture(gpu, untinted.image.width, untinted.image.height, &untinted.image.rgba, FORMAT)?);
            Some(index)
        } else { None };
        let mut array = |layers: &[&crate::vrm::Image], empty: [u8; 4]| -> Result<usize, String> {
            appearance.textures.push(match layers {
                [] => layered(gpu, 1, 1, 1, &empty, FORMAT, vk::ImageViewType::TYPE_2D_ARRAY)?,
                layers => layered(gpu, layers[0].width, layers[0].height, layers.len() as u32, &layers.iter().flat_map(|layer| layer.rgba.iter().copied()).collect::<Vec<_>>(), FORMAT, vk::ImageViewType::TYPE_2D_ARRAY)?,
            });
            Ok(appearance.textures.len() - 1)
        };
        let mut decal_textures = Vec::new();
        for page in &pages {
            decal_textures.push(array(&page.iter().map(|&at| &decals[at]).collect::<Vec<_>>(), [0; 4])?);
        }
        let no_decal = array(&[], [0; 4])?;
        decal_textures.resize(*DECAL_PAGES, no_decal);
        let matcaps = array(&looks.into_iter().flat_map(|looks| &looks.matcaps).collect::<Vec<_>>(), [255; 4])?;
        let toon_layers = array(&looks.into_iter().flat_map(|looks| looks.shades.iter().chain(&looks.rim_tint)).collect::<Vec<_>>(), [255; 4])?;
        let coat_maps = {
            let maps = looks.map(|looks| &looks.clearcoat_maps[..]).unwrap_or_default();
            let (width, height) = maps.iter().fold((1, 1), |(width, height), map| (map.width.max(width), map.height.max(height)));
            let mut rgba = Vec::new();
            for map in maps {
                if (map.width, map.height) == (width, height) { rgba.extend_from_slice(&map.rgba) } else { rgba.extend(resized(map, width, height)) }
            }
            appearance.textures.push(layered(gpu, width, height, maps.len().max(1) as u32, if maps.is_empty() { &[255; 4] } else { &rgba }, vk::Format::R8G8B8A8_UNORM, vk::ImageViewType::TYPE_2D_ARRAY)?);
            appearance.textures.len() - 1
        };
        let mut single = |image: Option<&crate::vrm::Image>, format: vk::Format| -> Result<Option<usize>, String> {
            let Some(image) = image else { return Ok(None) };
            appearance.textures.push(texture(gpu, image.width, image.height, &image.rgba, format)?);
            Ok(Some(appearance.textures.len() - 1))
        };
        let look_index = single(looks.and_then(|looks| looks.index.as_ref()).map(|index| crate::vrm::Image { width: index.width, height: index.height, rgba: index.rgba.iter().step_by(4).copied().collect() }).as_ref(), vk::Format::R8_UNORM)?;
        let matcap_masks = single(looks.and_then(|looks| looks.matcap_masks.as_ref()), vk::Format::R8G8B8A8_UNORM)?;
        let emission = single(looks.and_then(|looks| looks.emission.as_ref()), FORMAT)?;
        let white = appearance.textures.len();
        appearance.textures.push(texture(gpu, 1, 1, &[255; 4], FORMAT)?);
        let black = appearance.textures.len();
        appearance.textures.push(texture(gpu, 1, 1, &[0, 0, 0, 255], vk::Format::R8G8B8A8_UNORM)?);
        let layouts = vec![self.set_layout.layout; model.materials.len()];
        let sets = unsafe { device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(descriptors).set_layouts(&layouts)) }.map_err(vk_error("vkAllocateDescriptorSets"))?;
        for (material, &set) in model.materials.iter().zip(&sets) {
            let image = |index: Option<usize>| [vk::DescriptorImageInfo::default().image_view(appearance.textures[index.unwrap_or(white)].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let (base, shade, width) = (image(material.base_image), image(material.shade_image), image(material.outline.as_ref().and_then(|outline| outline.width_image)));
            let mask = image(hue_mask);
            let untinted = image(untinted);
            let decal: Vec<_> = decal_textures.iter().map(|&texture| vk::DescriptorImageInfo::default().image_view(appearance.textures[texture].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)).collect();
            let (index, matcap, masks, glow, coat, toon) = (image(Some(look_index.unwrap_or(black))), image(Some(matcaps)), image(matcap_masks), image(Some(emission.unwrap_or(black))), image(Some(coat_maps)), image(Some(toon_layers)));
            let surfaces = [vk::DescriptorBufferInfo::default().buffer(appearance.surfaces.0).range(vk::WHOLE_SIZE)];
            let looks = [vk::DescriptorBufferInfo::default().buffer(appearance.looks.0).range(vk::WHOLE_SIZE)];
            let cells = [vk::DescriptorBufferInfo::default().buffer(appearance.untinted_cells.0).range(vk::WHOLE_SIZE)];
            let sampler = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
            let edge_sampler = [vk::DescriptorImageInfo::default().sampler(self.edge_sampler)];
            let palette = [vk::DescriptorBufferInfo::default().buffer(appearance.palette.0).range(vk::WHOLE_SIZE)];
            let materials = [vk::DescriptorBufferInfo::default().buffer(appearance.materials.0).range(vk::WHOLE_SIZE)];
            let writes = [
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&base),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&shade),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(2).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sampler),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(3).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&width),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(4).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&palette),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(5).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&materials),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(6).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&mask),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(7).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&surfaces),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(8).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&untinted),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(9).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&decal),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(10).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&index),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(11).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&looks),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(12).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&matcap),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(13).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&masks),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(14).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&glow),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(15).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&edge_sampler),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(16).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&cells),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(17).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&coat),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(18).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&toon),
            ];
            unsafe { device.update_descriptor_sets(&writes, &[]) };
        }
        appearance.drawn = skinned
            .draws
            .iter()
            .flat_map(|draw| {
                let material = &model.materials[draw.material];
                let call = |outline: bool| DrawCall {
                    first: draw.first,
                    count: draw.count,
                    set: sets[draw.material],
                    depth: (!outline && prepassed(material)).then(|| self.pipelines[&key(material, draw.features, false, true)]),
                    pipeline: self.pipelines[&key(material, draw.features, outline, false)],
                    material: draw.material as u32,
                    outline,
                    switched: draw.switched,
                };
                [Some(call(false)), material.outline.as_ref().map(|_| call(true))]
            })
            .flatten()
            .collect();
        if !model.particles.systems.is_empty() {
            let blends: Vec<Blend> = model.particles.systems.iter().flat_map(|system| system.renderer.materials.iter().map(|material| material.blend)).filter(|blend| !self.particle_pipelines.contains_key(blend)).collect();
            self.particle_pipelines(&blends)?;
            appearance.particles = Some(self.particle_buffers(model, &mut appearance.textures)?);
        }
        Ok(appearance)
    }

    fn particle_buffers(&self, model: &Model, textures: &mut Vec<Texture>) -> Result<ParticleBuffers, String> {
        let gpu = &self.gpu;
        let device = &gpu.device;
        let definition = &model.particles;
        let mut vertices: Vec<[f32; 5]> = vec![[-0.5, 0.5, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 1.0, 0.0], [0.5, -0.5, 0.0, 1.0, 1.0], [-0.5, -0.5, 0.0, 0.0, 1.0]];
        let mut indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];
        let mut meshes = Vec::new();
        for mesh in &definition.meshes {
            meshes.push((indices.len() as u32, mesh.indices.len() as u32, vertices.len() as i32));
            vertices.extend(mesh.positions.iter().zip(&mesh.uvs).map(|(p, uv)| [p.x, p.y, p.z, uv.x, uv.y]));
            indices.extend(&mesh.indices);
        }
        let mut instances = 0u64;
        let draws: Vec<SystemDraw> = definition.systems.iter().map(|system| {
            let (first, count, vertex_offset) = match system.renderer.mode {
                Mode::Mesh(mesh) => meshes[mesh],
                _ => (0, 6, 0),
            };
            let draw = SystemDraw { first, count, vertex_offset, instances, max: system.max as u32 };
            instances += (system.max * std::mem::size_of::<Instance>()) as u64;
            draw
        }).collect();
        let curves: Vec<[f32; 4]> = definition.systems.iter().flat_map(|system| system.curves()).collect();
        let unborn: Vec<Instance> = definition.systems.iter().flat_map(|system| vec![crate::vrm::particles::EMPTY; system.max]).collect();
        let first_texture = textures.len();
        textures.push(texture(gpu, 1, 1, &[255; 4], FORMAT)?);
        for image in &definition.images {
            textures.push(texture(gpu, image.width, image.height, &image.rgba, FORMAT)?);
        }
        let set_count = 1 + definition.images.len() as u32;
        let none = (vk::Buffer::null(), vk::DeviceMemory::null(), std::ptr::null_mut());
        let mut buffers = ParticleBuffers { gpu: gpu.clone(), vertices: none, indices: none, instances: none, systems: none, curves: none, descriptors: vk::DescriptorPool::null(), sets: Vec::new(), draws, shown: Vec::new(), space: Mat4::IDENTITY };
        buffers.vertices = upload(gpu, bytes_of(&vertices), vk::BufferUsageFlags::VERTEX_BUFFER)?;
        buffers.indices = upload(gpu, bytes_of(&indices), vk::BufferUsageFlags::INDEX_BUFFER)?;
        buffers.instances = upload(gpu, bytes_of(&unborn), vk::BufferUsageFlags::VERTEX_BUFFER)?;
        buffers.systems = gpu.host_buffer((definition.systems.len() * std::mem::size_of::<SystemData>()) as u64, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        buffers.curves = upload(gpu, bytes_of(&curves), vk::BufferUsageFlags::STORAGE_BUFFER)?;
        buffers.descriptors = self.particle_set_layout.pool(device, set_count)?;
        let layouts = vec![self.particle_set_layout.layout; set_count as usize];
        buffers.sets = unsafe { device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(buffers.descriptors).set_layouts(&layouts)) }.map_err(vk_error("vkAllocateDescriptorSets"))?;
        for (at, &set) in buffers.sets.iter().enumerate() {
            let systems = [vk::DescriptorBufferInfo::default().buffer(buffers.systems.0).range(vk::WHOLE_SIZE)];
            let curves = [vk::DescriptorBufferInfo::default().buffer(buffers.curves.0).range(vk::WHOLE_SIZE)];
            let image = [vk::DescriptorImageInfo::default().image_view(textures[first_texture + at].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
            let writes = [
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&systems),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&curves),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(2).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&image),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(3).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sampler),
            ];
            unsafe { device.update_descriptor_sets(&writes, &[]) };
        }
        Ok(buffers)
    }

    /// Makes a pipeline for each blend of `blends` not yet made, keeping each as it is made so the renderer frees it
    /// whatever fails after.
    fn particle_pipelines(&mut self, blends: &[Blend]) -> Result<(), String> {
        if blends.is_empty() {
            return Ok(());
        }
        let gpu = self.gpu.clone();
        let no_constants = naga::back::PipelineConstants::default();
        let (vertex, fragment) = (Module::new(&gpu.device, &PARTICLE_SHADER.spirv(naga::ShaderStage::Vertex, "vertex", &no_constants)?)?, Module::new(&gpu.device, &PARTICLE_SHADER.spirv(naga::ShaderStage::Fragment, "fragment", &no_constants)?)?);
        let bindings = [
            vk::VertexInputBindingDescription::default().binding(0).stride(20).input_rate(vk::VertexInputRate::VERTEX),
            vk::VertexInputBindingDescription::default().binding(1).stride(std::mem::size_of::<Instance>() as u32).input_rate(vk::VertexInputRate::INSTANCE),
        ];
        let mut attributes = vec![
            vk::VertexInputAttributeDescription::default().location(0).binding(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
            vk::VertexInputAttributeDescription::default().location(1).binding(0).format(vk::Format::R32G32_SFLOAT).offset(12),
        ];
        attributes.extend((0..5).map(|at| vk::VertexInputAttributeDescription::default().location(2 + at).binding(1).format(vk::Format::R32G32B32A32_SFLOAT).offset(16 * at)));
        let input = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&bindings).vertex_attribute_descriptions(&attributes);
        let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vertex.module).name(c"vertex"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fragment.module).name(c"fragment"),
        ];
        use vk::BlendFactor as F;
        for &blend in blends {
            if self.particle_pipelines.contains_key(&blend) {
                continue;
            }
            let ([color_from, color_to], [alpha_from, alpha_to]) = match blend {
                Blend::Opaque | Blend::Cutout => ([F::ONE, F::ZERO], [F::ONE, F::ZERO]),
                Blend::Alpha | Blend::Premultiplied => ([F::ONE, F::ONE_MINUS_SRC_ALPHA], [F::ONE, F::ONE_MINUS_SRC_ALPHA]),
                Blend::Add => ([F::ONE, F::ONE], [F::ONE, F::ONE]),
                Blend::Multiply => ([F::DST_COLOR, F::ZERO], [F::ZERO, F::ONE]),
            };
            let attachment = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(!blend.solid())
                .src_color_blend_factor(color_from)
                .dst_color_blend_factor(color_to)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(alpha_from)
                .dst_alpha_blend_factor(alpha_to)
                .alpha_blend_op(vk::BlendOp::ADD)
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
            let color = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachment);
            let depth = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(blend.solid()).depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
            let pipeline = self.pipeline(self.particle_layout, &stages, &input, &raster, &depth, &color)?;
            self.particle_pipelines.insert(blend, pipeline);
        }
        Ok(())
    }

    /// Makes a pipeline for each key of `keys` not yet made, keeping each as it is made so the renderer frees it
    /// whatever fails after.
    fn pipelines(&mut self, keys: &[PipelineKey]) -> Result<(), String> {
        let gpu = self.gpu.clone();
        let vertex = |key: &PipelineKey| (naga::ShaderStage::Vertex, if key.shading.features.coat { c"coated_vertex" } else { c"vertex" }, Shading::default());
        let shader = |key: &PipelineKey| (naga::ShaderStage::Fragment, match key.pass {
            Pass::Equal { cutout: false } | Pass::Shade => if key.shading.features.coat { c"coated_fragment" } else { c"fragment" },
            Pass::Depth => c"depth",
            Pass::Equal { cutout: true } | Pass::Cutout => if key.shading.features.coat { c"coated_cutout" } else { c"cutout" },
        }, key.shading);
        let mut modules = HashMap::new();
        for key in keys {
            for (stage, entry, shading) in [vertex(key), shader(key)] {
                if let std::collections::hash_map::Entry::Vacant(unmade) = modules.entry((entry, shading)) {
                    unmade.insert(Module::new(&gpu.device, &AVATAR_SHADER.spirv(stage, entry.to_str().unwrap(), &shading.constants())?)?);
                }
            }
        }
        let vertex_bindings = [
            vk::VertexInputBindingDescription::default().binding(0).stride(std::mem::size_of::<Vertex>() as u32).input_rate(vk::VertexInputRate::VERTEX),
            vk::VertexInputBindingDescription::default().binding(1).stride(std::mem::size_of::<[f32; 2]>() as u32).input_rate(vk::VertexInputRate::VERTEX),
        ];
        let vertex_attributes = [
            vk::VertexInputAttributeDescription::default().location(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
            vk::VertexInputAttributeDescription::default().location(1).format(vk::Format::R32G32B32_SFLOAT).offset(12),
            vk::VertexInputAttributeDescription::default().location(2).format(vk::Format::R32G32_SFLOAT).offset(24),
            vk::VertexInputAttributeDescription::default().location(3).format(vk::Format::R16G16B16A16_UINT).offset(32),
            vk::VertexInputAttributeDescription::default().location(4).format(vk::Format::R32G32B32A32_SFLOAT).offset(40),
            vk::VertexInputAttributeDescription::default().location(5).format(vk::Format::R32_UINT).offset(56),
            vk::VertexInputAttributeDescription::default().location(6).binding(1).format(vk::Format::R32G32_SFLOAT).offset(0),
        ];
        let plain = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&vertex_bindings[..1]).vertex_attribute_descriptions(&vertex_attributes[..6]);
        let coated = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&vertex_bindings).vertex_attribute_descriptions(&vertex_attributes);
        for &key in keys {
            if self.pipelines.contains_key(&key) {
                continue;
            }
            let cull = match key.cull {
                Cull::None => vk::CullModeFlags::NONE,
                Cull::Back => vk::CullModeFlags::BACK,
                Cull::Front => vk::CullModeFlags::FRONT,
            };
            let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(cull).front_face(vk::FrontFace::COUNTER_CLOCKWISE).line_width(1.0);
            let (compare, write) = match key.pass {
                Pass::Equal { .. } => (vk::CompareOp::EQUAL, false),
                _ => (vk::CompareOp::LESS_OR_EQUAL, key.depth_write),
            };
            let depth = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(write).depth_compare_op(compare);
            let attachment = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(key.blend)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD)
                .color_write_mask(if key.pass == Pass::Depth { vk::ColorComponentFlags::empty() } else { vk::ColorComponentFlags::RGBA })];
            let color = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachment);
            let stages = [
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(modules[&(vertex(&key).1, vertex(&key).2)].module).name(vertex(&key).1),
                vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(modules[&(shader(&key).1, shader(&key).2)].module).name(shader(&key).1),
            ];
            let pipeline = self.pipeline(self.layout, &stages, if key.shading.features.coat { &coated } else { &plain }, &raster, &depth, &color)?;
            self.pipelines.insert(key, pipeline);
        }
        Ok(())
    }

    /// A pipeline in the render pass with the state every one shares: triangle lists, a dynamic viewport and scissor,
    /// and the targets' samples.
    fn pipeline(&self, layout: vk::PipelineLayout, stages: &[vk::PipelineShaderStageCreateInfo], input: &vk::PipelineVertexInputStateCreateInfo, raster: &vk::PipelineRasterizationStateCreateInfo, depth: &vk::PipelineDepthStencilStateCreateInfo, color: &vk::PipelineColorBlendStateCreateInfo) -> Result<vk::Pipeline, String> {
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(SAMPLES);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(stages)
            .vertex_input_state(input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(raster)
            .multisample_state(&multisample)
            .depth_stencil_state(depth)
            .color_blend_state(color)
            .dynamic_state(&dynamic)
            .layout(layout)
            .render_pass(self.render_pass);
        let created = unsafe { self.gpu.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) }.map_err(|(_, error)| format!("vkCreateGraphicsPipelines: {error}"))?;
        Ok(created[0])
    }

    fn attachment(&self, format: vk::Format, samples: vk::SampleCountFlags, usage: vk::ImageUsageFlags, aspect: vk::ImageAspectFlags) -> Result<Texture, String> {
        let (image, memory) = self.gpu.image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D { width: self.eye[0] * 2, height: self.eye[1], depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(samples)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .initial_layout(vk::ImageLayout::UNDEFINED),
        )?;
        let range = vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1);
        let view = unsafe { self.gpu.device.create_image_view(&vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(format).subresource_range(range), None) }.map_err(vk_error("vkCreateImageView"))?;
        Ok(Texture { image, memory, view })
    }

    fn target(&self) -> Result<Target, String> {
        let device = &self.gpu.device;
        let texture = self.attachment(FORMAT, vk::SampleCountFlags::TYPE_1, vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED, vk::ImageAspectFlags::COLOR)?;
        let views = [self.samples.view, self.depth.view, texture.view];
        let framebuffer = unsafe { device.create_framebuffer(&vk::FramebufferCreateInfo::default().render_pass(self.render_pass).attachments(&views).width(self.eye[0] * 2).height(self.eye[1]).layers(1), None) }.map_err(vk_error("vkCreateFramebuffer"))?;
        let commands = unsafe { device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(self.gpu.pool).command_buffer_count(1)) }.map_err(vk_error("vkAllocateCommandBuffers"))?[0];
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None) }.map_err(vk_error("vkCreateFence"))?;
        Ok(Target { texture, framebuffer, commands, fence })
    }

    /// Reads the GPU time of the drawing `target` finished.
    fn time_drawing(&mut self, target: usize) -> Result<(), String> {
        let (Some(pool), Some((tick, mask))) = (self.timestamps, self.gpu.timestamp) else { return Ok(()) };
        if !std::mem::take(&mut self.timed[target]) {
            return Ok(());
        }
        let mut stamps = [0u64; 2];
        unsafe { self.gpu.device.get_query_pool_results(pool, 2 * target as u32, &mut stamps, vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT) }.map_err(vk_error("vkGetQueryPoolResults"))?;
        let took = std::time::Duration::from_nanos(((stamps[1].wrapping_sub(stamps[0]) & mask) as f64 * tick as f64) as u64);
        self.drawing = Some(took);
        self.density.time(took);
        Ok(())
    }

    /// Draws each eye's image `texels` square, packed from the target's top left; the overlay shows that part through `Overlay::part`.
    pub fn render(&mut self, appearance: &Appearance, eyes: [Option<Mat4>; 2], space: Mat4, texels: u32) -> Result<Drawing, String> {
        let device = &self.gpu.device;
        let target = &self.targets[self.next];
        unsafe { device.wait_for_fences(&[target.fence], true, u64::MAX) }.map_err(vk_error("vkWaitForFences"))?;
        self.time_drawing(self.next)?;
        let device = &self.gpu.device;
        let target = &self.targets[self.next];
        let [width, height] = self.eye;
        assert!(texels <= width && texels <= height, "{texels} texels per eye overflow the {width}x{height} target");
        let clear = [vk::ClearValue { color: vk::ClearColorValue { float32: [0.0; 4] } }, vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } }];
        unsafe {
            device.reset_command_buffer(target.commands, vk::CommandBufferResetFlags::empty()).map_err(vk_error("vkResetCommandBuffer"))?;
            device.begin_command_buffer(target.commands, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(vk_error("vkBeginCommandBuffer"))?;
            if let Some(pool) = self.timestamps {
                device.cmd_reset_query_pool(target.commands, pool, 2 * self.next as u32, 2);
                device.cmd_write_timestamp(target.commands, vk::PipelineStageFlags::TOP_OF_PIPE, pool, 2 * self.next as u32);
            }
            device.cmd_begin_render_pass(
                target.commands,
                &vk::RenderPassBeginInfo::default().render_pass(self.render_pass).framebuffer(target.framebuffer).render_area(vk::Rect2D { offset: vk::Offset2D::default(), extent: vk::Extent2D { width: texels * 2, height: texels } }).clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            {
                for (side, projection) in eyes.iter().enumerate() {
                    let Some(projection) = projection else { continue };
                    let view_projection = *projection * space;
                    let x = (side as u32 * texels) as f32;
                    device.cmd_set_viewport(target.commands, 0, &[vk::Viewport { x, y: 0.0, width: texels as f32, height: texels as f32, min_depth: 0.0, max_depth: 1.0 }]);
                    device.cmd_set_scissor(target.commands, 0, &[vk::Rect2D { offset: vk::Offset2D { x: x as i32, y: 0 }, extent: vk::Extent2D { width: texels, height: texels } }]);
                    match appearance.coat_uvs {
                        Some(coat_uvs) => device.cmd_bind_vertex_buffers(target.commands, 0, &[appearance.vertices.0, coat_uvs.0], &[0, 0]),
                        None => device.cmd_bind_vertex_buffers(target.commands, 0, &[appearance.vertices.0], &[0]),
                    }
                    device.cmd_bind_index_buffer(target.commands, appearance.indices.0, 0, vk::IndexType::UINT32);
                    let shown = || appearance.drawn.iter().filter(|drawn| drawn.switched.is_none_or(|node| !appearance.hidden.contains(&node)));
                    let depths = shown().filter_map(|drawn| Some((drawn, drawn.depth?)));
                    for (drawn, pipeline) in depths.chain(shown().map(|drawn| (drawn, drawn.pipeline))) {
                        let push = Push { view_projection: view_projection.to_cols_array(), camera: camera(view_projection).to_array(), material: drawn.material, outline: drawn.outline as u32, outline_scale: self.outline_scale, focal: projection.y_axis.y, seconds: (appearance.seconds % THEME_WRAP) as f32 };
                        let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                        device.cmd_bind_pipeline(target.commands, vk::PipelineBindPoint::GRAPHICS, pipeline);
                        device.cmd_bind_descriptor_sets(target.commands, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[drawn.set], &[]);
                        device.cmd_push_constants(target.commands, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                        device.cmd_draw_indexed(target.commands, drawn.count, 1, drawn.first, 0, 0);
                    }
                    let Some(particles) = &appearance.particles else { continue };
                    let view_projection = view_projection * particles.space;
                    let (right, up, focal) = view_axes(view_projection);
                    device.cmd_bind_index_buffer(target.commands, particles.indices.0, 0, vk::IndexType::UINT32);
                    for &(system, set, blend, now) in &particles.shown {
                        let draw = &particles.draws[system];
                        let push = ParticlePush { view_projection: view_projection.to_cols_array(), eye: camera(view_projection).to_array(), now, right: right.to_array(), system: system as u32, up: up.to_array(), focal };
                        let bytes = std::slice::from_raw_parts(&push as *const ParticlePush as *const u8, std::mem::size_of::<ParticlePush>());
                        device.cmd_bind_pipeline(target.commands, vk::PipelineBindPoint::GRAPHICS, self.particle_pipelines[&blend]);
                        device.cmd_bind_descriptor_sets(target.commands, vk::PipelineBindPoint::GRAPHICS, self.particle_layout, 0, &[set], &[]);
                        device.cmd_push_constants(target.commands, self.particle_layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                        device.cmd_bind_vertex_buffers(target.commands, 0, &[particles.vertices.0, particles.instances.0], &[0, draw.instances]);
                        device.cmd_draw_indexed(target.commands, draw.count, draw.max, draw.first, draw.vertex_offset, 0);
                    }
                }
            }
            device.cmd_end_render_pass(target.commands);
            if let Some(pool) = self.timestamps {
                device.cmd_write_timestamp(target.commands, vk::PipelineStageFlags::BOTTOM_OF_PIPE, pool, 2 * self.next as u32 + 1);
            }
            device.end_command_buffer(target.commands).map_err(vk_error("vkEndCommandBuffer"))?;
            let buffers = [target.commands];
            device.reset_fences(&[target.fence]).map_err(vk_error("vkResetFences"))?;
            device.queue_submit(self.gpu.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], target.fence).map_err(vk_error("vkQueueSubmit"))?;
        }
        appearance.drawing.set(Some(target.fence));
        self.timed[self.next] = self.timestamps.is_some();
        self.last = Some((self.next, texels));
        self.next = (self.next + 1) % RING;
        Ok(Drawing { gpu: self.gpu.clone(), fence: target.fence, texture: self.gpu.texture_data(target.texture.image, width * 2, height, FORMAT) })
    }

    /// The last frame's two eye images side by side, at the size it drew them.
    pub fn read(&self) -> Result<Vec<u8>, String> {
        let [width, height] = self.eye;
        let (target, texels) = self.last.ok_or("nothing rendered")?;
        let whole = self.gpu.read(self.targets[target].texture.image, width * 2, height)?;
        Ok(whole.chunks_exact(width as usize * 8).take(texels as usize).flat_map(|row| &row[..texels as usize * 8]).copied().collect())
    }
}

fn bytes_of<T: Copy>(values: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values)) }
}

fn material_data(material: &crate::vrm::Material) -> MaterialData {
    let outline = material.outline.as_ref();
    MaterialData {
        base: material.base,
        shade: material.shade,
        blend: (material.alpha == Alpha::Blend) as u8 as f32,
        shading_shift: material.shading_shift,
        shading_toony: material.shading_toony,
        cutoff: match material.alpha {
            Alpha::Cutout(cutoff) => cutoff,
            _ => 0.0,
        },
        equalization: material.equalization,
        outline_color: outline.map_or([0.0; 3], |outline| outline.color),
        outline_lighting_mix: outline.map_or(0.0, |outline| outline.lighting_mix),
        outline_width: outline.map_or(0.0, |outline| outline.factor),
        outline_screen: outline.is_some_and(|outline| outline.width == OutlineWidth::Screen) as u8 as f32,
        outline_layered: outline.is_some_and(|outline| outline.layered) as u8 as f32,
        padding: 0.0,
    }
}

impl Appearance {
    fn settled(&self) {
        if let Some(fence) = self.drawing.take() {
            unsafe { self.gpu.device.wait_for_fences(&[fence], true, u64::MAX) }.expect("vkWaitForFences");
        }
    }

    pub fn update(&mut self, palette: &[Mat4], skinned: &Skinned, changed: &[u32]) {
        assert_eq!(palette.len(), self.joints, "the palette has one matrix per joint");
        self.settled();
        let bytes = bytes_of(palette);
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.palette.2, bytes.len()) };
        let bytes = bytes_of(&skinned.surfaces);
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.surfaces.2, bytes.len()) };
        let target = self.vertices.2 as *mut Vertex;
        for &vertex in changed {
            unsafe { target.add(vertex as usize).write(skinned.vertices[vertex as usize]) };
        }
    }
}

impl Appearance {
    /// Writes the particles born or cleared since the last call and each system's state now; `skin` places the figure
    /// as the palette's placement does.
    pub fn place_particles(&mut self, particles: &mut Particles, skin: Mat4) {
        self.settled();
        let Some(buffers) = &mut self.particles else { return };
        let instances = buffers.instances.2;
        let draws = &buffers.draws;
        particles.written(|system, slot, instance| unsafe { (instances.add(draws[system].instances as usize) as *mut Instance).add(slot).write(*instance) });
        let mut shown = Vec::new();
        for at in 0..particles.len() {
            let system = particles.system(at);
            // A system that draws nothing, one without a material among them, needs no data on the GPU.
            if !system.renderer.draw {
                continue;
            }
            let material = particles.material(at);
            let renderer = &system.renderer;
            let data = SystemData {
                emitter: particles.emitter(at).to_cols_array(),
                tint: material.color.to_array(),
                emission: material.emission.to_array(),
                cutoff: if material.blend == Blend::Cutout { material.cutoff } else { 0.0 },
                uv: material.uv,
                pivot: renderer.pivot.to_array(),
                length_scale: renderer.length,
                speed_ranges: system.speed_ranges(),
                velocity_scale: renderer.velocity_scale,
                noise_frequency: system.noise_frequency(),
                damping: system.damping(),
                flags: system.flags(),
                mode: match renderer.mode { Mode::Billboard => 0, Mode::Stretched => 1, Mode::Horizontal => 2, Mode::Vertical => 3, Mode::Mesh(_) => 4 },
                align: renderer.align as u32,
                blend: material.blend as u32,
                max_size: renderer.max_size,
            };
            unsafe { (buffers.systems.2 as *mut SystemData).add(at).write(data) };
            if particles.shown(at) {
                shown.push((at, buffers.sets[material.image.map_or(0, |image| image + 1)], material.blend, particles.now(at)));
            }
        }
        shown.sort_by_key(|&(_, _, blend, _)| !blend.solid());
        buffers.shown = shown;
        buffers.space = skin * particles.to_model();
    }
}

fn destroy(gpu: &Gpu, texture: &Texture) {
    unsafe {
        gpu.device.destroy_image_view(texture.view, None);
        gpu.device.destroy_image(texture.image, None);
    }
    gpu.free_memory(texture.memory);
}

impl Drop for Appearance {
    fn drop(&mut self) {
        let device = &self.gpu.device;
        unsafe {
            let _ = device.device_wait_idle();
            for buffer in [self.vertices, self.indices, self.palette, self.materials, self.surfaces, self.looks, self.untinted_cells].into_iter().chain(self.coat_uvs) {
                self.gpu.free_buffer(buffer);
            }
            for texture in &self.textures {
                destroy(&self.gpu, texture);
            }
            device.destroy_descriptor_pool(self.descriptors, None);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let device = &self.gpu.device;
        unsafe {
            let _ = device.device_wait_idle();
            for target in &self.targets {
                device.destroy_fence(target.fence, None);
                device.destroy_framebuffer(target.framebuffer, None);
                device.free_command_buffers(self.gpu.pool, &[target.commands]);
                destroy(&self.gpu, &target.texture);
            }
            destroy(&self.gpu, &self.depth);
            destroy(&self.gpu, &self.samples);
            if let Some(pool) = self.timestamps {
                device.destroy_query_pool(pool, None);
            }
            for &pipeline in self.pipelines.values().chain(self.particle_pipelines.values()) {
                device.destroy_pipeline(pipeline, None);
            }
            device.destroy_pipeline_layout(self.particle_layout, None);
            device.destroy_descriptor_set_layout(self.particle_set_layout.layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_sampler(self.edge_sampler, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.set_layout.layout, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

fn resized(map: &crate::vrm::Image, width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_raw(map.width, map.height, map.rgba.clone()).expect("an image holds its pixels");
    image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle).into_raw()
}

fn texture(gpu: &Gpu, width: u32, height: u32, rgba: &[u8], format: vk::Format) -> Result<Texture, String> {
    layered(gpu, width, height, 1, rgba, format, vk::ImageViewType::TYPE_2D)
}

fn layered(gpu: &Gpu, width: u32, height: u32, layers: u32, rgba: &[u8], format: vk::Format, view: vk::ImageViewType) -> Result<Texture, String> {
    let device = &gpu.device;
    let levels = 32 - width.max(height).leading_zeros();
    let (image, memory) = gpu.image(
        &vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(levels)
            .array_layers(layers)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
            .initial_layout(vk::ImageLayout::UNDEFINED),
    )?;
    let staging = gpu.host_buffer(rgba.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC)?;
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), staging.2, rgba.len()) };
    gpu.once(|commands| unsafe {
        let all = color_range(levels);
        device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier(image, all, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)]);
        let region = vk::BufferImageCopy::default().image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(layers)).image_extent(vk::Extent3D { width, height, depth: 1 });
        device.cmd_copy_buffer_to_image(commands, staging.0, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
        let (mut w, mut h) = (width as i32, height as i32);
        for level in 1..levels {
            let source = vk::ImageSubresourceRange { base_mip_level: level - 1, level_count: 1, ..color_range(1) };
            device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier(image, source, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_READ)]);
            let layers = |mip_level| vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level, base_array_layer: 0, layer_count: layers };
            let (next_w, next_h) = ((w / 2).max(1), (h / 2).max(1));
            let blit = vk::ImageBlit::default()
                .src_subresource(layers(level - 1))
                .src_offsets([vk::Offset3D::default(), vk::Offset3D { x: w, y: h, z: 1 }])
                .dst_subresource(layers(level))
                .dst_offsets([vk::Offset3D::default(), vk::Offset3D { x: next_w, y: next_h, z: 1 }]);
            device.cmd_blit_image(commands, image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[blit], vk::Filter::LINEAR);
            device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], &[barrier(image, source, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::SHADER_READ)]);
            (w, h) = (next_w, next_h);
        }
        let last = vk::ImageSubresourceRange { base_mip_level: levels - 1, level_count: 1, ..color_range(1) };
        device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], &[barrier(image, last, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::SHADER_READ)]);
    })?;
    gpu.free_buffer(staging);
    let view = unsafe { device.create_image_view(&vk::ImageViewCreateInfo::default().image(image).view_type(view).format(format).subresource_range(color_range(levels)), None) }.map_err(vk_error("vkCreateImageView"))?;
    Ok(Texture { image, memory, view })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::expressions::DecalBlend;
    use crate::vrm::{tests::figure, Version};

    const COVERAGE: f32 = 0.95;
    const COLOR: f32 = 12.0;

    #[test]
    fn the_window_corners_land_on_the_eye_image_corners() {
        let eye = Vec3::new(0.03, 0.1, 0.5);
        let projection = eye_projection(eye, 0.2, 0.2).unwrap();
        for (corner, expected) in [(Vec3::new(-0.2, 0.2, 0.0), [-1.0, -1.0]), (Vec3::new(0.2, -0.2, 0.0), [1.0, 1.0])] {
            let clip = projection * corner.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!((ndc.x - expected[0]).abs() < 1e-5 && (ndc.y - expected[1]).abs() < 1e-5, "{corner} → {ndc}");
            assert!((0.0..1.0).contains(&ndc.z));
        }
        assert!(eye_projection(Vec3::new(0.0, 0.0, 0.01), 0.2, 0.2).is_none(), "an eye at the window draws nothing");
    }

    fn draw(model: &Model, pose: &crate::vrm::Pose, eye: [u32; 2], eyes: [Option<Mat4>; 2], shift: Vec3) -> Vec<u8> {
        draw_at(model, pose, eye, eyes, shift, crate::HEIGHT, crate::FLOOR)
    }

    fn draw_at(model: &Model, pose: &crate::vrm::Pose, eye: [u32; 2], eyes: [Option<Mat4>; 2], shift: Vec3, height: f32, floor: f32) -> Vec<u8> {
        let mut skinned = model.skinned().unwrap();
        let changed = skinned.morph(&pose.weights);
        let fit = crate::vrm::Fit::new(model, &skinned, height);
        let worlds = model.worlds(pose);
        let palette = skinned.palette(&worlds, Mat4::from_translation(shift) * fit.placement(model, &worlds, floor));
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), eye, height / PAGE_HEIGHT).unwrap();
        let mut appearance = renderer.appearance(model, &skinned).unwrap();
        appearance.update(&palette, &skinned, &changed);
        renderer.render(&appearance, eyes, Mat4::IDENTITY, eye[0]).unwrap();
        renderer.read().unwrap()
    }

    fn rendered(version: Version, eyes: [Vec3; 2]) -> (Vec<u8>, u32) {
        let model = Model::parse(&figure(version)).unwrap();
        (draw(&model, &model.rest(), [64, 64], eyes.map(|eye| eye_projection(eye, 0.2, 0.2)), Vec3::ZERO), 128)
    }

    fn pixel(pixels: &[u8], row_length: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * row_length + x) * 4) as usize;
        pixels[at..at + 4].try_into().unwrap()
    }

    fn covered_columns(pixels: &[u8], row_length: u32, eye: u32, y: u32) -> Vec<u32> {
        (0..64).filter(|&x| pixel(pixels, row_length, eye * 64 + x, y)[3] == 255).collect()
    }

    #[test]
    fn both_eyes_draw_the_textured_figure_on_a_clear_background() {
        for version in [Version::Zero, Version::One] {
            let (pixels, row) = rendered(version, [Vec3::new(-0.032, 0.0, 0.4), Vec3::new(0.032, 0.0, 0.4)]);
            for eye in 0..2 {
                assert_eq!(pixel(&pixels, row, eye * 64, 0), [0; 4], "the background stays transparent");
                assert_eq!(pixel(&pixels, row, eye * 64 + 63, 63), [0; 4]);
                let middle = pixel(&pixels, row, eye * 64 + 32, 40);
                assert_eq!(middle[3], 255, "eye {eye}: the opaque cutout figure covers the middle ({version:?})");
                assert!(middle[0] > 0 || middle[1] > 0 || middle[2] > 0);
            }
        }
    }

    #[test]
    fn a_windows_texels_hold_while_its_viewer_sways_and_follow_a_real_change_of_distance() {
        let most = eye_texels(OFF_HEADSET_DENSITY, 0.4, NEAREST);
        let mut density = Density::new(most);
        let edges = (0..12).map(|step| 0.4 * OFF_HEADSET_DENSITY / (most as f32 * STEP.powi(step)));
        for middle in edges.flat_map(|edge| [edge * 0.98, edge, edge * 1.02, edge * 1.05]) {
            for approach in [0.5, 2.0] {
                let mut held = density.settle(eye_texels(OFF_HEADSET_DENSITY, 0.4, middle * approach) as f32);
                let mut changes = 0;
                for step in 0..200 {
                    let sway = 0.02 * middle * (step as f32 * 0.7).sin();
                    let texels = density.settle(eye_texels(OFF_HEADSET_DENSITY, 0.4, middle + sway) as f32);
                    changes += usize::from(step > 0 && texels != held);
                    held = texels;
                }
                assert!(changes <= 1, "swaying 2% of the distance about {middle} m changed the texels {changes} times");
            }
        }
        let mut changes = 0;
        let mut last = density.settle(eye_texels(OFF_HEADSET_DENSITY, 0.4, 0.4) as f32);
        let there_and_back = (0..3600).map(|step| 0.4 + 0.001 * step as f32).chain((0..3600).rev().map(|step| 0.4 + 0.001 * step as f32));
        for distance in there_and_back {
            let (texels, wanted) = (density.settle(eye_texels(OFF_HEADSET_DENSITY, 0.4, distance) as f32), eye_texels(OFF_HEADSET_DENSITY, 0.4, distance) as f32);
            assert!(texels as f32 <= wanted * SLACK + 1.0 && texels as f32 >= wanted * STEP - 1.0, "at {distance} m: {texels} texels where the display sees {wanted}");
            changes += usize::from(texels != last);
            last = texels;
        }
        let steps = (10.0f32.ln() / -STEP.ln()) as usize;
        assert!((2 * steps - 4..=2 * steps + 4).contains(&changes), "walking out to ten times the distance and back changes the texels {changes} times, a step at a time: about {steps} each way");
        assert_eq!(density.settle(most as f32), most, "back near, the target's own size");
    }

    /// A figure's drawing time at full density, in budgets, and the share of it that stays whatever the texels:
    /// the rest goes as them.
    type Figure = (f32, f32);

    fn time_at(density: &Density, (cost, fixed): Figure, lowered: u32) -> f32 {
        cost * (fixed + (1.0 - fixed) * (density.step(lowered) / density.most as f32).powi(2))
    }

    /// Feeds `density` `frames` drawings of `figure`, each scattered up to 15% about its time, and returns how
    /// many of them changed the texels.
    fn draw_frames(density: &mut Density, figure: Figure, frames: usize, random: &mut crate::motion::Random) -> usize {
        let budget = density.budget.unwrap();
        let mut changes = 0;
        for _ in 0..frames {
            let texels = density.settle(density.most as f32);
            density.time(budget.mul_f32(time_at(density, figure, density.lowered) * (0.85 + 0.3 * random.next())));
            changes += usize::from(density.settle(density.most as f32) != texels);
        }
        changes
    }

    fn budgeted() -> Density {
        Density { budget: Some(std::time::Duration::from_micros(10_400)), ..Density::new(eye_texels(OFF_HEADSET_DENSITY, 0.4, NEAREST)) }
    }

    #[test]
    fn drawing_times_scattered_about_the_budget_leave_the_texels_alone() {
        let mut random = crate::motion::Random(0x9E37_79B9_7F4A_7C15);
        for fixed in [0.0, 0.5] {
            for cost in [0.8, 1.056, 1.2, 1.45, 1.6, 2.0, 2.7, 3.5, 5.0] {
                let mut density = budgeted();
                draw_frames(&mut density, (cost, fixed), 2000, &mut random);
                let label = format!("a figure taking {cost} budgets at full density, {fixed} of it whatever the texels");
                let took = time_at(&density, (cost, fixed), density.lowered);
                assert!(took <= 1.05 || density.lowered == LOWEST, "{label}, settles where it takes {took} of one");
                assert!(density.lowered == 0 || time_at(&density, (cost, fixed), density.lowered - 1) > HEADROOM - 0.08, "{label}, settles {} steps down, where a step denser would fit", density.lowered);
                let changes = draw_frames(&mut density, (cost, fixed), 5000, &mut random);
                assert!(changes <= 1, "{label}, changed its texels on {changes} of 5000 frames after settling");
            }
        }
    }

    #[test]
    fn one_slow_drawing_does_not_lower_the_texels() {
        let mut density = budgeted();
        let budget = density.budget.unwrap();
        for frame in 0..500 {
            density.time(if frame % 300 == 0 { budget * 6 } else { budget.mul_f32(0.9) });
        }
        assert_eq!(density.settle(density.most as f32), density.most);
    }

    #[test]
    fn a_costlier_figure_lowers_the_texels_and_a_lighter_one_returns_them_to_the_targets_size() {
        let mut random = crate::motion::Random(0x9E37_79B9_7F4A_7C15);
        let mut density = budgeted();
        let most = density.most;
        draw_frames(&mut density, (1.6, 0.0), 1000, &mut random);
        let settled = density.settle(most as f32);
        assert!(settled < most, "a figure over its budget draws fewer texels: {settled}");
        draw_frames(&mut density, (3.0, 0.0), 1000, &mut random);
        assert!(density.settle(most as f32) < settled, "a costlier one fewer still");
        draw_frames(&mut density, (100.0, 0.0), 1000, &mut random);
        let least = (most as f32 * STEP.powi(LOWEST as i32)).round() as u32;
        assert_eq!(density.settle(most as f32), least, "never under the least density");
        assert_eq!(density.settle(least as f32), (most as f32 * STEP.powi(2 * LOWEST as i32)).round() as u32, "a window seen from afar is lowered by as many steps");
        draw_frames(&mut density, (0.5, 0.5), 1000, &mut random);
        assert_eq!(density.settle(most as f32), most, "a figure that fits at full density returns to it");
        draw_frames(&mut density, (3.0, 0.0), 1000, &mut random);
        let budget = density.budget.take().unwrap();
        density.time(budget * 3);
        assert_eq!(density.settle(most as f32), most, "without a budget nothing is lowered");
    }

    #[test]
    fn the_texels_rise_by_the_cost_a_step_measured_not_by_the_texels_alone() {
        let mut random = crate::motion::Random(0x9E37_79B9_7F4A_7C15);
        let mut density = budgeted();
        draw_frames(&mut density, (1.6, 0.5), 1000, &mut random);
        assert_eq!(density.lowered, LOWEST, "half of this figure's time stays at any density, so no step fits it");
        let changes = draw_frames(&mut density, (1.05, 0.5), 2000, &mut random);
        let took = time_at(&density, (1.05, 0.5), density.lowered);
        assert!(density.lowered <= 3, "eased to 1.05 budgets it rises until a step denser would not fit, to where it takes {took} of one; by its texels alone it would stay {} steps down", density.lowered);
        assert!(took <= 1.0);
        assert_eq!(changes as u32, LOWEST - density.lowered, "each of its rises fitted");
    }

    #[test]
    fn a_load_that_lifts_just_after_the_texels_fell_still_lets_them_return() {
        let mut density = budgeted();
        let budget = density.budget.unwrap();
        for _ in 0..SETTLED {
            density.time(budget.mul_f32(1.2));
        }
        assert_eq!(density.lowered, 1);
        for _ in 0..10 * SETTLED {
            density.time(budget.mul_f32(0.3));
        }
        assert_eq!(density.lowered, 0);
    }

    #[test]
    fn a_farther_window_draws_no_denser_than_the_display_sees_it() {
        assert_eq!(eye_texels(OFF_HEADSET_DENSITY, 0.4, 0.2), eye_texels(OFF_HEADSET_DENSITY, 0.4, NEAREST), "nearer than the closest, the target's size");
        assert!(eye_texels(OFF_HEADSET_DENSITY, 0.4, 4.0) as f32 <= OFF_HEADSET_DENSITY * 0.4 / 4.0 + 1.0);
        assert_eq!(eye_texels(1000.0, 0.4, 0.5), 800, "a window takes the density its display reports");
        assert_eq!(eye_texels(500.0, 0.4, 0.5), 400);
        let mut gpu = Gpu::new(None).unwrap();
        gpu.density = 500.0;
        let renderer = Renderer::for_window(Rc::new(gpu), 0.4, 1.0).unwrap();
        assert_eq!((renderer.eye, renderer.texels(0.4, 0.5)), ([eye_texels(500.0, 0.4, NEAREST); 2], 400));
        let model = Model::parse(&figure(Version::One)).unwrap();
        let mut skinned = model.skinned().unwrap();
        let changed = skinned.morph(&model.rest().weights);
        let fit = crate::vrm::Fit::new(&model, &skinned, crate::HEIGHT);
        let worlds = model.worlds(&model.rest());
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [64, 64], crate::HEIGHT / PAGE_HEIGHT).unwrap();
        let mut appearance = renderer.appearance(&model, &skinned).unwrap();
        appearance.update(&skinned.palette(&worlds, fit.placement(&model, &worlds, crate::FLOOR)), &skinned, &changed);
        let eyes = [-0.032, 0.032].map(|x| eye_projection(Vec3::new(x, 0.0, 0.4), 0.2, 0.2));
        renderer.render(&appearance, eyes, Mat4::IDENTITY, 64).unwrap();
        let full = renderer.read().unwrap();
        renderer.render(&appearance, eyes, Mat4::IDENTITY, 16).unwrap();
        let far = renderer.read().unwrap();
        assert_eq!(far.len(), 32 * 16 * 4, "the image holds the drawn part alone");
        for eye in 0..2 {
            assert_eq!(pixel(&far, 32, eye * 16 + 8, 10)[3], 255, "eye {eye} draws the figure in its own half");
            assert_eq!(pixel(&far, 32, eye * 16, 0), [0; 4]);
            assert_eq!(pixel(&full, 128, eye * 64 + 32, 40)[3], 255);
        }
    }

    #[test]
    fn material_controls_change_color_without_changing_coverage() {
        let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({"VRMC_vrm": {"humanoid": {"humanBones": {}}}}));
        let position = builder.accessor("VEC3", &[-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        let normal = builder.accessor("VEC3", &[0.0, 0.0, 1.0].repeat(8));
        let uv = builder.accessor("VEC2", &[[0.25, 0.25].repeat(4), [0.75, 0.25].repeat(4)].concat());
        let indices = builder.indices(&[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        builder.set("nodes", serde_json::json!([{"mesh": 0}]));
        builder.set("meshes", serde_json::json!([{"primitives": [{"attributes": {"POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv}, "indices": indices, "material": 0}]}]));
        builder.set("materials", serde_json::json!([{"pbrMetallicRoughness": {"baseColorFactor": [1.0, 1.0, 1.0, 1.0]}}]));
        let data = builder.glb();
        for (space, value) in [(1.0, 1.0 / 3.0), (0.0, 0.5), (2.0, std::f32::consts::TAU / 3.0), (3.0, 1.0), (4.0, 2.0), (5.0, -1.0), (6.0, 1.0), (7.0, 1.0), (8.0, 1.0), (9.0, 1.0), (10.0, 1.0), (11.0, 0.0)] {
            let tone = [4.0, 5.0, 6.0, 11.0].iter().position(|&at| at == space);
            let mut model = Model::parse(&data).unwrap();
            model.images.push(crate::vrm::Image { width: 1, height: 1, rgba: if tone.is_some() || space == 10.0 { vec![128, 64, 32, 255] } else { vec![255, 0, 0, 255] } });
            model.materials[0].base_image = Some(0);
            model.materials[0].shade_image = Some(0);
            let rect = [0.0, 0.0, if space == 2.0 || space >= 4.0 { 1.0 } else { 0.5 }, 1.0];
            let effect = if let Some(channel) = tone {
                serde_json::json!({"tone": [{"node": 0, "rect": rect, "channel": channel}]})
            } else if space == 3.0 {
                serde_json::json!({"color": [{"node": 0, "rect": rect, "channel": 1, "color": [0.0, 0.0, 0.0]}]})
            } else if space >= 7.0 {
                serde_json::json!({"look": [{"node": 0, "rect": rect, "field": "decal", "slot": 1}]})
            } else {
                serde_json::json!({"hue": [{"node": 0, "rect": rect, "space": space, "shift": 1.0}]})
            };
            let rest = if space == 4.0 || space == 11.0 { 1.0 } else { 0.0 };
            let mut menu = crate::vrm::expressions::Expressions::parse(&serde_json::json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": [{"effect": effect, "default": rest}]})).unwrap();
            let mut mask = vec![255, 255, 255, 255, 0, 0, 0, 255];
            if let Some(tone) = tone {
                let channel = [3, 2, 1, 0][tone];
                mask[..4].fill(0);
                mask[4..].fill(255);
                mask[channel] = 255;
                mask[4 + channel] = 0;
            }
            menu.hue_mask = Some(crate::vrm::Image { width: 2, height: 1, rgba: mask });
            menu.untinted = Some(crate::vrm::expressions::Untinted { image: crate::vrm::Image { width: 4, height: 1, rgba: [[255, 0, 0, 255], [255, 0, 0, 255], [255; 4], [255; 4]].concat() }, cells: vec![[0.0, 0.0, 1.0, 1.0, 0.5, 0.0, 1.0, 1.0]] });
            let mut look = looks_test::look();
            look.decals[1] = Some(crate::vrm::expressions::Decal { alpha: 0.0, intensity: 1.0, color_blend: [(8.0, DecalBlend::Multiply), (9.0, DecalBlend::Lighten), (10.0, DecalBlend::Darken)].into_iter().find(|(at, _)| *at == space).map_or(DecalBlend::Replace, |(_, blend)| blend), image: 0, hue: None });
            menu.looks = Some(crate::vrm::expressions::Looks { index: Some(crate::vrm::Image { width: 1, height: 1, rgba: vec![1, 0, 0, 255] }), materials: vec![look], decals: vec![crate::vrm::Image { width: 2, height: 1, rgba: if space == 10.0 { vec![64, 255, 255, 255, 0, 0, 0, 0] } else { vec![0, 255, 0, 255, 0, 0, 0, 0] } }], matcaps: Vec::new(), matcap_masks: None, emission: None, clearcoat_maps: Vec::new(), outline: None, shades: Vec::new(), rim_tint: None });
            model.menu = Some(menu);
            let mut skinned = model.skinned().unwrap();
            let original_vertices = bytes_of(&skinned.vertices).to_vec();
            let worlds = model.worlds(&model.rest());
            let fit = crate::vrm::Fit::new(&model, &skinned, crate::HEIGHT);
            let palette = skinned.palette(&worlds, fit.placement(&model, &worlds, crate::FLOOR));
            let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [32, 32], crate::HEIGHT / PAGE_HEIGHT).unwrap();
            let mut appearance = renderer.appearance(&model, &skinned).unwrap();
            let eyes = [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None];
            let mut capture = |skinned: &Skinned| {
                appearance.update(&palette, skinned, &[]);
                renderer.render(&appearance, eyes, Mat4::IDENTITY, 32).unwrap();
                renderer.read().unwrap()
            };
            let before = capture(&skinned);
            skinned.surface_values(model.menu.as_ref().unwrap(), &[value]);
            let after = capture(&skinned);
            assert_eq!(bytes_of(&skinned.vertices), original_vertices, "material animation does not rewrite the mesh");
            assert!(before.chunks_exact(4).zip(after.chunks_exact(4)).all(|(a, b)| a[3] == b[3]));
            let center = pixel(&after, 64, 8, 16);
            assert_eq!(pixel(&after, 64, 24, 16), pixel(&before, 64, 24, 16), "the neighboring atlas region or masked texel stays unchanged");
            let initial = pixel(&before, 64, 8, 16);
            if space == 4.0 { assert!(center[0] < initial[0], "gamma darkens the midtone"); }
            else if space == 5.0 { assert!(center[0] < initial[0] && center[1] > initial[1], "desaturation moves red toward luminance"); }
            else if space == 6.0 { assert!(center[0] > initial[0], "brightness raises the midtone"); }
            else if space == 11.0 { assert!(center[0].abs_diff(initial[0]) <= 2 && center[2] > initial[2] + 16 && center[1] > initial[1], "an HSV saturation of 0 greys brown at its red's value: {initial:?} to {center:?}"); }
            else if space == 8.0 { assert!(center[0] < initial[0] / 4 && center[1] < 8, "multiplying red by green leaves black: {center:?}"); }
            else if space == 9.0 { assert!(center[0].abs_diff(initial[0]) <= 2 && center[1] > initial[0] / 2, "lightening red by green keeps the red and lays the green, yellow: {center:?}"); }
            else if space == 10.0 { assert!(center[0] > initial[0] / 3 && center[0] < initial[0] - initial[0] / 4 && center[1].abs_diff(initial[1]) <= 2 && center[2].abs_diff(initial[2]) <= 2, "darkening brown by cyan keeps the darker red, the cyan's, and the brown's green and blue: {initial:?} to {center:?}"); }
            else { assert!(center[1] > center[0], "material control {space} produces green: {center:?}"); }
            skinned.surface_values(model.menu.as_ref().unwrap(), &[rest]);
            let again = capture(&skinned);
            assert!(before.iter().zip(again).all(|(&a, b)| a.abs_diff(b) <= 1));
        }
    }

    mod looks_test {
        use crate::vrm::expressions::{Lighting, Look, Mapping, Matcap, HueShift};

        pub fn look() -> Look {
            Look { rect: [0.0, 0.0, 1.0, 1.0], decals: [None, None, None, None], matcaps: [None, None, None, None], emission: None, lighting: Lighting::Lit { ramp: None, shadow_tint: 0.0, cap: None }, rim: None, rim_shade: None, themes: Default::default(), clearcoat: None }
        }

        pub fn matcap() -> Matcap {
            Matcap { texture: Some(0), mode: Mapping::Normal, border: 0.5, color: [1.0; 4], intensity: 1.0, base_mix: 0.0, replace: 1.0, multiply: 0.0, add: 0.0, screen: 0.0, mixed: 0.0, light_add: 0.0, emission: 0.0, light_mask: 0.0, hue: Some(HueShift { shift: 0.0, space: 0.0, select: 1.0 }), circle: None, theme: None }
        }
    }

    /// How long after its first capture `look_drawn` takes its second: a third of the twenty seconds a theme colour's
    /// hue turns once in at a speed of one.
    const LATER: f64 = 20.0 / 3.0;

    /// Two quads over the left and right texels of a 2x1 atlas of `rgba`, the left one's material carrying `look`, drawn
    /// before and after its one target `effect` moves from `rest` to `value`: the left quad's pixel each time, and whether
    /// the right one stayed as it was.
    fn look_drawn(rgba: [u8; 4], look: crate::vrm::expressions::Look, effect: Option<serde_json::Value>, rest: f32, value: f32, matcaps: Vec<crate::vrm::Image>) -> ([u8; 4], [u8; 4], bool) {
        coat_drawn(rgba, look, effect, rest, value, matcaps, Vec::new())
    }

    /// `look_drawn` with `clearcoat_maps`, read through a second UV equal to the first.
    fn coat_drawn(rgba: [u8; 4], look: crate::vrm::expressions::Look, effect: Option<serde_json::Value>, rest: f32, value: f32, matcaps: Vec<crate::vrm::Image>, clearcoat_maps: Vec<crate::vrm::Image>) -> ([u8; 4], [u8; 4], bool) {
        let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({"VRMC_vrm": {"humanoid": {"humanBones": {}}}}));
        let position = builder.accessor("VEC3", &[-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        let normal = builder.accessor("VEC3", &[0.0, 0.0, 1.0].repeat(8));
        let uv = builder.accessor("VEC2", &[[0.25, 0.25].repeat(4), [0.75, 0.25].repeat(4)].concat());
        let indices = builder.indices(&[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        builder.set("nodes", serde_json::json!([{"mesh": 0}]));
        builder.set("meshes", serde_json::json!([{"primitives": [{"attributes": {"POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv, "TEXCOORD_1": uv}, "indices": indices, "material": 0}]}]));
        builder.set("materials", serde_json::json!([{"pbrMetallicRoughness": {"baseColorFactor": [1.0, 1.0, 1.0, 1.0]}}]));
        let mut model = Model::parse(&builder.glb()).unwrap();
        model.images.push(crate::vrm::Image { width: 1, height: 1, rgba: rgba.to_vec() });
        model.materials[0].base_image = Some(0);
        model.materials[0].shade_image = Some(0);
        let targets: Vec<serde_json::Value> = effect.into_iter().map(|effect| serde_json::json!({"effect": effect, "default": rest})).collect();
        let mut menu = crate::vrm::expressions::Expressions::parse(&serde_json::json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": targets})).unwrap();
        menu.looks = Some(crate::vrm::expressions::Looks {
            index: Some(crate::vrm::Image { width: 2, height: 1, rgba: vec![1, 0, 0, 255, 0, 0, 0, 255] }),
            materials: vec![look],
            decals: vec![crate::vrm::Image { width: 1, height: 1, rgba: vec![0, 255, 0, 255] }],
            matcaps,
            matcap_masks: None,
            emission: Some(crate::vrm::Image { width: 1, height: 1, rgba: vec![255; 4] }),
            clearcoat_maps,
            outline: None,
            shades: Vec::new(),
            rim_tint: None,
        });
        model.menu = Some(menu);
        let mut skinned = model.skinned().unwrap();
        let worlds = model.worlds(&model.rest());
        let fit = crate::vrm::Fit::new(&model, &skinned, crate::HEIGHT);
        let palette = skinned.palette(&worlds, fit.placement(&model, &worlds, crate::FLOOR));
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [32, 32], crate::HEIGHT / PAGE_HEIGHT).unwrap();
        let mut appearance = renderer.appearance(&model, &skinned).unwrap();
        let eyes = [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None];
        let mut capture = |skinned: &Skinned, seconds: f64| {
            appearance.seconds = seconds;
            appearance.update(&palette, skinned, &[]);
            renderer.render(&appearance, eyes, Mat4::IDENTITY, 32).unwrap();
            renderer.read().unwrap()
        };
        let before = capture(&skinned, 0.0);
        let targets = model.menu.as_ref().unwrap().targets.len();
        skinned.surface_values(model.menu.as_ref().unwrap(), &vec![value; targets]);
        let after = capture(&skinned, LATER);
        (pixel(&before, 64, 8, 16), pixel(&after, 64, 8, 16), pixel(&before, 64, 24, 16) == pixel(&after, 64, 24, 16))
    }

    #[test]
    fn a_flat_material_draws_its_albedo_under_its_cap_whichever_way_it_faces() {
        let mut flat = looks_test::look();
        flat.lighting = crate::vrm::expressions::Lighting::Flat { cap: Some(1.0), shadow: None };
        let (drawn, _, _) = look_drawn([128, 64, 32, 255], flat.clone(), None, 0.0, 0.0, Vec::new());
        assert!(drawn[..3].iter().zip([128, 64, 32]).all(|(&a, b)| a.abs_diff(b) <= 2), "capped at 1 under the overlay's lights, the albedo as its texture holds it: {drawn:?}");
        let (lit, _, _) = look_drawn([128, 64, 32, 255], looks_test::look(), None, 0.0, 0.0, Vec::new());
        assert!(lit[..3].iter().zip([128, 64, 32]).any(|(&a, b)| a.abs_diff(b) > 8), "a lit material facing the viewer is shaded by its lights' angles: {lit:?}");
        flat.lighting = crate::vrm::expressions::Lighting::Flat { cap: None, shadow: None };
        let (uncapped, _, _) = look_drawn([128, 64, 32, 255], flat, None, 0.0, 0.0, Vec::new());
        assert!(uncapped[0] > 140, "uncapped, it takes its main and ambient light whole: {uncapped:?}");
    }

    /// What `faces_drawn` lays beyond its look: the looks' matcap images, shade layers and rim tint, the menu's targets, each at
    /// its default, and whether the quads turn their back faces to the viewer, drawn two-sided.
    #[derive(Default)]
    struct Faces {
        matcaps: Vec<crate::vrm::Image>,
        shades: Vec<crate::vrm::Image>,
        rim_tint: Option<crate::vrm::Image>,
        targets: Vec<serde_json::Value>,
        back: bool,
    }

    /// Two quads facing the viewer whose vertex normals are `normals`, left and right, both of a one-texel atlas of
    /// `rgba` whose material carries `look`, with `faces`: the pixel at the middle of each.
    fn faces_drawn(rgba: [u8; 4], look: crate::vrm::expressions::Look, normals: [[f32; 3]; 2], faces: Faces) -> [[u8; 4]; 2] {
        let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({"VRMC_vrm": {"humanoid": {"humanBones": {}}}}));
        let position = builder.accessor("VEC3", &[-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        let normal = builder.accessor("VEC3", &normals.map(|normal| Vec3::from(normal).normalize().to_array().repeat(4)).concat());
        let uv = builder.accessor("VEC2", &[0.5, 0.5].repeat(8));
        let indices = builder.indices(if faces.back { &[0, 2, 1, 0, 3, 2, 4, 6, 5, 4, 7, 6] } else { &[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7] });
        builder.set("nodes", serde_json::json!([{"mesh": 0}]));
        builder.set("meshes", serde_json::json!([{"primitives": [{"attributes": {"POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv}, "indices": indices, "material": 0}]}]));
        builder.set("materials", serde_json::json!([{"doubleSided": faces.back, "pbrMetallicRoughness": {"baseColorFactor": [1.0, 1.0, 1.0, 1.0]}}]));
        let mut model = Model::parse(&builder.glb()).unwrap();
        model.images.push(crate::vrm::Image { width: 1, height: 1, rgba: rgba.to_vec() });
        model.materials[0].base_image = Some(0);
        let mut menu = crate::vrm::expressions::Expressions::parse(&serde_json::json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": faces.targets})).unwrap();
        menu.looks = Some(crate::vrm::expressions::Looks { index: Some(crate::vrm::Image { width: 1, height: 1, rgba: vec![1, 0, 0, 255] }), materials: vec![look], decals: Vec::new(), matcaps: faces.matcaps, matcap_masks: None, emission: None, clearcoat_maps: Vec::new(), outline: None, shades: faces.shades, rim_tint: faces.rim_tint });
        model.menu = Some(menu);
        let pixels = draw(&model, &model.rest(), [32, 32], [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None], Vec3::ZERO);
        [8, 24].map(|x| pixel(&pixels, 64, x, 16))
    }

    fn texel(rgb: [u8; 3]) -> crate::vrm::Image {
        crate::vrm::Image { width: 1, height: 1, rgba: vec![rgb[0], rgb[1], rgb[2], 255] }
    }

    #[test]
    fn a_shadowed_flat_material_takes_its_shade_colour_turned_from_the_key_light() {
        use crate::vrm::expressions::{Lighting, Shadow};
        let mut look = looks_test::look();
        look.lighting = Lighting::Flat { cap: Some(1.0), shadow: Some(Shadow { border: 0.5, blur: 0.1, strength: 1.0, second: None, edge: None }) };
        let [toward, away] = [[0.4685, 0.7496, 0.5622], [-0.4685, -0.7496, 0.5622]];
        let [lit, dark] = faces_drawn([128, 64, 32, 255], look.clone(), [toward, away], Faces { shades: vec![texel([40, 20, 30])], ..Faces::default() });
        assert!(lit[..3].iter().zip([128, 64, 32]).all(|(&a, b)| a.abs_diff(b) <= 2), "the side facing the key light draws the albedo: {lit:?}");
        assert!(dark[..3].iter().zip([40, 20, 30]).all(|(&a, b)| a.abs_diff(b) <= 2), "the side turned from it draws the shade colour: {dark:?}");
        let [_, brighter] = faces_drawn([128, 64, 32, 255], look, [toward, away], Faces { shades: vec![texel([200, 20, 30])], ..Faces::default() });
        assert!(brighter[..3].iter().zip([128, 20, 30]).all(|(&a, b)| a.abs_diff(b) <= 2), "a shade never outshines the albedo: {brighter:?}");
    }

    #[test]
    fn a_colour_region_moves_the_shade_as_it_moves_the_albedo() {
        use crate::vrm::expressions::{Lighting, Shadow};
        let mut look = looks_test::look();
        look.lighting = Lighting::Flat { cap: Some(1.0), shadow: Some(Shadow { border: 0.5, blur: 0.1, strength: 1.0, second: None, edge: None }) };
        let [toward, away] = [[0.4685, 0.7496, 0.5622], [-0.4685, -0.7496, 0.5622]];
        let grey = 0.21586;
        let doubled = serde_json::json!({"effect": {"color": [{"node": 0, "rect": [0, 0, 1, 1], "channel": 0, "color": [grey, grey, grey]}]}, "default": 2.0 * grey});
        let [lit, dark] = faces_drawn([128, 128, 128, 255], look, [toward, away], Faces { shades: vec![texel([64, 64, 64])], targets: vec![doubled], ..Faces::default() });
        assert!(lit[0].abs_diff(176) <= 3 && lit[1].abs_diff(128) <= 2, "the region doubles the albedo's linear red: {lit:?}");
        assert!(dark[0].abs_diff(90) <= 3 && dark[1].abs_diff(64) <= 2, "and the shade's, which a shade the region left would hold at its own: {dark:?}");
    }

    /// An image of `side` texels, red over its top half, blue over its lower right and green over its lower left.
    fn quartered(side: u32) -> crate::vrm::Image {
        let rgba = (0..side * side).flat_map(|at| match (at % side, at / side) {
            (_, row) if row < side / 2 => [255, 0, 0, 255],
            (column, _) if column >= side / 2 => [0, 0, 255, 255],
            _ => [0, 255, 0, 255],
        }).collect();
        crate::vrm::Image { width: side, height: side, rgba }
    }

    #[test]
    fn a_matcap_shows_its_top_to_a_normal_turned_up_and_its_right_to_one_turned_right() {
        let mut look = looks_test::look();
        look.lighting = crate::vrm::expressions::Lighting::Flat { cap: Some(1.0), shadow: None };
        look.matcaps[0] = Some(crate::vrm::expressions::Matcap { hue: None, ..looks_test::matcap() });
        let [up, right] = faces_drawn([128, 128, 128, 255], look, [[0.0, 0.7, 0.7], [0.7, -0.3, 0.65]], Faces { matcaps: vec![quartered(8)], ..Faces::default() });
        assert!(up[0] > 200 && up[1] < 40 && up[2] < 40, "a normal turned up takes the matcap's top, red: {up:?}");
        assert!(right[2] > 200 && right[0] < 40 && right[1] < 40, "a normal turned right takes its lower right, blue: {right:?}");
    }

    /// Poiyomi's reflection matcap reads the view reflected in Unity's axes, these mirrored through X: a surface turned
    /// right reflects the view right here, which is left there.
    #[test]
    fn a_reflection_matcap_shows_its_top_to_a_normal_turned_up_and_its_left_to_one_turned_right() {
        let mut look = looks_test::look();
        look.lighting = crate::vrm::expressions::Lighting::Flat { cap: Some(1.0), shadow: None };
        look.matcaps[0] = Some(crate::vrm::expressions::Matcap { hue: None, mode: crate::vrm::expressions::Mapping::Reflection, ..looks_test::matcap() });
        let [up, right] = faces_drawn([128, 128, 128, 255], look, [[0.0, 0.38, 0.92], [0.3, -0.2, 0.93]], Faces { matcaps: vec![quartered(8)], ..Faces::default() });
        assert!(up[0] > 200 && up[1] < 40 && up[2] < 40, "a normal turned up reflects into the matcap's top, red: {up:?}");
        assert!(right[1] > 200 && right[0] < 40 && right[2] < 40, "a normal turned right reflects into its lower left, green: {right:?}");
    }

    #[test]
    fn a_rim_shade_darkens_the_surface_turning_from_the_view() {
        use crate::vrm::expressions::{Lighting, RimShade};
        let mut look = looks_test::look();
        look.lighting = Lighting::Flat { cap: Some(1.0), shadow: None };
        let [square, grazing] = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.5]];
        let plain = faces_drawn([128, 64, 32, 255], look.clone(), [square, grazing], Faces::default());
        let shade = RimShade { color: [1.0, 0.25, 0.0], strength: 1.0, border: 0.5, blur: 0.8, power: 1.0 };
        let drawn = |shade: RimShade| faces_drawn([128, 64, 32, 255], crate::vrm::expressions::Look { rim_shade: Some(shade), ..look.clone() }, [square, grazing], Faces::default());
        let [facing, shaded] = drawn(shade.clone());
        assert_eq!(facing, plain[0], "a surface square to the view takes no rim shade");
        assert!(shaded[0] == plain[1][0] && shaded[1] + 20 < plain[1][1] && shaded[2] + 10 < plain[1][2] && shaded[2] > 0, "a surface part of the way across the band is multiplied part of the way to the rim shade's colour: {shaded:?} under {:?}", plain[1]);
        let [_, squared] = drawn(RimShade { power: 2.0, ..shade.clone() });
        assert!(squared[1] > shaded[1] + 5 && squared[1] + 5 < plain[1][1], "a higher power lowers the fresnel term and so darkens less: {squared:?} between {shaded:?} and {:?}", plain[1]);
        let [_, late] = drawn(RimShade { border: 1.0, blur: 0.0, ..shade.clone() });
        assert_eq!(late, plain[1], "a band no turn reaches leaves it as it was");
        let [_, half] = drawn(RimShade { strength: 0.5, ..shade.clone() });
        assert!(half[0] == plain[1][0] && half[1] > shaded[1] + 5 && half[1] + 5 < plain[1][1], "half its strength darkens half as far: {half:?} between {shaded:?} and {:?}", plain[1]);
        let [_, whole] = drawn(RimShade { border: 0.0, blur: 0.0, ..shade.clone() });
        assert!(whole[0] == plain[1][0] && whole[2] == 0, "past the band the colour is multiplied by the rim shade's whole: {whole:?} under {:?}", plain[1]);
        look.rim_shade = Some(shade);
        let behind = [[0.0, 0.0, -1.0], [-1.0, 0.0, -0.5]];
        let back = || Faces { back: true, ..Faces::default() };
        let [_, unshaded] = faces_drawn([128, 64, 32, 255], crate::vrm::expressions::Look { rim_shade: None, ..look.clone() }, behind, back());
        let [_, behind_shaded] = faces_drawn([128, 64, 32, 255], look, behind, back());
        assert!(behind_shaded[1] + 20 < unshaded[1], "a back face turning from the view is darkened too: {behind_shaded:?} under {unshaded:?}");
    }

    #[test]
    fn a_rim_brightens_the_surface_turning_from_the_view() {
        use crate::vrm::expressions::{Lighting, Rim};
        let mut look = looks_test::look();
        look.lighting = Lighting::Flat { cap: Some(1.0), shadow: None };
        let [square, grazing] = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.15]];
        let plain = faces_drawn([128, 64, 32, 255], look.clone(), [square, grazing], Faces::default());
        look.rim = Some(Rim { color: [0.0, 0.0, 1.0], strength: 1.0, main: 0.0, border: 0.5, blur: 0.1, power: 1.0, lit: 1.0, shadow_mask: 0.0, backface: false });
        let [facing, rimmed] = faces_drawn([128, 64, 32, 255], look.clone(), [square, grazing], Faces::default());
        assert_eq!(facing, plain[0], "a surface square to the view takes no rim");
        assert!(rimmed[2] > plain[1][2] + 100 && rimmed[..2] == plain[1][..2], "the grazing surface adds the rim's colour: {rimmed:?} over {:?}", plain[1]);
        let texel = |rgba: [u8; 4]| crate::vrm::Image { width: 1, height: 1, rgba: rgba.to_vec() };
        let tinted = |blue: u8, shades: usize| faces_drawn([128, 64, 32, 255], look.clone(), [square, grazing], Faces { shades: vec![texel([128, 64, 32, 255]); shades], rim_tint: Some(texel([255, 255, blue, 255])), ..Faces::default() })[1];
        for shades in 0..=2 {
            assert_eq!(tinted(0, shades), plain[1], "a rim tint without the rim's colour takes the rim away, after {shades} shade layers");
            let half = tinted(188, shades);
            assert!(half[2] > plain[1][2] + 20 && half[2] + 20 < rimmed[2], "a rim tint half the rim's colour in linear light adds part of it, after {shades} shade layers: {half:?} between {:?} and {rimmed:?}", plain[1]);
        }
        let behind = [[0.0, 0.0, -1.0], [-1.0, 0.0, -0.15]];
        let back = || Faces { back: true, ..Faces::default() };
        let [_, unlit] = faces_drawn([128, 64, 32, 255], crate::vrm::expressions::Look { rim: None, ..look.clone() }, behind, back());
        let [_, hidden] = faces_drawn([128, 64, 32, 255], look.clone(), behind, back());
        assert_eq!(hidden, unlit, "a rim hidden on back faces leaves a grazing back face as it was");
        let [_, shown] = faces_drawn([128, 64, 32, 255], crate::vrm::expressions::Look { rim: look.rim.clone().map(|rim| Rim { backface: true, ..rim }), ..look }, behind, back());
        assert!(shown[2] > unlit[2] + 100 && shown[..2] == unlit[..2], "one drawn on back faces adds its colour there: {shown:?} over {unlit:?}");
    }

    #[test]
    fn a_ramp_lit_material_takes_its_main_light_through_its_ramp() {
        let lighting = |json: serde_json::Value| serde_json::from_value::<crate::vrm::expressions::Lighting>(json).unwrap();
        let mut look = looks_test::look();
        look.lighting = lighting(serde_json::json!({"model": "lit", "ramp": [1.0, 1.0], "shadowTint": 0.5, "cap": 1.0}));
        let (whole, _, _) = look_drawn([128, 64, 32, 255], look.clone(), None, 0.0, 0.0, Vec::new());
        assert!(whole[..3].iter().zip([128, 64, 32]).all(|(&a, b)| a.abs_diff(b) <= 2), "a ramp whole at every angle, capped at 1, draws the albedo as a flat one does: {whole:?}");
        look.lighting = lighting(serde_json::json!({"model": "lit", "ramp": [0.0, 0.0]}));
        let (dark, _, _) = look_drawn([128, 64, 32, 255], look.clone(), None, 0.0, 0.0, Vec::new());
        assert!(dark[0] < 100, "a ramp dark at every angle with no shadow tint leaves the ambient alone: {dark:?}");
        look.lighting = lighting(serde_json::json!({"model": "lit", "ramp": [0.0, 0.0], "shadowTint": 1.0}));
        let (tinted, _, _) = look_drawn([128, 64, 32, 255], look, None, 0.0, 0.0, Vec::new());
        assert!(tinted[0] > dark[0] + 4 && tinted[2] <= dark[2] + 2, "a whole shadow tint lends the shade the albedo, its red more than its blue: {tinted:?} over {dark:?}");
        assert_eq!(lighting(serde_json::json!({"model": "lit"})), crate::vrm::expressions::Lighting::Lit { ramp: None, shadow_tint: 0.0, cap: None }, "a lit material without a ramp is lit by angle");
    }

    #[test]
    fn a_decal_no_clip_animates_draws_at_its_saved_alpha() {
        let mut look = looks_test::look();
        look.decals[1] = Some(crate::vrm::expressions::Decal { alpha: 1.0, intensity: 1.0, color_blend: DecalBlend::Replace, image: 0, hue: None });
        let (shown, _, _) = look_drawn([255, 0, 0, 255], look.clone(), None, 0.0, 0.0, Vec::new());
        assert!(shown[1] > shown[0], "the texel's material lays its decal on with no surface value: {shown:?}");
        let (hidden, _, _) = look_drawn([255, 0, 0, 255], looks_test::look(), None, 0.0, 0.0, Vec::new());
        assert!(hidden[0] > hidden[1], "a slot without a decal draws none: {hidden:?}");
        let effect = serde_json::json!({"look": [{"node": 0, "rect": [0.0, 0.0, 1.0, 1.0], "field": "decal", "slot": 1}]});
        let (kept, cleared, right) = look_drawn([255, 0, 0, 255], look, Some(effect), 0.5, 0.0, Vec::new());
        assert!(kept[1] > kept[0], "a target at rest leaves the material its own alpha: {kept:?}");
        assert!(cleared[0] > cleared[1] && right, "an animated alpha overrides the saved one wherever the renderer shows: {cleared:?}");
    }

    #[test]
    fn a_clip_turns_a_decals_hue_and_its_rest_turn_holds_without_one() {
        let mut look = looks_test::look();
        let hue = crate::vrm::expressions::HueShift { shift: 0.0, space: 1.0, select: 1.0 };
        look.decals[1] = Some(crate::vrm::expressions::Decal { alpha: 1.0, intensity: 1.0, color_blend: DecalBlend::Replace, image: 0, hue: Some(hue.clone()) });
        let effect = serde_json::json!({"look": [{"node": 0, "rect": [0.0, 0.0, 1.0, 1.0], "field": "decalHue", "slot": 1}]});
        let (rest, turned, _) = look_drawn([0, 0, 0, 255], look.clone(), Some(effect), 0.0, 1.0 / 3.0, Vec::new());
        assert!(rest[1] > 2 * rest[2], "at its rest turn the green decal stays green: {rest:?}");
        assert!(turned[2] > 2 * turned[1], "a third of a turn in HSV takes green to blue: {turned:?}");
        look.decals[1].as_mut().unwrap().hue = Some(crate::vrm::expressions::HueShift { shift: 1.0 / 3.0, ..hue });
        let (saved, _, _) = look_drawn([0, 0, 0, 255], look, None, 0.0, 0.0, Vec::new());
        assert!(saved[2] > 2 * saved[1], "no clip: the decal draws turned by its saved shift: {saved:?}");
    }

    /// A square quad textured black over the whole atlas, whose 8x8 look index gives material 1 the top-right quarter,
    /// `[0.5, 0.0, 1.0, 0.5]`, and its first slot `decal`: the quad's covered pixels, a 64-pixel-wide eye.
    fn decal_drawn(decal: crate::vrm::Image) -> Vec<u8> {
        let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({"VRMC_vrm": {"humanoid": {"humanBones": {}}}}));
        let position = builder.accessor("VEC3", &[-0.5, 0.0, 0.0, 0.5, 0.0, 0.0, 0.5, 1.0, 0.0, -0.5, 1.0, 0.0]);
        let normal = builder.accessor("VEC3", &[0.0, 0.0, 1.0].repeat(4));
        let uv = builder.accessor("VEC2", &[0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
        let indices = builder.indices(&[0, 1, 2, 0, 2, 3]);
        builder.set("nodes", serde_json::json!([{"mesh": 0}]));
        builder.set("meshes", serde_json::json!([{"primitives": [{"attributes": {"POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv}, "indices": indices, "material": 0}]}]));
        builder.set("materials", serde_json::json!([{"pbrMetallicRoughness": {"baseColorFactor": [1.0, 1.0, 1.0, 1.0]}}]));
        let mut model = Model::parse(&builder.glb()).unwrap();
        model.images.push(crate::vrm::Image { width: 1, height: 1, rgba: vec![0, 0, 0, 255] });
        model.materials[0].base_image = Some(0);
        model.materials[0].shade_image = Some(0);
        let mut menu = crate::vrm::expressions::Expressions::parse(&serde_json::json!({"menu": [], "parameters": [], "layers": [], "clips": [], "targets": []})).unwrap();
        let mut look = looks_test::look();
        look.rect = [0.5, 0.0, 1.0, 0.5];
        look.decals[0] = Some(crate::vrm::expressions::Decal { alpha: 1.0, intensity: 1.0, color_blend: DecalBlend::Replace, image: 1, hue: None });
        let index = (0..64).flat_map(|at| [(at % 8 >= 4 && at / 8 < 4) as u8, 0, 0, 255]).collect();
        let unused = crate::vrm::Image { width: 3, height: 5, rgba: vec![255; 60] };
        menu.looks = Some(crate::vrm::expressions::Looks { index: Some(crate::vrm::Image { width: 8, height: 8, rgba: index }), materials: vec![look], decals: vec![unused, decal], matcaps: Vec::new(), matcap_masks: None, emission: None, clearcoat_maps: Vec::new(), outline: None, shades: Vec::new(), rim_tint: None });
        model.menu = Some(menu);
        let skinned = model.skinned().unwrap();
        let worlds = model.worlds(&model.rest());
        let fit = crate::vrm::Fit::new(&model, &skinned, crate::HEIGHT);
        let palette = skinned.palette(&worlds, fit.placement(&model, &worlds, crate::FLOOR));
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [64, 64], crate::HEIGHT / PAGE_HEIGHT).unwrap();
        let mut appearance = renderer.appearance(&model, &skinned).unwrap();
        appearance.update(&palette, &skinned, &[]);
        renderer.render(&appearance, [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None], Mat4::IDENTITY, 64).unwrap();
        renderer.read().unwrap()
    }

    #[test]
    fn a_decal_of_any_size_spans_exactly_its_materials_cell() {
        let quadrants = |side: u32| {
            let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]];
            let rgba = (0..side * side).flat_map(|at| colors[((at / side >= side / 2) as usize) * 2 + (at % side >= side / 2) as usize]).collect();
            crate::vrm::Image { width: side, height: side, rgba }
        };
        for side in [2, 16] {
            let pixels = decal_drawn(quadrants(side));
            let covered: Vec<(u32, u32)> = (0..64).flat_map(|y| (0..64).map(move |x| (x, y))).filter(|&(x, y)| pixel(&pixels, 128, x, y)[3] == 255).collect();
            let (left, right) = (covered.iter().map(|p| p.0).min().unwrap(), covered.iter().map(|p| p.0).max().unwrap() + 1);
            let (top, bottom) = (covered.iter().map(|p| p.1).min().unwrap(), covered.iter().map(|p| p.1).max().unwrap() + 1);
            assert!(left > 0 && right < 64 && top > 0 && bottom < 64, "the whole quad is in view: {left}..{right} x {top}..{bottom}");
            let at = |u: f32, v: f32| pixel(&pixels, 128, left + (u * (right - left) as f32) as u32, top + (v * (bottom - top) as f32) as u32);
            let dominant = |color: [u8; 4]| -> &str {
                let [r, g, b, _] = color.map(u32::from);
                if r.max(g).max(b) < 24 { "black" } else if r.min(g).min(b) * 2 > r.max(g).max(b) { "white" } else if r > 2 * g && r > 2 * b { "red" } else if g > 2 * r && g > 2 * b { "green" } else if b > 2 * r && b > 2 * g { "blue" } else { "mixed" }
            };
            for (u, v, expected) in [(0.625, 0.125, "red"), (0.875, 0.125, "green"), (0.625, 0.375, "blue"), (0.875, 0.375, "white")] {
                assert_eq!(dominant(at(u, v)), expected, "a {side}x{side} decal's quadrant lands on its quarter of the cell at uv ({u}, {v}): {:?}", at(u, v));
            }
            for (u, v) in [(0.25, 0.25), (0.25, 0.75), (0.75, 0.75), (0.45, 0.25), (0.75, 0.55)] {
                assert_eq!(dominant(at(u, v)), "black", "a {side}x{side} decal stays off the atlas outside its cell at uv ({u}, {v}): {:?}", at(u, v));
            }
        }
    }

    #[test]
    fn decals_share_pages_by_size_and_no_page_outgrows_its_budget_but_a_lone_decal() {
        let image = |side: u32| crate::vrm::Image { width: side, height: side, rgba: Vec::new() };
        let decals: Vec<crate::vrm::Image> = [[2048; 25].as_slice(), &[1024; 7], &[512; 4], &[4096; 2], &[4; 2], &[8192; 1]].concat().into_iter().map(image).collect();
        let (pages, places) = decal_pages(&decals);
        for page in &pages {
            let decal = &decals[page[0]];
            assert!(page.iter().all(|&at| (decals[at].width, decals[at].height) == (decal.width, decal.height)), "a page holds one size");
            let bytes = page.len() as u64 * decal.width as u64 * decal.height as u64 * 16 / 3;
            assert!(bytes <= PAGE_BYTES || page.len() == 1, "{} decals of {}: {bytes} bytes", page.len(), decal.width);
        }
        assert_eq!(pages.len(), 2 + 1 + 1 + 1 + 1 + 1);
        for (at, place) in places.iter().enumerate() {
            assert_eq!(pages[place.page][place.layer as usize], at, "each decal's place names it");
        }
    }

    #[test]
    fn emission_glows_unlit_and_turns_by_its_hue() {
        let mut look = looks_test::look();
        look.emission = Some(crate::vrm::expressions::Emission { color: [1.0, 0.0, 0.0], strength: 1.0, hue: 0.0, shifts_hue: true, theme: None });
        let effect = serde_json::json!({"look": [{"node": 0, "rect": [0.0, 0.0, 1.0, 1.0], "field": "emissionHue", "slot": 0}]});
        let (red, green, right) = look_drawn([0, 0, 0, 255], look, Some(effect), 0.0, std::f32::consts::TAU / 3.0, Vec::new());
        assert!(red[0] > 200 && red[1] < 40, "a black albedo glows its emission colour: {red:?}");
        assert!(green[1] > 200 && green[0] < 40, "a third of a turn takes red to green: {green:?}");
        assert!(right, "a texel of a material without emission stays dark");
    }

    #[test]
    fn a_theme_colours_offsets_recolour_the_emission_drawn_in_it() {
        let theme = |hue: f32, speed: f32| crate::vrm::expressions::Theme { hsv: [0.0, 1.0, 0.5], exposure: 2.0, hue, saturation: 0.0, value: 0.0, speed };
        let themed = |theme: crate::vrm::expressions::Theme| {
            let mut look = looks_test::look();
            look.emission = Some(crate::vrm::expressions::Emission { color: [1.0, 0.0, 0.0], strength: 1.0, hue: 0.0, shifts_hue: false, theme: Some(2) });
            look.themes[2] = Some(theme);
            look
        };
        let look = themed(theme(0.0, 0.0));
        let effect = |field: &str| serde_json::json!({"look": [{"node": 0, "rect": [0.0, 0.0, 1.0, 1.0], "field": field, "slot": 2}]});
        let (red, green, right) = look_drawn([0, 0, 0, 255], look.clone(), Some(effect("themeHue")), 0.0, 1.0 / 3.0, Vec::new());
        assert!(red[0] > 200 && red[1] < 40, "a black albedo glows its theme colour, at its exposure: {red:?}");
        assert!(green[1] > 200 && green[0] < 40, "a third of a turn of the theme's hue takes red to green: {green:?}");
        assert!(right, "a texel of a material without emission stays dark");
        let (_, grey, _) = look_drawn([0, 0, 0, 255], look.clone(), Some(effect("themeSaturation")), 0.0, -1.0, Vec::new());
        assert!(grey[0] > 200 && grey[1] > 200 && grey[2] > 200, "the theme's saturation taken to nothing makes it white: {grey:?}");
        let (_, dark, _) = look_drawn([0, 0, 0, 255], look.clone(), Some(effect("themeValue")), 0.0, -0.5, Vec::new());
        assert!(dark[0] < 24, "the theme's value taken down by its own makes it dark: {dark:?}");
        let (_, turned, _) = look_drawn([0, 0, 0, 255], look.clone(), Some(effect("themeSpeed")), 0.0, 1.0, Vec::new());
        assert!(turned[1] > 200 && turned[0] < 40, "a hue speed of one turns it a third in a third of twenty seconds: {turned:?}");
        let (_, kept, _) = look_drawn([0, 0, 0, 255], look, Some(serde_json::json!({"look": [{"node": 0, "rect": [0.0, 0.0, 1.0, 1.0], "field": "themeHue", "slot": 1}]})), 0.0, 1.0 / 3.0, Vec::new());
        assert!(kept[0] > 200 && kept[1] < 40, "another theme's hue leaves it: {kept:?}");
        let (shifted, _, _) = look_drawn([0, 0, 0, 255], themed(theme(1.0 / 3.0, 0.0)), None, 0.0, 0.0, Vec::new());
        assert!(shifted[1] > 200 && shifted[0] < 40, "a theme resting a third of a turn round glows green with no clip: {shifted:?}");
        let (start, later, _) = look_drawn([0, 0, 0, 255], themed(theme(0.0, 1.0)), None, 0.0, 0.0, Vec::new());
        assert!(start[0] > 200 && start[1] < 40 && later[1] > 200 && later[0] < 40, "a theme turning at rest turns with no clip: {start:?} then {later:?}");
    }

    #[test]
    fn a_matcap_in_a_theme_colour_takes_its_offsets() {
        let mut look = looks_test::look();
        let mut matcap = looks_test::matcap();
        matcap.hue = None;
        matcap.theme = Some(1);
        look.matcaps[0] = Some(matcap);
        look.themes[1] = Some(crate::vrm::expressions::Theme { hsv: [1.0 / 3.0, 1.0, 1.0], exposure: 1.0, hue: 0.0, saturation: 0.0, value: 0.0, speed: 0.0 });
        let effect = serde_json::json!({"look": [{"node": 0, "rect": [0.0, 0.0, 1.0, 1.0], "field": "themeHue", "slot": 1}]});
        let white = vec![crate::vrm::Image { width: 1, height: 1, rgba: vec![255; 4] }];
        let (before, after, right) = look_drawn([255, 0, 0, 255], look, Some(effect), 0.0, 1.0 / 3.0, white);
        assert!(before[1] > 2 * before[0] && before[1] > 2 * before[2], "a white matcap in a green theme colour replaces red with green: {before:?}");
        assert!(after[2] > 2 * after[0] && after[2] > 2 * after[1], "a third of a turn of the theme's hue takes it to blue: {after:?}");
        assert!(right, "the matcap stays on its material's texels");
    }

    #[test]
    fn a_matcap_replaces_the_albedo_and_turns_by_its_hue() {
        let mut look = looks_test::look();
        look.matcaps[2] = Some(looks_test::matcap());
        let effect = serde_json::json!({"look": [{"node": 0, "rect": [0.0, 0.0, 1.0, 1.0], "field": "matcapHueShift", "slot": 2}]});
        let green = vec![crate::vrm::Image { width: 1, height: 1, rgba: vec![0, 255, 0, 255] }];
        let (before, after, right) = look_drawn([255, 0, 0, 255], look, Some(effect), 0.0, 0.5, green);
        assert!(before[1] > before[0], "the matcap replaces red with green: {before:?}");
        assert!(after[0] > after[1], "half a turn of hue takes green toward magenta: {after:?}");
        assert!(right, "the matcap stays on its material's texels");
    }

    #[test]
    fn a_clear_coat_brightens_its_material_where_its_map_masks_it_and_follows_an_animated_offset() {
        let coated = |strength: f32| {
            let mut look = looks_test::look();
            look.clearcoat = Some(crate::vrm::expressions::ClearCoat { strength, smoothness: 0.9, reflection: 1.0, specular: 1.0, fresnel: 1.0, specular_tint: [1.0; 3], reflection_tint: [1.0; 3], st: [1.0, 1.0, 0.0, 0.0], inverts: [false; 4], gsaa: [0.15, 0.1], map: Some(0) });
            look
        };
        let map = vec![crate::vrm::Image { width: 2, height: 1, rgba: vec![255, 255, 255, 255, 0, 255, 255, 255] }];
        let offset = serde_json::json!({"look": [{"node": 0, "rect": [0.0, 0.0, 1.0, 1.0], "field": "clearcoatST", "slot": 2}]});
        let (bare, _, _) = coat_drawn([64, 64, 64, 255], coated(0.0), None, 0.0, 0.0, Vec::new(), map.clone());
        let (plain, _, _) = look_drawn([64, 64, 64, 255], looks_test::look(), None, 0.0, 0.0, Vec::new());
        assert!(bare.iter().zip(plain).all(|(&a, b)| a.abs_diff(b) <= 1), "a coat of no strength draws as no coat: {bare:?} {plain:?}");
        let (wet, dry, right) = coat_drawn([64, 64, 64, 255], coated(1.0), Some(offset), 0.0, 0.5, Vec::new(), map);
        assert!(wet[..3].iter().zip(&bare[..3]).all(|(&a, &b)| a > b + 8), "the coat adds its sheen where the map's mask is full: {wet:?} over {bare:?}");
        assert!(dry.iter().zip(bare).all(|(&a, b)| a.abs_diff(b) <= 1), "half the map over, its mask is empty: {dry:?}");
        assert!(right, "a texel of an uncoated material stays as it was");
    }

    #[test]
    fn every_figure_entry_compiles_with_every_feature_and_with_none() {
        let every = Features { decals: 0b1111, matcaps: 0b1111, shadow: true, second_shadow: true, rim: true, rim_shade: true, emission: true, flat: true, ramp: true, coat: true };
        for shading in [Shading::default(), Shading { pages: *DECAL_PAGES, features: every, untinted: true, outline_unlit: false }, Shading { outline_unlit: true, ..Shading::default() }] {
            for (stage, entry) in [(naga::ShaderStage::Vertex, "vertex"), (naga::ShaderStage::Vertex, "coated_vertex"), (naga::ShaderStage::Fragment, "depth"), (naga::ShaderStage::Fragment, "fragment"), (naga::ShaderStage::Fragment, "cutout"), (naga::ShaderStage::Fragment, "coated_fragment"), (naga::ShaderStage::Fragment, "coated_cutout")] {
                AVATAR_SHADER.spirv(stage, entry, &shading.constants()).unwrap();
            }
        }
    }

    #[test]
    fn only_draws_reaching_a_coated_look_take_the_coated_pipeline() {
        let model = Model::parse(&figure(Version::One)).unwrap();
        let skinned = model.skinned().unwrap();
        assert!(skinned.draws.iter().all(|draw| !draw.features.coat) && skinned.coat_uvs.is_empty(), "an uncoated figure keeps its plain pipelines and no second UV");
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [16, 16], 1.0).unwrap();
        let _appearance = renderer.appearance(&model, &skinned).unwrap();
        assert!(renderer.pipelines.keys().all(|key| !key.shading.features.coat));
    }

    #[test]
    fn the_texture_reaches_the_screen_unmixed() {
        let (pixels, row) = rendered(Version::One, [Vec3::new(0.0, 0.0, 0.4); 2]);
        let column = covered_columns(&pixels, row, 0, 58);
        let lower_left = pixel(&pixels, row, column[0] + 2, 58);
        let lower_right = pixel(&pixels, row, column[column.len() / 2 + 2], 58);
        assert!(lower_left[2] > lower_left[0] && lower_left[2] > lower_left[1], "the blue texel is at the figure's lower left: {lower_left:?}");
        assert!(lower_right[0] > lower_left[0] + 60 && lower_right[1] > lower_left[1] + 60, "the white texel is at its lower right: {lower_right:?}");
        assert_eq!(lower_right[3], 255, "a cutout texel above the cutoff is opaque, whatever its alpha");
    }

    #[test]
    fn a_screen_outline_keeps_its_width_when_the_lit_frame_turns_from_the_window() {
        let corners = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
        let outlined = |factor: f32, space: Mat4| {
            let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": {} } } }));
            let position = builder.accessor("VEC3", corners.as_flattened());
            let normal = builder.accessor("VEC3", corners.as_flattened());
            let indices = builder.indices(&[0, 2, 4, 2, 1, 4, 1, 3, 4, 3, 0, 4, 2, 0, 5, 1, 2, 5, 3, 1, 5, 0, 3, 5]);
            builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position, "NORMAL": normal }, "indices": indices, "material": 0 }] }]));
            builder.set("materials", serde_json::json!([{ "doubleSided": true, "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0] } }]));
            let mut model = Model::parse(&builder.glb()).unwrap();
            model.materials[0].outline = Some(crate::vrm::Outline { width: crate::vrm::OutlineWidth::Screen, factor, width_image: None, color: [0.0; 3], lighting_mix: 0.0, layered: false });
            let skinned = model.skinned().unwrap();
            let worlds = model.worlds(&model.rest());
            let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [64, 64], crate::HEIGHT / PAGE_HEIGHT).unwrap();
            let mut appearance = renderer.appearance(&model, &skinned).unwrap();
            appearance.update(&skinned.palette(&worlds, space.inverse() * Mat4::from_scale(Vec3::splat(0.08))), &skinned, &[]);
            renderer.render(&appearance, [eye_projection(Vec3::new(0.05, 0.15, 0.4), 0.2, 0.2), None], space, 64).unwrap();
            renderer.read().unwrap().chunks_exact(4).map(|texel| texel[3] == 255).collect::<Vec<bool>>()
        };
        let covered = |mask: &[bool]| mask.iter().filter(|&&covered| covered).count();
        let square = outlined(0.1, Mat4::IDENTITY);
        let bare = covered(&outlined(0.0, Mat4::IDENTITY));
        assert!(covered(&square) > bare + 100, "the outline widens the figure: {} texels over {bare}", covered(&square));
        for turn in [Mat4::from_rotation_x(0.6), Mat4::from_rotation_x(-0.6), Mat4::from_rotation_z(0.5) * Mat4::from_rotation_x(0.4)] {
            let turned = outlined(0.1, turn);
            let changed = square.iter().zip(&turned).filter(|(a, b)| a != b).count();
            assert!(changed <= 4, "skinning into a frame turned {turn} from the window changes {changed} of {} outlined texels", covered(&square));
        }
    }

    #[test]
    fn particle_systems_draw_where_they_are_born_in_their_blend() {
        let at = |x: f32, z: f32| Mat4::from_translation(Vec3::new(x, -0.6, z)).to_cols_array();
        let system = |x: f32, z: f32, mode: &str, blend: &str, color: [f32; 4], most: f32| serde_json::json!({
            "node": 0, "world": true, "loop": false, "bursts": [{"time": 0, "count": {"min": [1], "max": [1], "scale": [1, 1]}}],
            "speed": {"min": [1], "max": [1], "scale": [0, 0]}, "lifetime": {"min": [1], "max": [1], "scale": [10, 10]},
            "size": {"min": [1], "max": [1], "scale": [0.3, 0.3]}, "shape": {"type": "point", "matrix": at(x, z)},
            "renderer": {"draw": true, "mode": mode, "mesh": 0, "align": "world", "maxSize": most, "materials": [{"blend": blend, "color": color}]}
        });
        let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({
            "VRMC_vrm": {"humanoid": {"humanBones": {}}},
            "UNITY_particles": {
                "systems": [
                    system(-0.8, 0.5, "billboard", "opaque", [1.0, 1.0, 1.0, 1.0], 0.5), system(0.0, 0.5, "billboard", "add", [1.0, 0.0, 0.0, 1.0], 0.5), system(0.8, 0.5, "mesh", "cutout", [0.0, 1.0, 0.0, 1.0], 0.5), system(0.4, 0.5, "billboard", "opaque", [1.0, 1.0, 1.0, 1.0], 0.02),
                    system(-0.4, 0.5, "billboard", "opaque", [1.0, 1.0, 1.0, 1.0], 0.5), system(-0.4, 0.7, "billboard", "multiply", [1.0, 0.0, 0.0, 0.5], 0.5),
                    {"node": 0, "renderer": {"draw": true, "materials": []}}
                ],
                "meshes": [{"positions": [-0.5, -0.5, 0, 0.5, -0.5, 0, 0.5, 0.5, 0, -0.5, 0.5, 0], "uvs": [0, 1, 1, 1, 1, 0, 0, 0], "indices": [0, 1, 2, 0, 2, 3]}]
            }
        }));
        let position = builder.accessor("VEC3", &[-1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0]);
        let indices = builder.indices(&[0, 1, 2, 0, 2, 3]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position }, "indices": indices, "material": 0 }] }]));
        builder.set("materials", serde_json::json!([{ "pbrMetallicRoughness": { "baseColorFactor": [0.2, 0.2, 0.9, 1.0] } }]));
        let model = Model::parse(&builder.glb()).unwrap();
        assert_eq!(model.particles.systems.len(), 7, "a system with no material is read, and draws nothing");
        let skinned = model.skinned().unwrap();
        let worlds = model.worlds(&model.rest());
        let mut particles = Particles::new(&model.particles, None);
        particles.step(&worlds, Mat4::IDENTITY, 0.02);
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [64, 64], 1.0).unwrap();
        let mut appearance = renderer.appearance(&model, &skinned).unwrap();
        appearance.update(&skinned.palette(&worlds, Mat4::IDENTITY), &skinned, &[]);
        appearance.place_particles(&mut particles, Mat4::IDENTITY);
        renderer.render(&appearance, [eye_projection(Vec3::new(0.0, 0.0, 3.0), 1.5, 1.5), None], Mat4::IDENTITY, 64).unwrap();
        let pixels = renderer.read().unwrap();
        let column = |x: f32| ((x * 1.2 + 1.5) / 3.0 * 64.0) as u32;
        let row = ((1.5 + 0.6 * 1.2) / 3.0 * 64.0) as u32;
        let seen = |x: f32| pixel(&pixels, 128, column(x), row);
        assert_eq!(seen(-0.8), [255, 255, 255, 255], "an opaque billboard covers its spot");
        assert_eq!(pixel(&pixels, 128, column(-0.8) + 3, row)[3], 255);
        assert!(seen(0.4)[3] > 0 && pixel(&pixels, 128, column(0.4) + 3, row)[3] == 0, "a quad past its share of the view's height is held to it");
        let added = seen(0.0);
        assert!(added[0] > 250 && added[1] < 5 && added[3] == 0, "an additive one adds its light and no coverage: {added:?}");
        let cut = seen(0.8);
        assert!(cut[1] > 250 && cut[0] < 5 && cut[3] == 255, "a cutout mesh covers its spot: {cut:?}");
        let multiplied = seen(-0.4);
        assert!(multiplied[0] > 250 && (100..220).contains(&multiplied[1]) && multiplied[1] == multiplied[2] && multiplied[3] == 255, "a half-covering multiply halves the white behind it toward red: {multiplied:?}");
        assert_eq!(pixel(&pixels, 128, 32, 63), [0; 4], "nothing below them");
        assert!(pixel(&pixels, 128, 32, 20)[2] > 100, "the figure still draws behind them");
        particles.reset();
        appearance.place_particles(&mut particles, Mat4::IDENTITY);
        renderer.render(&appearance, [eye_projection(Vec3::new(0.0, 0.0, 3.0), 1.5, 1.5), None], Mat4::IDENTITY, 64).unwrap();
        let cleared = renderer.read().unwrap();
        assert_eq!(pixel(&cleared, 128, ((-0.8 * 1.2 + 1.5) / 3.0 * 64.0) as u32, ((1.5 + 0.6 * 1.2) / 3.0 * 64.0) as u32), [0; 4], "cleared particles draw nothing");
    }

    #[test]
    fn a_layered_outline_takes_its_colour_from_the_layer_lit_flat() {
        let corners = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
        let drawn = |share: u8| {
            let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": {} } } }));
            let position = builder.accessor("VEC3", corners.as_flattened());
            let normal = builder.accessor("VEC3", corners.as_flattened());
            let indices = builder.indices(&[0, 2, 4, 2, 1, 4, 1, 3, 4, 3, 0, 4, 2, 0, 5, 1, 2, 5, 3, 1, 5, 0, 3, 5]);
            builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position, "NORMAL": normal }, "indices": indices, "material": 0 }] }]));
            builder.set("materials", serde_json::json!([{ "pbrMetallicRoughness": { "baseColorFactor": [1.0, 0.0, 0.0, 1.0] } }]));
            let mut model = Model::parse(&builder.glb()).unwrap();
            model.images.push(crate::vrm::Image { width: 1, height: 1, rgba: vec![0, 255, 0, share] });
            model.materials[0].outline = Some(crate::vrm::Outline { width: crate::vrm::OutlineWidth::World, factor: 0.2, width_image: Some(0), color: [1.0; 3], lighting_mix: 0.0, layered: true });
            let skinned = model.skinned().unwrap();
            let worlds = model.worlds(&model.rest());
            let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [64, 64], crate::HEIGHT / PAGE_HEIGHT).unwrap();
            let mut appearance = renderer.appearance(&model, &skinned).unwrap();
            appearance.update(&skinned.palette(&worlds, Mat4::from_scale(Vec3::splat(0.08))), &skinned, &[]);
            renderer.render(&appearance, [eye_projection(Vec3::new(0.05, 0.15, 0.4), 0.2, 0.2), None], Mat4::IDENTITY, 64).unwrap();
            renderer.read().unwrap().chunks_exact(4).map(|texel| [texel[0], texel[1], texel[2], texel[3]]).collect::<Vec<[u8; 4]>>()
        };
        let bare = drawn(0);
        let outlined = drawn(255);
        let hull: Vec<[u8; 4]> = bare.iter().zip(&outlined).filter(|(bare, _)| bare[3] == 0).map(|(_, outlined)| *outlined).filter(|texel| texel[3] == 255).collect();
        assert!(hull.len() > 100, "the layer's alpha widens the figure by {} texels", hull.len());
        assert!(hull.iter().all(|texel| texel[0] < 8 && texel[1] > 240), "the hull is the layer's green, lit flat to white and not by the red albedo: {:?}", &hull[..4]);
    }

    #[test]
    fn a_layered_outline_draws_no_hull_where_the_layer_gives_no_width() {
        let drawn = |share: u8| {
            let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": {} } } }));
            let position = builder.accessor("VEC3", &[-1.0, -1.0, 0.0, 0.0, 1.0, 0.0, 1.0, -1.0, 0.0]);
            let normal = builder.accessor("VEC3", &[0.0, 0.0, -1.0, 0.0, 0.0, -1.0, 0.0, 0.0, -1.0]);
            builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position, "NORMAL": normal }, "material": 0 }] }]));
            builder.set("materials", serde_json::json!([{ "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0] } }]));
            let mut model = Model::parse(&builder.glb()).unwrap();
            model.images.push(crate::vrm::Image { width: 1, height: 1, rgba: vec![0, 255, 0, share] });
            model.materials[0].outline = Some(crate::vrm::Outline { width: crate::vrm::OutlineWidth::World, factor: 0.2, width_image: Some(0), color: [1.0; 3], lighting_mix: 0.0, layered: true });
            let skinned = model.skinned().unwrap();
            let worlds = model.worlds(&model.rest());
            let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [32, 32], crate::HEIGHT / PAGE_HEIGHT).unwrap();
            let mut appearance = renderer.appearance(&model, &skinned).unwrap();
            appearance.update(&skinned.palette(&worlds, Mat4::from_scale(Vec3::splat(0.08))), &skinned, &[]);
            renderer.render(&appearance, [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None], Mat4::IDENTITY, 32).unwrap();
            renderer.read().unwrap().chunks_exact(4).filter(|texel| texel[3] != 0).count()
        };
        assert!(drawn(1) > 0, "a hull of any width shows its back face where the surface culls its own");
        assert_eq!(drawn(0), 0, "a zero-width vertex drops the hull's triangles");
    }

    #[test]
    fn the_two_eyes_see_the_figure_with_parallax() {
        let (pixels, row) = rendered(Version::One, [Vec3::new(-0.032, 0.0, 0.4), Vec3::new(0.032, 0.0, 0.4)]);
        let [left, right] = [0, 1].map(|eye| covered_columns(&pixels, row, eye, 58));
        assert!(!left.is_empty() && left.len() == right.len());
        assert_eq!(left, right, "geometry on the window plane has no disparity");
        let model = Model::parse(&figure(Version::One)).unwrap();
        let pixels = draw(&model, &model.rest(), [64, 64], [Vec3::new(-0.032, 0.0, 0.4), Vec3::new(0.032, 0.0, 0.4)].map(|eye| eye_projection(eye, 0.2, 0.2)), Vec3::new(0.0, 0.0, 0.05));
        let [left, right] = [0, 1].map(|eye| covered_columns(&pixels, row, eye, 58));
        assert!(left[0] > right[0], "in front of the window the left eye sees it further right: {left:?} {right:?}");
    }

    #[test]
    fn a_transparent_surface_that_writes_depth_hides_a_later_one_behind_it() {
        let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": {} } } }));
        let quad = |z: f32| [-1.0, 0.0, z, 1.0, 0.0, z, 1.0, 1.0, z, -1.0, 1.0, z];
        let positions: Vec<f32> = quad(0.5).iter().chain(&quad(0.0)).copied().collect();
        let position = builder.accessor("VEC3", &positions);
        let normal = builder.accessor("VEC3", &[0.0, 0.0, 1.0].repeat(8));
        let near = builder.indices(&[0, 1, 2, 0, 2, 3]);
        let far = builder.indices(&[4, 5, 6, 4, 6, 7]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [
            { "attributes": { "POSITION": position, "NORMAL": normal }, "indices": near, "material": 0 },
            { "attributes": { "POSITION": position, "NORMAL": normal }, "indices": far, "material": 1 }
        ] }]));
        let toon = |zwrite: bool, color: [f32; 4]| serde_json::json!({ "alphaMode": "BLEND", "pbrMetallicRoughness": { "baseColorFactor": color }, "extensions": { "VRMC_materials_mtoon": { "transparentWithZWrite": zwrite } } });
        let plain = |zwrite: bool, color: [f32; 4]| serde_json::json!({ "alphaMode": "BLEND", "pbrMetallicRoughness": { "baseColorFactor": color }, "extras": { "depthWrite": zwrite } });
        for material in [toon, plain] {
            let mut builder = builder.clone();
            builder.set("materials", serde_json::json!([material(true, [1.0, 0.0, 0.0, 0.5]), material(false, [0.0, 0.0, 1.0, 0.5])]));
            let model = Model::parse(&builder.glb()).unwrap();
            let pixels = draw(&model, &model.rest(), [32, 32], [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None], Vec3::ZERO);
            let center = pixel(&pixels, 64, 16, 16);
            assert!(center[0] > 100 && center[2] == 0, "only the near surface shows: {center:?}");
        }
    }

    #[test]
    fn a_plain_puppet_material_is_lit_by_the_key_light() {
        let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": {} } } }));
        let position = builder.accessor("VEC3", &[-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        let [toward, away] = [[0.4685, 0.7496, 0.5622], [-0.4685, -0.7496, 0.5622]];
        let normal = builder.accessor("VEC3", &[toward.repeat(4), away.repeat(4)].concat());
        let indices = builder.indices(&[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position, "NORMAL": normal }, "indices": indices, "material": 0 }] }]));
        builder.set("materials", serde_json::json!([{ "pbrMetallicRoughness": { "baseColorFactor": [0.3, 0.3, 0.3, 1.0] } }]));
        let model = Model::parse(&builder.glb()).unwrap();
        let pixels = draw(&model, &model.rest(), [32, 32], [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None], Vec3::ZERO);
        let [lit, dark] = [8, 24].map(|x| pixel(&pixels, 64, x, 16));
        assert!(lit[3] == 255 && dark[3] == 255, "both halves cover the eye: {lit:?} {dark:?}");
        assert!(u32::from(lit[0]) > 2 * u32::from(dark[0]), "the half facing the key light is brighter than the half facing away: {lit:?} {dark:?}");
    }

    #[test]
    fn a_new_appearance_draws_in_place_on_the_same_device() {
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [32, 32], crate::HEIGHT / PAGE_HEIGHT).unwrap();
        let eyes = [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None];
        let mut centre = |model: &Model, appearance: &mut Option<Appearance>| {
            let skinned = model.skinned().unwrap();
            let fit = crate::vrm::Fit::new(model, &skinned, crate::HEIGHT);
            let worlds = model.worlds(&model.rest());
            let loaded = renderer.appearance(model, &skinned).unwrap();
            let shown = appearance.insert(loaded);
            shown.update(&skinned.palette(&worlds, fit.placement(model, &worlds, crate::FLOOR)), &skinned, &[]);
            renderer.render(shown, eyes, Mat4::IDENTITY, 32).unwrap();
            pixel(&renderer.read().unwrap(), 64, 16, 16)
        };
        let mut shown = None;
        let red = centre(&crate::vrm::tests::plain([1.0, 0.0, 0.0, 1.0]), &mut shown);
        assert!(red[0] > 100 && red[2] == 0 && red[3] == 255, "{red:?}");
        let blue = centre(&crate::vrm::tests::plain([0.0, 0.0, 1.0, 1.0]), &mut shown);
        assert!(blue[2] > 100 && blue[0] == 0 && blue[3] == 255, "{blue:?}");
        let red = centre(&crate::vrm::tests::plain([1.0, 0.0, 0.0, 1.0]), &mut shown);
        assert!(red[0] > 100 && red[2] == 0, "and back: {red:?}");
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Golden {
        eye: [f32; 3],
        size: u32,
        time: f32,
        quad: f32,
        floor: f32,
        height: f32,
        page_height: f32,
    }

    fn golden() -> Golden {
        serde_json::from_str(include_str!("../golden/pose.json")).unwrap()
    }

    #[test]
    fn the_golden_pose_is_framed_as_the_host_frames_the_puppet() {
        let golden = golden();
        assert_eq!([golden.quad, golden.floor, golden.height, golden.page_height], [crate::QUAD, crate::FLOOR, crate::HEIGHT, PAGE_HEIGHT]);
    }

    fn golden_render(model: &Model, motion: &[u8]) -> Vec<u8> {
        let golden = golden();
        let standing = model.standing();
        let clip = crate::motion::Clip::parse(motion, model.version, model.rest_hips()).unwrap();
        let mut animator = crate::motion::Animator::new([("idle".to_owned(), clip)].into(), Vec::new(), crate::motion::Random::seeded());
        animator.update(golden.time);
        let mut pose = model.pose(&animator.humanoid(&standing, model.rest_hips()));
        model.express(&mut pose, "blink", 1.0);
        let eyes = [eye_projection(Vec3::from(golden.eye), golden.quad / 2.0, golden.quad / 2.0), None];
        let pixels = draw_at(model, &pose, [golden.size; 2], eyes, Vec3::ZERO, golden.height, golden.floor);
        let row = (golden.size * 8) as usize;
        pixels.chunks_exact(row).flat_map(|line| &line[..row / 2]).copied().collect()
    }

    /// A recorded transcript the page waves at, through the overlay's driver and the real classifier, to the figure on screen.
    #[test]
    fn a_recorded_transcript_the_page_waves_at_waves_the_overlays_figure() {
        use glam::Quat;
        assert!(crate::played("wave") && crate::played("idle"), "the overlay loads the gesture clips beside its idles");
        let replay = &crate::action::tests::replays()[0];
        let decisions = crate::action::tests::replay(replay);
        assert!(decisions.iter().any(|action| action.name == "wave"), "the page waves at this transcript: {decisions:?}");
        let model = Model::parse(include_bytes!("../golden/figure.vrm")).unwrap();
        let standing = model.standing();
        let raised = Quat::from_rotation_z(-1.2).to_array().repeat(2);
        let wave = serde_json::json!({ "name": "test wave", "duration": 2.0, "tracks": [{ "name": "rightUpperArm.quaternion", "times": [0.0, 2.0], "values": raised }] });
        let clips = |gestures: bool| {
            let idle = crate::motion::Clip::parse(include_bytes!("../golden/idle.json"), model.version, model.rest_hips()).unwrap();
            let mut clips = std::collections::HashMap::from([("idle".to_owned(), idle)]);
            if gestures {
                clips.insert("wave".to_owned(), crate::motion::Clip::parse(&serde_json::to_vec(&wave).unwrap(), model.version, model.rest_hips()).unwrap());
            }
            clips
        };
        let golden = golden();
        let eyes = [eye_projection(Vec3::from(golden.eye), golden.quad / 2.0, golden.quad / 2.0), None];
        let frame = |gestures: bool, after: f32| {
            let mut animator = crate::motion::Animator::new(clips(gestures), Vec::new(), crate::motion::Random::seeded());
            for action in &decisions {
                crate::act(&mut animator, action);
            }
            for _ in 0..(after / 0.02) as usize {
                animator.update(0.02);
            }
            let humanoid = animator.humanoid(&standing, model.rest_hips());
            let pose = model.pose(&humanoid);
            let pixels = draw_at(&model, &pose, [golden.size; 2], eyes, Vec3::ZERO, golden.height, golden.floor);
            let row = (golden.size * 8) as usize;
            (humanoid.rotations.get("rightUpperArm").copied(), pixels.chunks_exact(row).flat_map(|line| &line[..row / 2]).copied().collect::<Vec<u8>>())
        };
        let (arm, waving) = frame(true, 1.0);
        let (_, idle) = frame(false, 1.0);
        let (_, after) = frame(true, 3.0);
        if let Some(directory) = std::env::var_os("VOICE_VR_PROOF") {
            for (name, pixels) in [("idle", &idle), ("waving", &waving), ("after", &after)] {
                image::save_buffer(std::path::Path::new(&directory).join(format!("{name}.png")), pixels, golden.size, golden.size, image::ExtendedColorType::Rgba8).unwrap();
            }
        }
        let held = Quat::from_rotation_z(if model.version == Version::Zero { 1.2 } else { -1.2 });
        assert!(arm.unwrap().abs_diff_eq(held, 1e-4), "mid-gesture the arm follows the wave clip: {arm:?}");
        let moved = difference(&waving, &idle);
        assert!(moved.coverage < 0.8, "the waving figure differs from the idle one: overlap {:.4}", moved.coverage);
        let back = difference(&after, &idle);
        eprintln!("overlap with the idle figure: waving {:.4}, after {:.4}", moved.coverage, back.coverage);
        assert!(back.coverage > 0.95, "after the clip the figure is back in its idle: overlap {:.4}", back.coverage);
    }

    struct Difference {
        coverage: f32,
        color: f32,
    }

    fn difference(native: &[u8], page: &[u8]) -> Difference {
        let (mut both, mut either, mut error) = (0usize, 0usize, 0f64);
        for (a, b) in native.chunks_exact(4).zip(page.chunks_exact(4)) {
            let (in_a, in_b) = (a[3] > 127, b[3] > 127);
            either += (in_a || in_b) as usize;
            both += (in_a && in_b) as usize;
            if in_a || in_b {
                error += (0..3).map(|channel| (a[channel] as f64 - b[channel] as f64).abs()).sum::<f64>() / 3.0;
            }
        }
        Difference { coverage: both as f32 / either.max(1) as f32, color: (error / either.max(1) as f64) as f32 }
    }

    #[test]
    fn the_native_render_of_the_golden_pose_matches_the_page() {
        let model = Model::parse(include_bytes!("../golden/figure.vrm")).unwrap();
        let native = golden_render(&model, include_bytes!("../golden/idle.json"));
        let mut page = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&include_bytes!("../golden/page.rgba.gz")[..]), &mut page).unwrap();
        let difference = difference(&native, &page);
        eprintln!("golden: silhouette overlap {:.4}, mean colour error {:.2}/255", difference.coverage, difference.color);
        assert!(difference.coverage >= COVERAGE, "the native silhouette overlaps the page's by {:.4}", difference.coverage);
        assert!(difference.color <= COLOR, "the native colours differ from the page's by {:.2}/255 on average", difference.color);
    }
}
