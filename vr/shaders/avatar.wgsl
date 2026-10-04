struct Draw {
    view_projection: mat4x4<f32>,
    base: vec4<f32>,
    shade: vec3<f32>,
    blend: f32,
    shading_shift: f32,
    shading_toony: f32,
    cutoff: f32,
    equalization: f32,
}

var<push_constant> draw: Draw;

@group(0) @binding(0) var base_texture: texture_2d<f32>;
@group(0) @binding(1) var shade_texture: texture_2d<f32>;
@group(0) @binding(2) var texture_sampler: sampler;

struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
}

@vertex
fn vertex(input: Vertex) -> Varying {
    var out: Varying;
    out.clip = draw.view_projection * vec4<f32>(input.position, 1.0);
    out.normal = input.normal;
    out.uv = input.uv;
    return out;
}

// The page's lights, divided by pi as three.js's Lambert term does.
const KEY_DIRECTION = vec3<f32>(0.4685, 0.7496, 0.5622);
const KEY_COLOR = vec3<f32>(0.987, 0.685, 0.514);
const RIM_DIRECTION = vec3<f32>(-0.7276, 0.4851, -0.4851);
const RIM_COLOR = vec3<f32>(0.157, 0.200, 0.764);
const SKY = vec3<f32>(0.596, 0.681, 0.859);
const GROUND = vec3<f32>(0.0064, 0.0048, 0.0106);

fn toon(normal: vec3<f32>, direction: vec3<f32>) -> f32 {
    let toony = draw.shading_toony;
    return clamp((dot(normal, direction) + draw.shading_shift - (-1.0 + toony)) / max(2.0 - 2.0 * toony, 1e-4), 0.0, 1.0);
}

@fragment
fn fragment(input: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let base = textureSample(base_texture, texture_sampler, input.uv) * draw.base;
    if base.a < draw.cutoff {
        discard;
    }
    let shade = textureSample(shade_texture, texture_sampler, input.uv).rgb * draw.shade;
    var normal = normalize(input.normal);
    if !front {
        normal = -normal;
    }
    let direct = mix(shade, base.rgb, toon(normal, KEY_DIRECTION)) * KEY_COLOR + mix(shade, base.rgb, toon(normal, RIM_DIRECTION)) * RIM_COLOR;
    let hemisphere = mix(GROUND, SKY, 0.5 * normal.y + 0.5);
    let indirect = mix(hemisphere, 0.5 * (SKY + GROUND), draw.equalization) * base.rgb;
    let alpha = select(1.0, base.a, draw.blend > 0.5);
    return vec4<f32>((direct + indirect) * alpha, alpha);
}
