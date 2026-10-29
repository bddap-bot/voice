struct Draw {
    view_projection: mat4x4<f32>,
    camera: vec3<f32>,
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
    look: array<vec4<f32>, 10>,
    animated: vec4<u32>,
}
@group(0) @binding(7) var<storage, read> surfaces: array<Surface>;
@group(0) @binding(8) var color_texture: texture_2d<f32>;
@group(0) @binding(9) var decal_layers: texture_2d_array<f32>;

struct Matcap {
    color: vec4<f32>,
    hue: vec4<f32>,
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
}
struct Look {
    decal: vec4<f32>,
    intensity: vec4<f32>,
    multiply: vec4<f32>,
    emission_color: vec3<f32>,
    emission_strength: f32,
    emission_hue: f32,
    shifts_hue: f32,
    matcaps: array<Matcap, 4>,
}
@group(0) @binding(10) var look_index: texture_2d<f32>;
@group(0) @binding(11) var<storage, read> looks: array<Look>;
@group(0) @binding(12) var matcap_textures: texture_2d_array<f32>;
@group(0) @binding(13) var matcap_masks: texture_2d<f32>;
@group(0) @binding(14) var emission_map: texture_2d<f32>;

// Rows of Surface.look, in the order of expressions::Field.
const DECAL = 0u;
const DECAL_INTENSITY = 1u;
const EMISSION = 2u;
const EMISSION_HUE = 3u;
const MATCAP_HUE = 4u;
const MATCAP_INTENSITY = 5u;
const MATCAP_REPLACE = 6u;
const MATCAP_MULTIPLY = 7u;
const MATCAP_ADD = 8u;
const MATCAP_MIXED = 9u;

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
    @location(3) world: vec3<f32>,
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
    out.world = position;
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

// A clip animating a renderer's material value sets it for whichever material the renderer shows, as Unity does.
fn value(at: u32, row: u32, slot: u32, rest: f32) -> f32 {
    let bit = row * 4u + slot;
    return select(rest, surfaces[at].look[row][slot], ((surfaces[at].animated[bit / 32u] >> (bit % 32u)) & 1u) != 0u);
}

// Poiyomi lays its decals over the adjusted albedo in slot order; the shade takes them through its own tint.
fn decals(color: vec3<f32>, tint: vec3<f32>, at: u32, look: u32, uv: vec2<f32>) -> vec3<f32> {
    var result = color;
    for (var slot = 0u; slot < min(textureNumLayers(decal_layers), 4u); slot++) {
        let layer = textureSample(decal_layers, texture_sampler, uv, slot);
        let laid = mix(layer.rgb * tint, result * layer.rgb, looks[look].multiply[slot]);
        let alpha = value(at, DECAL, slot, looks[look].decal[slot]) * value(at, DECAL_INTENSITY, slot, looks[look].intensity[slot]);
        result = mix(result, laid, clamp(layer.a * alpha, 0.0, 1.0));
    }
    return result;
}

// Poiyomi's matcaps over the decalled albedo and shade, each sampled by the normal as seen from `view`; returns what
// they add to the light.
fn matcaps(base: ptr<function, vec3<f32>>, shade: ptr<function, vec3<f32>>, tint: vec3<f32>, at: u32, look: u32, uv: vec2<f32>, normal: vec3<f32>, view: vec3<f32>, lit: f32) -> vec3<f32> {
    var glow = vec3<f32>(0.0);
    let masks = textureSample(matcap_masks, texture_sampler, uv);
    let up = normalize(vec3<f32>(0.0, 1.0, 0.0) - view * dot(view, vec3<f32>(0.0, 1.0, 0.0)));
    let right = normalize(cross(view, up));
    let reflected = reflect(-view, normal);
    for (var slot = 0u; slot < 4u; slot++) {
        let matcap = looks[look].matcaps[slot];
        if matcap.on == 0u { continue; }
        let mapped = select(vec2<f32>(dot(right, normal), dot(up, normal)), reflected.xy, matcap.reflection != 0u) * matcap.border + 0.5;
        let sampled = select(vec4<f32>(1.0), textureSampleLevel(matcap_textures, texture_sampler, mapped, max(matcap.texture, 0), 0.0), matcap.texture >= 0) * matcap.color;
        let light = sampled.rgb * value(at, MATCAP_INTENSITY, slot, matcap.intensity);
        var mask = masks[slot] * mix(1.0, lit, matcap.light_mask);
        if matcap.circled != 0u {
            mask *= 1.0 - smoothstep(matcap.circle_border - 0.5 * matcap.circle_blur, matcap.circle_border + 0.5 * matcap.circle_blur, distance(mapped, vec2<f32>(0.5)));
        }
        let cover = sampled.a * mask;
        let hue_settings = vec4<f32>(value(at, MATCAP_HUE, slot, matcap.hue.x), matcap.hue.yzw);
        let replace = value(at, MATCAP_REPLACE, slot, matcap.replace) * cover;
        let multiply = value(at, MATCAP_MULTIPLY, slot, matcap.multiply) * cover;
        let add = value(at, MATCAP_ADD, slot, matcap.add) * cover;
        let mixed = value(at, MATCAP_MIXED, slot, matcap.mixed) * cover;
        let strengths = vec4<f32>(replace, multiply, add, matcap.screen * cover);
        let colored = hue(mix(light, light * *base, matcap.base_mix), hue_settings);
        *base = blend(*base, colored, vec3<f32>(1.0), strengths, mixed);
        *shade = blend(*shade, hue(mix(light, light * *shade, matcap.base_mix), hue_settings), tint, strengths, mixed);
        glow += colored * (matcap.light_add + matcap.emission) * cover;
    }
    return glow;
}

// Poiyomi's matcap blends, by strength: replace, multiply, add and screen, then mixed.
fn blend(color: vec3<f32>, light: vec3<f32>, tint: vec3<f32>, strengths: vec4<f32>, mixed: f32) -> vec3<f32> {
    var result = mix(color, light * tint, strengths.x);
    result *= mix(vec3<f32>(1.0), light, strengths.y);
    result += light * tint * strengths.z;
    result = mix(result, 1.0 - (1.0 - result) * (1.0 - light), strengths.w);
    return mix(result, result + result * light, mixed);
}

// VRChat Toon Standard's emission, turned by its hue shift under the hue mask.
fn emission(at: u32, look: u32, uv: vec2<f32>, mask: f32) -> vec3<f32> {
    let glow = textureSample(emission_map, texture_sampler, uv).rgb * looks[look].emission_color * value(at, EMISSION, 0u, looks[look].emission_strength);
    return mix(glow, hue(glow, vec4<f32>(value(at, EMISSION_HUE, 0u, looks[look].emission_hue), 2.0, 0.0, 1.0)), mask * looks[look].shifts_hue);
}

@fragment
fn fragment(input: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let material = materials[draw.material];
    let surface = surfaces[input.surface];
    let indices = vec2<f32>(textureDimensions(look_index));
    let look = u32(round(textureLoad(look_index, vec2<i32>(clamp(input.uv * indices, vec2<f32>(0.0), indices - 1.0)), 0).r * 255.0));
    var normal = normalize(input.normal);
    if !front {
        normal = -normal;
    }
    let view = normalize(draw.camera - input.world);
    let lit = toon(material, normal, KEY_DIRECTION);
    let raw = textureSample(color_texture, texture_sampler, input.uv).rgb * surface.color.rgb;
    let sampled = textureSample(base_texture, texture_sampler, input.uv) * material.base;
    let mask = textureSample(hue_mask, texture_sampler, input.uv);
    if sampled.a < material.cutoff {
        discard;
    }
    var albedo = decals(adjust(mix(sampled.rgb, raw * material.base.rgb, surface.color.a), surface, mask), vec3<f32>(1.0), input.surface, look, input.uv);
    var shade = decals(adjust(mix(textureSample(shade_texture, texture_sampler, input.uv).rgb, raw, surface.color.a) * material.shade, surface, mask), material.shade, input.surface, look, input.uv);
    let glow = matcaps(&albedo, &shade, material.shade, input.surface, look, input.uv, normal, view, lit);
    let base = vec4<f32>(albedo, sampled.a);
    let direct = mix(shade, base.rgb, lit) * KEY_COLOR + mix(shade, base.rgb, toon(material, normal, RIM_DIRECTION)) * RIM_COLOR;
    let hemisphere = mix(GROUND, SKY, 0.5 * normal.y + 0.5);
    let indirect = mix(hemisphere, 0.5 * (SKY + GROUND), material.equalization) * base.rgb;
    var color = direct + indirect + glow + emission(input.surface, look, input.uv, mask.r);
    if draw.outline != 0u {
        color = material.outline_color * mix(vec3<f32>(1.0), color, material.outline_lighting_mix);
    }
    let alpha = select(1.0, base.a, material.blend > 0.5);
    return vec4<f32>(color * alpha, alpha);
}
