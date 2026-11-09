struct Draw {
    view_projection: mat4x4<f32>,
    camera: vec3<f32>,
    material: u32,
    outline: u32,
    outline_scale: f32,
    focal: f32,
    seconds: f32,
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
    look: array<vec4<f32>, 16>,
    animated: vec4<u32>,
}
@group(0) @binding(7) var<storage, read> surfaces: array<Surface>;
@group(0) @binding(8) var untinted_texture: texture_2d<f32>;
struct UntintedCell {
    atlas: vec4<f32>,
    image: vec4<f32>,
}
@group(0) @binding(16) var<storage, read> untinted_cells: array<UntintedCell>;
@group(0) @binding(9) var decal_pages: binding_array<texture_2d_array<f32>, 8>;

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
    theme: i32,
}
struct Theme {
    offsets: vec4<f32>,
    hsv: vec3<f32>,
    exposure: f32,
}
struct ClearCoat {
    levels: vec4<f32>,
    inverts: vec4<f32>,
    st: vec4<f32>,
    specular_tint: vec3<f32>,
    fresnel: f32,
    reflection_tint: vec3<f32>,
    map: i32,
    gsaa: vec2<f32>,
    on: u32,
}
struct Look {
    decal: vec4<f32>,
    intensity: vec4<f32>,
    blend: vec4<f32>,
    rect: vec4<f32>,
    page: vec4<i32>,
    layer: vec4<u32>,
    decal_hue: array<vec4<f32>, 4>,
    emission_color: vec3<f32>,
    emission_strength: f32,
    emission_hue: f32,
    shifts_hue: f32,
    flat: u32,
    light_cap: f32,
    themes: array<Theme, 4>,
    emission_theme: i32,
    matcaps: array<Matcap, 4>,
    ramp: array<vec4<f32>, 4>,
    ramped: u32,
    shadow_tint: f32,
    coat: ClearCoat,
}
@group(0) @binding(10) var look_index: texture_2d<f32>;
@group(0) @binding(11) var<storage, read> looks: array<Look>;
@group(0) @binding(12) var matcap_textures: texture_2d_array<f32>;
@group(0) @binding(13) var matcap_masks: texture_2d<f32>;
@group(0) @binding(14) var emission_map: texture_2d<f32>;
@group(0) @binding(15) var edge_sampler: sampler;
@group(0) @binding(17) var clearcoat_maps: texture_2d_array<f32>;

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
const DECAL_HUE = 10u;
const THEME_HUE = 11u;
const THEME_SATURATION = 12u;
const THEME_VALUE = 13u;
const THEME_SPEED = 14u;
const CLEARCOAT_ST = 15u;

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
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) surface: u32,
    @location(3) world: vec3<f32>,
}

struct CoatedVarying {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) surface: u32,
    @location(3) world: vec3<f32>,
    @location(4) coat_uv: vec2<f32>,
}

@vertex
fn vertex(input: Vertex) -> Varying {
    return skin(input);
}

@vertex
fn coated_vertex(input: Vertex, @location(6) coat_uv: vec2<f32>) -> CoatedVarying {
    let out = skin(input);
    return CoatedVarying(out.clip, out.normal, out.uv, out.surface, out.world, coat_uv);
}

fn skin(input: Vertex) -> Varying {
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

fn uncoated(input: CoatedVarying) -> Varying {
    return Varying(input.clip, input.normal, input.uv, input.surface, input.world);
}

// The page's lights, divided by pi as three.js's Lambert term does.
// The points a ramp-lit look's ramp holds, render.rs RAMP.
const RAMP = 16u;
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

fn untinted(uv: vec2<f32>, gradient: mat2x2<f32>) -> vec3<f32> {
    for (var i = 0u; i < arrayLength(&untinted_cells); i++) {
        let cell = untinted_cells[i];
        if all(uv >= cell.atlas.xy) && all(uv <= cell.atlas.zw) {
            let to = (cell.image.zw - cell.image.xy) / (cell.atlas.zw - cell.atlas.xy);
            return textureSampleGrad(untinted_texture, texture_sampler, cell.image.xy + (uv - cell.atlas.xy) * to, gradient[0] * to, gradient[1] * to).rgb;
        }
    }
    return vec3<f32>(1.0);
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

// How many pages a pipeline samples: build.rs compiles one fragment shader per count, since every sampled page costs the
// driver's compiler whether a figure fills it or not.
override decal_pages_used: u32;

// Pages are indexed by constants alone: indexing them by a per-texel value needs a device feature the overlay does not ask for.
fn decal(page: i32, layer: u32, uv: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    switch page {
        case 0: { if 0u < decal_pages_used { return textureSampleGrad(decal_pages[0], edge_sampler, uv, layer, dx, dy); } }
        case 1: { if 1u < decal_pages_used { return textureSampleGrad(decal_pages[1], edge_sampler, uv, layer, dx, dy); } }
        case 2: { if 2u < decal_pages_used { return textureSampleGrad(decal_pages[2], edge_sampler, uv, layer, dx, dy); } }
        case 3: { if 3u < decal_pages_used { return textureSampleGrad(decal_pages[3], edge_sampler, uv, layer, dx, dy); } }
        case 4: { if 4u < decal_pages_used { return textureSampleGrad(decal_pages[4], edge_sampler, uv, layer, dx, dy); } }
        case 5: { if 5u < decal_pages_used { return textureSampleGrad(decal_pages[5], edge_sampler, uv, layer, dx, dy); } }
        case 6: { if 6u < decal_pages_used { return textureSampleGrad(decal_pages[6], edge_sampler, uv, layer, dx, dy); } }
        case 7: { if 7u < decal_pages_used { return textureSampleGrad(decal_pages[7], edge_sampler, uv, layer, dx, dy); } }
        default: {}
    }
    return vec4<f32>(0.0);
}

// The albedo and the shade share one sample of each decal.
fn decal_samples(at: u32, look: u32, uv: vec2<f32>) -> array<vec4<f32>, 4> {
    var samples: array<vec4<f32>, 4>;
    let rect = looks[look].rect;
    let size = rect.zw - rect.xy;
    let local = (uv - rect.xy) / size;
    let dx = dpdx(uv) / size;
    let dy = dpdy(uv) / size;
    for (var slot = 0u; slot < 4u; slot++) {
        if value(at, DECAL, slot, looks[look].decal[slot]) * value(at, DECAL_INTENSITY, slot, looks[look].intensity[slot]) == 0.0 { continue; }
        samples[slot] = decal(looks[look].page[slot], looks[look].layer[slot], local, dx, dy);
    }
    return samples;
}

// Poiyomi's decal Color Blend Mode `mode` (its _DecalBlendType) of `laid` over `base`, a colour times `tint`, as
// that colour blended and then times `tint`: the albedo's tint is one, the shade's its own, so each mode is written
// without dividing the tint back out.
fn decal_blend(base: vec3<f32>, laid: vec3<f32>, tint: vec3<f32>, mode: u32) -> vec3<f32> {
    let over = laid * tint;
    switch mode {
        case 1u: { return min(base, over); }
        case 2u: { return base * laid; }
        case 5u: { return max(base, over); }
        case 6u: { return base + over - base * laid; }
        case 7u: { return max(base - over, vec3<f32>(0.0)); }
        case 8u: { return base + over; }
        case 9u: { return select(tint - 2.0 * (tint - base) * (1.0 - laid), 2.0 * base * laid, base < 0.5 * tint); }
        case 20u: { return base + base * laid; }
        default: { return over; }
    }
}

// Poiyomi lays its decals over the adjusted albedo in slot order; the shade takes them through its own tint.
fn decals(color: vec3<f32>, tint: vec3<f32>, at: u32, look: u32, samples: array<vec4<f32>, 4>) -> vec3<f32> {
    var result = color;
    for (var slot = 0u; slot < 4u; slot++) {
        let layer = samples[slot];
        let turn = looks[look].decal_hue[slot];
        let colour = hue(layer.rgb, vec4<f32>(value(at, DECAL_HUE, slot, turn.x), turn.yzw));
        let laid = decal_blend(result, colour, tint, u32(looks[look].blend[slot] + 0.5));
        let alpha = value(at, DECAL, slot, looks[look].decal[slot]) * value(at, DECAL_INTENSITY, slot, looks[look].intensity[slot]);
        result = mix(result, laid, clamp(layer.a * alpha, 0.0, 1.0));
    }
    return result;
}

// Poiyomi's matcaps over the decalled albedo and shade, each sampled by the normal as seen from `view`; returns what
// they add to the light.
fn matcaps(base: ptr<function, vec3<f32>>, shade: ptr<function, vec3<f32>>, tint: vec3<f32>, at: u32, look: u32, uv: vec2<f32>, gradient: mat2x2<f32>, normal: vec3<f32>, view: vec3<f32>, lit: f32) -> vec3<f32> {
    var glow = vec3<f32>(0.0);
    if (looks[look].matcaps[0].on | looks[look].matcaps[1].on | looks[look].matcaps[2].on | looks[look].matcaps[3].on) == 0u { return glow; }
    let masks = textureSampleGrad(matcap_masks, texture_sampler, uv, gradient[0], gradient[1]);
    let up = normalize(vec3<f32>(0.0, 1.0, 0.0) - view * dot(view, vec3<f32>(0.0, 1.0, 0.0)));
    let right = normalize(cross(view, up));
    let reflected = reflect(-view, normal);
    for (var slot = 0u; slot < 4u; slot++) {
        let matcap = looks[look].matcaps[slot];
        if matcap.on == 0u { continue; }
        let replace_at = value(at, MATCAP_REPLACE, slot, matcap.replace);
        let multiply_at = value(at, MATCAP_MULTIPLY, slot, matcap.multiply);
        let add_at = value(at, MATCAP_ADD, slot, matcap.add);
        let mixed_at = value(at, MATCAP_MIXED, slot, matcap.mixed);
        if replace_at == 0.0 && multiply_at == 0.0 && add_at == 0.0 && mixed_at == 0.0 && matcap.screen == 0.0 && matcap.light_add + matcap.emission == 0.0 { continue; }
        let mapped = select(vec2<f32>(dot(right, normal), dot(up, normal)), reflected.xy, matcap.reflection != 0u) * matcap.border + 0.5;
        var color = matcap.color;
        if matcap.theme >= 0 { color = vec4<f32>(theme(at, look, u32(matcap.theme)), color.a); }
        let sampled = select(vec4<f32>(1.0), textureSampleLevel(matcap_textures, texture_sampler, mapped, max(matcap.texture, 0), 0.0), matcap.texture >= 0) * color;
        let light = sampled.rgb * value(at, MATCAP_INTENSITY, slot, matcap.intensity);
        var mask = masks[slot] * mix(1.0, lit, matcap.light_mask);
        if matcap.circled != 0u {
            mask *= 1.0 - smoothstep(matcap.circle_border - 0.5 * matcap.circle_blur, matcap.circle_border + 0.5 * matcap.circle_blur, distance(mapped, vec2<f32>(0.5)));
        }
        let cover = sampled.a * mask;
        let hue_settings = vec4<f32>(value(at, MATCAP_HUE, slot, matcap.hue.x), matcap.hue.yzw);
        let replace = replace_at * cover;
        let multiply = multiply_at * cover;
        let add = add_at * cover;
        let mixed = mixed_at * cover;
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

// A Poiyomi global theme colour moved by its offsets as Poiyomi's ModifyViaHSV moves it, its hue also turning at its
// speed a turn every twenty seconds, as Unity's _Time.x counts them, and its exposure put back.
fn theme(at: u32, look: u32, slot: u32) -> vec3<f32> {
    let rest = looks[look].themes[slot].offsets;
    let turn = value(at, THEME_HUE, slot, rest.x) + value(at, THEME_SPEED, slot, rest.w) * draw.seconds / 20.0;
    let hsv = looks[look].themes[slot].hsv;
    let h = fract(hsv.x + fract(turn));
    let s = saturate(hsv.y + value(at, THEME_SATURATION, slot, rest.y));
    let v = saturate(hsv.z + value(at, THEME_VALUE, slot, rest.z));
    let rgb = clamp(abs(fract(vec3<f32>(h) + vec3<f32>(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
    return v * mix(vec3<f32>(1.0), rgb, s) * looks[look].themes[slot].exposure;
}

// VRChat Toon Standard's emission, turned by its hue shift under the hue mask, or Poiyomi's in its theme colour.
fn emission(at: u32, look: u32, uv: vec2<f32>, gradient: mat2x2<f32>, mask: f32) -> vec3<f32> {
    let strength = value(at, EMISSION, 0u, looks[look].emission_strength);
    if strength == 0.0 { return vec3<f32>(0.0); }
    var color = looks[look].emission_color;
    if looks[look].emission_theme >= 0 { color = theme(at, look, u32(looks[look].emission_theme)); }
    let glow = textureSampleGrad(emission_map, texture_sampler, uv, gradient[0], gradient[1]).rgb * color * strength;
    return mix(glow, hue(glow, vec4<f32>(value(at, EMISSION_HUE, 0u, looks[look].emission_hue), 2.0, 0.0, 1.0)), mask * looks[look].shifts_hue);
}

const DIELECTRIC = 0.220916301;

fn pow5(x: f32) -> f32 {
    let square = x * x;
    return square * square * x;
}

// Poiyomi's clear coat: GGX's highlight of the key light and a reflection of the sky and ground, blurred toward their mean
// as it roughens in place of Unity's reflection probe.
fn coat(at: u32, look: u32, uv: vec2<f32>, gradient: mat2x2<f32>, normal: vec3<f32>, curvature: f32, view: vec3<f32>) -> vec3<f32> {
    let layer = looks[look].coat;
    if layer.on == 0u { return vec3<f32>(0.0); }
    var levels = layer.levels;
    if layer.map >= 0 {
        let st = vec4<f32>(value(at, CLEARCOAT_ST, 0u, layer.st.x), value(at, CLEARCOAT_ST, 1u, layer.st.y), value(at, CLEARCOAT_ST, 2u, layer.st.z), value(at, CLEARCOAT_ST, 3u, layer.st.w));
        let unity = vec2<f32>(uv.x, 1.0 - uv.y) * st.xy + st.zw;
        levels *= textureSampleGrad(clearcoat_maps, texture_sampler, vec2<f32>(unity.x, 1.0 - unity.y), layer.map, gradient[0] * st.xy, gradient[1] * st.xy);
    }
    levels = mix(levels, 1.0 - levels, layer.inverts);
    let perceptual = 1.0 - levels.y;
    // Filament's geometric specular antialiasing: roughness widened by the normal's change across the pixel.
    let kernel = min(2.0 * layer.gsaa.x * curvature, layer.gsaa.y);
    let widened = sqrt(sqrt(saturate(perceptual * perceptual * perceptual * perceptual + kernel)));
    let rough = max(widened * widened, 0.002);
    let half_way = normalize(KEY_DIRECTION + view);
    let nl = saturate(dot(normal, KEY_DIRECTION));
    let nv = saturate(dot(normal, view));
    let nh = saturate(dot(normal, half_way));
    let lh = saturate(dot(KEY_DIRECTION, half_way));
    var ggx = 0.0;
    if nl > 0.0 {
        let r2 = rough * rough;
        let visible = 0.5 / (nl * (nv * (1.0 - rough) + rough) + nv * (nl * (1.0 - rough) + rough) + 1e-5);
        let d = (nh * r2 - nh) * nh + 1.0;
        ggx = visible * r2 / (d * d + 1e-7);
    }
    let fresnel = DIELECTRIC + (1.0 - DIELECTRIC) * pow5(1.0 - lh);
    let highlight = KEY_COLOR * max(0.0, ggx * max(1e-5, nl)) * fresnel * levels.w * layer.specular_tint * nl;
    let reflected = reflect(-view, normal);
    let environment = mix(mix(GROUND, SKY, 0.5 * reflected.y + 0.5), 0.5 * (SKY + GROUND), perceptual * (1.7 - 0.7 * perceptual));
    let glancing = mix(DIELECTRIC, saturate(levels.y + 0.04), pow5(1.0 - nv));
    let reflection = environment / (rough * rough + 1.0) * mix(1.0, glancing, layer.fresnel) * layer.reflection_tint * levels.z;
    return saturate((highlight + reflection) * levels.x);
}

// A fragment shader that may discard runs before its depth test, so shading never discards: an alpha-tested surface
// that writes depth is drawn first by `depth`, then shaded where its own depth is the nearest.
@fragment
fn fragment(input: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shaded(input, facing(input, front), vec3<f32>(0.0));
}

@fragment
fn depth(input: Varying) {
    if covered(input) < materials[draw.material].cutoff {
        discard;
    }
}

@fragment
fn cutout(input: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if covered(input) < materials[draw.material].cutoff {
        discard;
    }
    return shaded(input, facing(input, front), vec3<f32>(0.0));
}

@fragment
fn coated_fragment(input: CoatedVarying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return coated(input, front);
}

@fragment
fn coated_cutout(input: CoatedVarying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if covered(uncoated(input)) < materials[draw.material].cutoff {
        discard;
    }
    return coated(input, front);
}

fn coated(input: CoatedVarying, front: bool) -> vec4<f32> {
    let gradient = mat2x2<f32>(dpdx(input.coat_uv), dpdy(input.coat_uv));
    let plain = uncoated(input);
    let at = facing(plain, front);
    let du = dpdx(at.normal);
    let dv = dpdy(at.normal);
    return shaded(plain, at, coat(input.surface, at.look, input.coat_uv, gradient, at.normal, dot(du, du) + dot(dv, dv), at.view));
}

fn covered(input: Varying) -> f32 {
    return textureSample(base_texture, texture_sampler, input.uv).a * materials[draw.material].base.a;
}

struct Facing {
    look: u32,
    normal: vec3<f32>,
    view: vec3<f32>,
}

fn facing(input: Varying, front: bool) -> Facing {
    let indices = vec2<f32>(textureDimensions(look_index));
    let look = u32(round(textureLoad(look_index, vec2<i32>(clamp(input.uv * indices, vec2<f32>(0.0), indices - 1.0)), 0).r * 255.0));
    var normal = normalize(input.normal);
    if !front {
        normal = -normal;
    }
    return Facing(look, normal, normalize(draw.camera - input.world));
}

fn shaded(input: Varying, at: Facing, sheen: vec3<f32>) -> vec4<f32> {
    // Taken before any branch on the look, which differs within a quad along a look's edge.
    let gradient = mat2x2<f32>(dpdx(input.uv), dpdy(input.uv));
    let material = materials[draw.material];
    let surface = surfaces[input.surface];
    let look = at.look;
    let normal = at.normal;
    let view = at.view;
    let lit = toon(material, normal, KEY_DIRECTION);
    var raw = surface.color.rgb;
    if surface.color.a > 0.0 {
        raw *= untinted(input.uv, gradient);
    }
    let sampled = textureSample(base_texture, texture_sampler, input.uv) * material.base;
    let mask = textureSample(hue_mask, texture_sampler, input.uv);
    let laid = decal_samples(input.surface, look, input.uv);
    var albedo = decals(adjust(mix(sampled.rgb, raw * material.base.rgb, surface.color.a), surface, mask), vec3<f32>(1.0), input.surface, look, laid);
    var shade = decals(adjust(mix(textureSample(shade_texture, texture_sampler, input.uv).rgb, raw, surface.color.a) * material.shade, surface, mask), material.shade, input.surface, look, laid);
    let glow = matcaps(&albedo, &shade, material.shade, input.surface, look, input.uv, gradient, normal, view, lit);
    let base = vec4<f32>(albedo, sampled.a);
    var light: vec3<f32>;
    // A flat material takes its main light's colour whole, as a Lambert surface square to it would, and the ambient
    // light, whichever way it faces: Unity's base pass, which lights it by the one main directional light.
    if looks[look].flat != 0u {
        light = min(KEY_COLOR + 0.5 * (SKY + GROUND), vec3<f32>(looks[look].light_cap)) * base.rgb;
    } else if looks[look].ramped != 0u {
        // Toon Standard's base pass: its main light through its ramp at the half-Lambert term, the shade lent the
        // albedo by its shadow tint, and the ambient, capped where it limits brightness.
        let x = clamp(dot(normal, KEY_DIRECTION) * 0.5 + 0.5, 0.0, 1.0) * f32(RAMP - 1u);
        let i = min(u32(x), RAMP - 2u);
        let j = i + 1u;
        let ramp = mix(looks[look].ramp[i / 4u][i % 4u], looks[look].ramp[j / 4u][j % 4u], x - f32(i));
        let level = ramp + (1.0 - ramp) * looks[look].shadow_tint * base.rgb;
        light = min(level * KEY_COLOR + 0.5 * (SKY + GROUND), vec3<f32>(looks[look].light_cap)) * base.rgb;
    } else {
        let direct = mix(shade, base.rgb, lit) * KEY_COLOR + mix(shade, base.rgb, toon(material, normal, RIM_DIRECTION)) * RIM_COLOR;
        let hemisphere = mix(GROUND, SKY, 0.5 * normal.y + 0.5);
        light = direct + mix(hemisphere, 0.5 * (SKY + GROUND), material.equalization) * base.rgb;
    }
    var color = light + glow + emission(input.surface, look, input.uv, gradient, mask.r) + sheen;
    if draw.outline != 0u {
        color = material.outline_color * mix(vec3<f32>(1.0), color, material.outline_lighting_mix);
    }
    let alpha = select(1.0, base.a, material.blend > 0.5);
    return vec4<f32>(color * alpha, alpha);
}
