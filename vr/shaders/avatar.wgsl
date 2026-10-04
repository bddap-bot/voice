struct Draw {
    view_projection: mat4x4<f32>,
    material: u32,
    outline: u32,
    outline_scale: f32,
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
}

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
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
            width *= (draw.view_projection * vec4<f32>(position, 1.0)).w / abs(draw.view_projection[1][1]);
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

@fragment
fn fragment(input: Varying, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let material = materials[draw.material];
    let base = textureSample(base_texture, texture_sampler, input.uv) * material.base;
    if base.a < material.cutoff {
        discard;
    }
    let shade = textureSample(shade_texture, texture_sampler, input.uv).rgb * material.shade;
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
