struct Draw {
    view_projection: mat4x4<f32>,
    eye: vec3<f32>,
    now: f32,
    right: vec3<f32>,
    system: u32,
    up: vec3<f32>,
    focal: f32,
}

var<push_constant> draw: Draw;

struct System {
    emitter: mat4x4<f32>,
    tint: vec4<f32>,
    emission: vec3<f32>,
    cutoff: f32,
    uv: vec4<f32>,
    pivot: vec3<f32>,
    length_scale: f32,
    speed_ranges: vec4<f32>,
    velocity_scale: f32,
    noise_frequency: f32,
    damping: f32,
    flags: u32,
    mode: u32,
    align: u32,
    blend: u32,
    max_size: f32,
}

@group(0) @binding(0) var<storage, read> systems: array<System>;
@group(0) @binding(1) var<storage, read> curves: array<vec4<f32>>;
@group(0) @binding(2) var image: texture_2d<f32>;
@group(0) @binding(3) var image_sampler: sampler;

const VELOCITY = 1u;
const VELOCITY_WORLD = 2u;
const NOISE = 4u;
const LIMIT = 8u;
const SIZE_BY_SPEED = 16u;
const COLOUR_BY_SPEED = 32u;
const COLOUR_OVER_LIFE = 64u;
const SPIN = 128u;
const WORLD = 256u;

const SAMPLES = 16u;
const ROWS = 16u;
const GRAVITY = vec3<f32>(0.0, -9.81, 0.0);
const LIMIT_DRAW = 0.5;

struct Corner {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
}

struct Instance {
    @location(2) start: vec4<f32>,
    @location(3) velocity: vec4<f32>,
    @location(4) size: vec4<f32>,
    @location(5) turn: vec4<f32>,
    @location(6) color: vec4<f32>,
}

struct Varying {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

fn random(seed: f32, k: f32) -> f32 {
    return fract(sin(seed * 12.9898 + k * 78.233) * 43758.5453);
}

fn life_curve(row: u32, u: f32, k: f32) -> vec4<f32> {
    let x = clamp(u, 0.0, 1.0) * f32(SAMPLES - 1u);
    let i = min(u32(floor(x)), SAMPLES - 1u);
    let j = min(i + 1u, SAMPLES - 1u);
    let f = x - f32(i);
    let base = draw.system * ROWS * SAMPLES + row * SAMPLES;
    let low = mix(curves[base + i], curves[base + j], f);
    let high = mix(curves[base + SAMPLES + i], curves[base + SAMPLES + j], f);
    return mix(low, high, k);
}

fn euler(r: vec3<f32>) -> mat3x3<f32> {
    let cx = cos(r.x);
    let sx = sin(r.x);
    let cy = cos(r.y);
    let sy = sin(r.y);
    let cz = cos(r.z);
    let sz = sin(r.z);
    let rx = mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, cx, sx), vec3<f32>(0.0, -sx, cx));
    let ry = mat3x3<f32>(vec3<f32>(cy, 0.0, -sy), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(sy, 0.0, cy));
    let rz = mat3x3<f32>(vec3<f32>(cz, sz, 0.0), vec3<f32>(-sz, cz, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    return ry * rx * rz;
}

fn look_along(forward: vec3<f32>) -> mat3x3<f32> {
    let z = normalize(forward);
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(z.y) > 0.999);
    let x = normalize(cross(up, z));
    return mat3x3<f32>(x, cross(z, x), z);
}

fn has(flags: u32, bit: u32) -> bool {
    return (flags & bit) != 0u;
}

@vertex
fn vertex(corner: Corner, particle: Instance) -> Varying {
    let system = systems[draw.system];
    var out: Varying;
    out.uv = corner.uv;
    out.color = particle.color;
    let age = draw.now - particle.start.w;
    let life = particle.velocity.w;
    let seed = particle.size.w;
    let gravity = particle.turn.w;
    if age < 0.0 || age >= life {
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    let world_space = has(system.flags, WORLD);
    let u = age / life;
    let v0 = particle.velocity.xyz;
    let turn = mat3x3<f32>(system.emitter[0].xyz, system.emitter[1].xyz, system.emitter[2].xyz);
    let down = select(transpose(turn) * GRAVITY, GRAVITY, world_space);
    var moved = v0 * age;
    var velocity = v0;
    if has(system.flags, LIMIT) {
        let limit = life_curve(10u, 0.0, LIMIT_DRAW).y;
        let speed = length(v0);
        if speed > limit && system.damping > 0.0 {
            let held = v0 / speed * limit;
            let fade = exp(-system.damping * age);
            moved = held * age + (v0 - held) * (1.0 - fade) / system.damping;
            velocity = held + (v0 - held) * fade;
        }
    }
    moved += 0.5 * down * gravity * age * age;
    velocity += down * gravity * age;
    if has(system.flags, VELOCITY) {
        let k = random(seed, 1.0);
        let path = life * life_curve(4u, u, k).xyz;
        let now_v = life_curve(6u, u, k).xyz;
        let other = has(system.flags, VELOCITY_WORLD) != world_space;
        var into = transpose(turn);
        if world_space {
            into = turn;
        }
        moved += select(path, into * path, other);
        velocity += select(now_v, into * now_v, other);
    }
    if has(system.flags, NOISE) {
        let strength = life_curve(10u, u, random(seed, 5.0)).x;
        let w = 6.2831853 * max(system.noise_frequency, 0.05);
        let phase = vec3<f32>(random(seed, 6.0), random(seed, 7.0), random(seed, 8.0)) * 6.2831853;
        moved += strength / w * (cos(phase) - cos(w * age + phase));
        velocity += strength * sin(w * age + phase);
    }
    let at = particle.start.xyz + moved;
    let world_at = select((system.emitter * vec4<f32>(at, 1.0)).xyz, at, world_space);
    let world_velocity = select(turn * velocity, velocity, world_space);
    let speed = length(world_velocity);
    var size = particle.size.xyz * life_curve(2u, u, random(seed, 2.0)).xyz;
    if has(system.flags, SIZE_BY_SPEED) {
        size *= life_curve(14u, (speed - system.speed_ranges.x) / (system.speed_ranges.y - system.speed_ranges.x), random(seed, 9.0)).xyz;
    }
    var spun = particle.turn.xyz;
    if has(system.flags, SPIN) {
        spun += life * life_curve(8u, u, random(seed, 3.0)).xyz;
    }
    if has(system.flags, COLOUR_OVER_LIFE) {
        out.color *= life_curve(0u, u, random(seed, 10.0));
    }
    if has(system.flags, COLOUR_BY_SPEED) {
        out.color *= life_curve(12u, (speed - system.speed_ranges.z) / (system.speed_ranges.w - system.speed_ranges.z), random(seed, 11.0));
    }
    let to_eye = draw.eye - world_at;
    let eye_way = normalize(to_eye);
    let sideways = world_velocity - dot(world_velocity, eye_way) * eye_way;
    if system.mode != 4u {
        // Unity holds a quad to its share of the view's height at the particle's distance.
        size = min(size, vec3<f32>(system.max_size * 2.0 * length(to_eye) / draw.focal));
    }
    // Modes and alignments as particles::Mode and particles::Align order them.
    var frame = mat3x3<f32>(draw.right, draw.up, cross(draw.right, draw.up));
    if system.align == 4u && speed > 1e-6 {
        frame = look_along(world_velocity);
    } else if system.align == 2u {
        frame = turn;
    } else if system.align == 1u {
        frame = mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    } else if system.align == 3u {
        frame = look_along(to_eye);
    }
    let c = cos(spun.z);
    let s = sin(spun.z);
    let flat = corner.position.xy;
    let turned = mat2x2<f32>(vec2<f32>(c, s), vec2<f32>(-s, c)) * (flat * size.xy);
    var world: vec3<f32>;
    if system.mode == 4u {
        world = world_at + frame * (euler(spun) * ((corner.position + system.pivot) * size));
    } else if system.mode == 1u && length(sideways) > 1e-6 {
        let along = normalize(sideways);
        let across = normalize(cross(along, eye_way));
        let stretch = size.x * system.length_scale + speed * system.velocity_scale;
        world = world_at + across * flat.x * size.x + along * (flat.y - 0.5) * stretch;
    } else if system.mode == 2u {
        world = world_at + vec3<f32>(turned.x, 0.0, -turned.y);
    } else if system.mode == 3u {
        let level = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(to_eye.x, 0.0, to_eye.z), length(to_eye.xz) > 1e-6);
        let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), level));
        world = world_at + right * turned.x + vec3<f32>(0.0, turned.y, 0.0);
    } else {
        let spin = select(spun, vec3<f32>(0.0, 0.0, spun.z), system.align == 0u);
        world = world_at + frame * (euler(spin) * vec3<f32>(flat * size.xy, 0.0));
    }
    out.clip = draw.view_projection * vec4<f32>(world, 1.0);
    return out;
}

// Blends as particles::Blend orders them, into the premultiplied image the compositor takes.
@fragment
fn fragment(input: Varying) -> @location(0) vec4<f32> {
    let system = systems[draw.system];
    let unity = vec2<f32>(input.uv.x, 1.0 - input.uv.y) * system.uv.xy + system.uv.zw;
    let texel = textureSample(image, image_sampler, vec2<f32>(unity.x, 1.0 - unity.y));
    let c = input.color * system.tint * texel + vec4<f32>(system.emission, 0.0);
    switch system.blend {
        case 0u: { return vec4<f32>(c.rgb, 1.0); }
        case 1u: {
            if c.a < system.cutoff { discard; }
            return vec4<f32>(c.rgb, 1.0);
        }
        case 2u: { return vec4<f32>(c.rgb * c.a, c.a); }
        case 3u: { return vec4<f32>(c.rgb * c.a, 0.0); }
        case 5u: { return vec4<f32>(mix(vec3<f32>(1.0), c.rgb, c.a), c.a); }
        default: { return c; }
    }
}
