struct Draw {
    view_projection: mat4x4<f32>,
    material: u32,
    outline: u32,
    outline_scale: f32,
    focal: f32,
}

struct Material {
    base: vec4<f32>,
    shade: vec3<f32>,
    blend: f32,
    shading_shift: f32,
    shading_toony: f32,
    cutoff: f32,
    equalization: f32,
    outline_color: vec3<f32>,
    outline_lighting_mix: f32,
    outline_width: f32,
    outline_screen: f32,
}

var<push_constant> draw: Draw;

@group(0) @binding(0) var base_texture: texture_2d<f32>;
@group(0) @binding(1) var shade_texture: texture_2d<f32>;
@group(0) @binding(6) var hue_mask: texture_2d<f32>;
struct Surface {
    hue: vec4<f32>,
    color: vec4<f32>,
    tone: vec4<f32>,
    decal: vec4<f32>,
    intensity: vec4<f32>,
    multiply: vec4<f32>,
}
@group(0) @binding(7) var<storage, read> surfaces: array<Surface>;
@group(0) @binding(8) var color_texture: texture_2d<f32>;
@group(0) @binding(9) var decal_layers: texture_2d_array<f32>;

@group(0) @binding(2) var texture_sampler: sampler;
@group(0) @binding(3) var outline_width_texture: texture_2d<f32>;
@group(0) @binding(4) var<storage, read> palette: array<mat4x4<f32>>;
@group(0) @binding(5) var<storage, read> materials: array<Material>;

struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
    @location(5) surface: u32,
}

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) surface: u32,
}

@vertex
fn vertex(input: Vertex) -> Varying {
    let skin = palette[input.joints.x] * input.weights.x + palette[input.joints.y] * input.weights.y + palette[input.joints.z] * input.weights.z + palette[input.joints.w] * input.weights.w;
    var position = (skin * vec4<f32>(input.position, 1.0)).xyz;
    let normal = normalize((skin * vec4<f32>(input.normal, 0.0)).xyz);
    var out: Varying;
    if draw.outline != 0u {
        let material = materials[draw.material];
        var width = material.outline_width * textureSampleLevel(outline_width_texture, texture_sampler, input.uv, 0.0).g;
        if material.outline_screen > 0.5 {
            width *= (draw.view_projection * vec4<f32>(position, 1.0)).w / abs(draw.focal);
        } else {
            width *= draw.outline_scale;
        }
        position += normal * width;
    }
    out.clip = draw.view_projection * vec4<f32>(position, 1.0);
    if draw.outline != 0u {
        // The hull sits a hair behind the surface it outlines, as three-vrm draws it.
        out.clip.z += 1e-6 * out.clip.w;
    }
    out.normal = normal;
    out.uv = input.uv;
    out.surface = input.surface;
    return out;
}

// The page's lights, divided by pi as three.js's Lambert term does.
const KEY_DIRECTION = vec3<f32>(0.4685, 0.7496, 0.5622);
const KEY_COLOR = vec3<f32>(0.987, 0.685, 0.514);
const RIM_DIRECTION = vec3<f32>(-0.7276, 0.4851, -0.4851);
const RIM_COLOR = vec3<f32>(0.157, 0.200, 0.764);
const SKY = vec3<f32>(0.596, 0.681, 0.859);
const GROUND = vec3<f32>(0.0064, 0.0048, 0.0106);

fn toon(material: Material, normal: vec3<f32>, direction: vec3<f32>) -> f32 {
    let toony = material.shading_toony;
    return clamp((dot(normal, direction) + material.shading_shift - (-1.0 + toony)) / max(2.0 - 2.0 * toony, 1e-4), 0.0, 1.0);
}

fn hue(color: vec3<f32>, settings: vec4<f32>) -> vec3<f32> {
    if settings.w == 0.0 { return color; }
    if settings.y > 1.5 {
        let axis = vec3<f32>(0.57735026919);
        let sine = sin(settings.x);
        let cosine = cos(settings.x);
        return color * cosine + cross(axis, color) * sine + axis * dot(axis, color) * (1.0 - cosine);
    }
    if settings.y > 0.5 {
        let high = max(color.r, max(color.g, color.b));
        let low = min(color.r, min(color.g, color.b));
        let delta = high - low;
        if delta < 1e-7 { return color; }
        var angle = (color.g - color.b) / delta;
        if high == color.g { angle = (color.b - color.r) / delta + 2.0; }
        else if high == color.b { angle = (color.r - color.g) / delta + 4.0; }
        angle = fract(angle / 6.0 * settings.z + settings.x);
        let rgb = clamp(abs(fract(vec3<f32>(angle) + vec3<f32>(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
        return vec3<f32>(low) + rgb * delta;
    }
    let lms = pow(max(vec3<f32>(
        dot(color, vec3<f32>(0.4122214708, 0.5363325363, 0.0514459929)),
        dot(color, vec3<f32>(0.2119034982, 0.6806995451, 0.1073969566)),
        dot(color, vec3<f32>(0.0883024619, 0.2817188376, 0.6299787005))), vec3<f32>(0.0)), vec3<f32>(1.0 / 3.0));
    let light = dot(lms, vec3<f32>(0.2104542553, 0.7936177850, -0.0040720468));
    var ab = vec2<f32>(dot(lms, vec3<f32>(1.9779984951, -2.4285922050, 0.4505937099)), dot(lms, vec3<f32>(0.0259040371, 0.7827717662, -0.8086757660)));
    if settings.z < 0.5 { ab = vec2<f32>(length(ab), 0.0); }
    let angle = settings.x * 6.28318530718;
    ab = vec2<f32>(cos(angle) * ab.x - sin(angle) * ab.y, sin(angle) * ab.x + cos(angle) * ab.y);
    let roots = vec3<f32>(light + 0.3963377774 * ab.x + 0.2158037573 * ab.y, light - 0.1055613458 * ab.x - 0.0638541728 * ab.y, light - 0.0894841775 * ab.x - 1.2914855480 * ab.y);
    let cubes = roots * roots * roots;
    return max(vec3<f32>(dot(cubes, vec3<f32>(4.0767416621, -3.3077115913, 0.2309699292)), dot(cubes, vec3<f32>(-1.2684380046, 2.6097574011, -0.3413193965)), dot(cubes, vec3<f32>(-0.0041960863, -0.7034186147, 1.7076147010))), vec3<f32>(0.0));
}

fn adjust(color: vec3<f32>, surface: Surface, mask: vec4<f32>) -> vec3<f32> {
    var result = mix(color, hue(color, surface.hue), mask.r);
    if surface.tone.w != 0.0 {
        result = mix(result, pow(abs(result), vec3<f32>(surface.tone.x)), mask.a);
        result = mix(result, vec3<f32>(dot(result, vec3<f32>(0.3, 0.59, 0.11))), -surface.tone.y * mask.b);
        result = clamp(mix(result, result * (surface.tone.z + 1.0), mask.g), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return result;
}

// Poiyomi lays its decals over the adjusted albedo in slot order; the shade takes them through its own tint.
fn decals(color: vec3<f32>, tint: vec3<f32>, surface: Surface, uv: vec2<f32>) -> vec3<f32> {
    var result = color;
    for (var slot = 0u; slot < min(textureNumLayers(decal_layers), 4u); slot++) {
        let layer = textureSample(decal_layers, texture_sampler, uv, slot);
        let laid = mix(layer.rgb * tint, result * layer.rgb, surface.multiply[slot]);
        result = mix(result, laid, clamp(layer.a * surface.decal[slot] * surface.intensity[slot], 0.0, 1.0));
    }
    return result;
}

@fragment
fn fragment(input: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let material = materials[draw.material];
    let surface = surfaces[input.surface];
    let raw = textureSample(color_texture, texture_sampler, input.uv).rgb * surface.color.rgb;
    let sampled = textureSample(base_texture, texture_sampler, input.uv) * material.base;
    let mask = textureSample(hue_mask, texture_sampler, input.uv);
    let base = vec4<f32>(decals(adjust(mix(sampled.rgb, raw * material.base.rgb, surface.color.a), surface, mask), vec3<f32>(1.0), surface, input.uv), sampled.a);
    if base.a < material.cutoff {
        discard;
    }
    let shade = decals(adjust(mix(textureSample(shade_texture, texture_sampler, input.uv).rgb, raw, surface.color.a) * material.shade, surface, mask), material.shade, surface, input.uv);
    var normal = normalize(input.normal);
    if !front {
        normal = -normal;
    }
    let direct = mix(shade, base.rgb, toon(material, normal, KEY_DIRECTION)) * KEY_COLOR + mix(shade, base.rgb, toon(material, normal, RIM_DIRECTION)) * RIM_COLOR;
    let hemisphere = mix(GROUND, SKY, 0.5 * normal.y + 0.5);
    let indirect = mix(hemisphere, 0.5 * (SKY + GROUND), material.equalization) * base.rgb;
    var color = direct + indirect;
    if draw.outline != 0u {
        color = material.outline_color * mix(vec3<f32>(1.0), color, material.outline_lighting_mix);
    }
    let alpha = select(1.0, base.a, material.blend > 0.5);
    return vec4<f32>(color * alpha, alpha);
}
