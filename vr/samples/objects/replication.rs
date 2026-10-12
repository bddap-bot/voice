type V = [f32; 3];

const SIDES: [f32; 2] = [-1.0, 1.0];
const LEADING: usize = 0;
const LAGGING: usize = 1;
const GROW: f32 = 0.4;
const PRIMER_STEP: f32 = 0.4;
const POLYMERASE_STEP: f32 = 0.8;
const BIND: f32 = 0.2;
const FRAGMENTS: [usize; 4] = [scene::OKAZAKI_FRAGMENT_1, scene::OKAZAKI_FRAGMENT_2, scene::OKAZAKI_FRAGMENT_3, scene::OKAZAKI_FRAGMENT_4];

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn mul(a: V, s: f32) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn lerp(a: V, b: V, t: f32) -> V {
    add(a, mul(sub(b, a), t))
}

fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn length(a: V) -> f32 {
    sqrt(dot(a, a))
}

fn unit(a: V) -> V {
    mul(a, 1.0 / length(a).max(1e-9))
}

/// The rotation whose local x and y are these directions, y kept exact.
fn frame(x: V, y: V) -> [f32; 4] {
    let y = unit(y);
    let x = unit(sub(x, mul(y, dot(x, y))));
    let z = cross(x, y);
    let (m00, m11, m22) = (x[0], y[1], z[2]);
    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = 2.0 * sqrt(trace + 1.0);
        [(y[2] - z[1]) / s, (z[0] - x[2]) / s, (x[1] - y[0]) / s, s / 4.0]
    } else if m00 > m11 && m00 > m22 {
        let s = 2.0 * sqrt(1.0 + m00 - m11 - m22);
        [s / 4.0, (y[0] + x[1]) / s, (z[0] + x[2]) / s, (y[2] - z[1]) / s]
    } else if m11 > m22 {
        let s = 2.0 * sqrt(1.0 + m11 - m00 - m22);
        [(y[0] + x[1]) / s, s / 4.0, (z[1] + y[2]) / s, (z[0] - x[2]) / s]
    } else {
        let s = 2.0 * sqrt(1.0 + m22 - m00 - m11);
        [(z[0] + x[2]) / s, (z[1] + y[2]) / s, s / 4.0, (x[1] - y[0]) / s]
    }
}

fn apart(i: f32, fork: f32) -> f32 {
    smoothstep(0.0, scene::FLARE, fork - i)
}

/// Where base pair `i`'s backbone sits on strand position `angle` (0 or 1) of daughter `side`; unreplicated pairs are the parental duplex, where both daughters coincide.
fn slot(side: usize, angle: usize, i: f32, fork: f32) -> V {
    let w = apart(i, fork);
    let theta = scene::PHASE - i * scene::TWIST + angle as f32 * scene::GROOVE;
    [SIDES[side] * scene::OFFSET * w + scene::RADIUS * cos(theta), scene::TOP - i * scene::RISE + scene::LIFT * w, -scene::RADIUS * sin(theta)]
}

fn axis(side: usize, i: f32, fork: f32) -> V {
    let w = apart(i, fork);
    [SIDES[side] * scene::OFFSET * w, scene::TOP - i * scene::RISE + scene::LIFT * w, 0.0]
}

fn beside(side: usize, angle: usize, i: f32, fork: f32, by: f32) -> V {
    let at = slot(side, angle, i, fork);
    let out = sub(at, axis(side, i, fork));
    add(at, mul(unit([out[0] + SIDES[side] * scene::RADIUS * 1.5, 0.0, out[2]]), by))
}

fn nucleotide(node: usize, side: usize, angle: usize, i: usize, link: i32, bonded: bool, size: f32, fork: f32) {
    let s = i as f32;
    let here = slot(side, angle, s, fork);
    let partner = slot(side, 1 - angle, s, fork);
    let (toward, stretch) = if bonded {
        let to = sub(slot(side, angle, s + link as f32, fork), here);
        (to, length(to) / scene::LINK)
    } else {
        (sub(here, slot(side, angle, s - link as f32, fork)), 1.0)
    };
    place(rest(node), here, frame(sub(mul(add(here, partner), 0.5), here), toward), [size, size * stretch, size]);
}

fn grow(at: f32, progress: f32) -> f32 {
    smoothstep(at, at + GROW, progress)
}

fn show(node: usize, at: V, rotation: [f32; 4], size: f32) {
    place(rest(node), at, rotation, [size, size, size]);
}

/// Fragment `k`'s polymerase lands on its primer's 3' end, then walks away from the fork one nucleotide per step, through the previous fragment's primer.
fn lagging_start(k: usize) -> f32 {
    ((k + 1) * scene::FRAGMENT as usize) as f32 - 1.0 + scene::LAG + BIND
}

fn lagging_steps(k: usize) -> f32 {
    scene::FRAGMENT - scene::PRIMER + if k > 0 { scene::PRIMER } else { 0.0 }
}

fn lagging_index(k: usize, progress: f32) -> f32 {
    let top = ((k + 1) * scene::FRAGMENT as usize) as f32 - scene::PRIMER;
    top - clamp((progress - lagging_start(k)) / POLYMERASE_STEP, 0.0, lagging_steps(k))
}

#[no_mangle]
pub extern "C" fn tick(time: f32, _x: f32, _y: f32, _z: f32, _touching: i32) -> i32 {
    let progress = clamp((time - scene::START) / (scene::FINISH - scene::START), 0.0, 1.0) * scene::PROGRESS_END;
    let fork = progress.min(scene::FORK_END);
    let bp = scene::BP as usize;
    let fragment = scene::FRAGMENT as usize;
    let primer = scene::PRIMER as usize;

    for i in 0..bp {
        nucleotide(scene::LEADING_TEMPLATE + 1 + 2 * i, LEADING, 1, i, 1, i + 1 < bp, 1.0, fork);
        nucleotide(scene::LAGGING_TEMPLATE + 1 + 2 * i, LAGGING, 0, i, 1, i + 1 < bp, 1.0, fork);
    }
    for j in 0..primer {
        nucleotide(scene::LEADING_PRIMER + 1 + 2 * j, LEADING, 0, j, -1, j > 0, grow(scene::LAG + BIND + PRIMER_STEP * j as f32, progress), fork);
    }
    for i in primer..bp {
        nucleotide(scene::LEADING_PRIMER + 1 + 2 * primer + 2 * (i - primer), LEADING, 0, i, -1, true, grow(i as f32 + scene::LAG, progress), fork);
    }
    for (k, &group) in FRAGMENTS.iter().enumerate() {
        let bottom = (k + 1) * fragment - 1;
        let laid = bottom as f32 + scene::LAG;
        let start = lagging_start(k);
        for j in 0..primer {
            let replaced = if k + 1 < FRAGMENTS.len() { grow(lagging_start(k + 1) + POLYMERASE_STEP * (fragment - primer + 1 + j) as f32, progress) } else { 0.0 };
            nucleotide(group + 2 + 2 * j, LAGGING, 1, bottom - j, 1, j > 0, grow(laid + PRIMER_STEP * j as f32, progress) * (1.0 - replaced), fork);
        }
        let steps = lagging_steps(k) as usize;
        for m in 0..steps {
            nucleotide(group + 2 + 2 * primer + 2 * m, LAGGING, 1, bottom - primer - m, 1, true, grow(start + POLYMERASE_STEP * (m + 1) as f32, progress), fork);
        }
    }

    let tip = slot(LEADING, 1, fork, fork);
    let tangent = sub(slot(LEADING, 1, fork + 0.05, -1.0), slot(LEADING, 1, fork - 0.05, -1.0));
    show(scene::HELICASE, tip, from_up(unit(tangent)), 1.0 - smoothstep(scene::BP - 0.6, scene::BP + 0.4, fork));

    let primase_at = (fork - scene::LAG).max(0.0);
    let hop = smoothstep(scene::LAG + 0.8, scene::LAG + 2.0, progress);
    let primase = lerp(beside(LEADING, 0, primase_at, fork, 0.009), beside(LAGGING, 1, primase_at, fork, 0.009), hop);
    show(scene::PRIMASE, primase, [0.0, 0.0, 0.0, 1.0], smoothstep(0.6, 1.6, progress) * (1.0 - smoothstep(scene::PROGRESS_END - 2.0, scene::PROGRESS_END, progress)));

    let lead = clamp(progress - scene::LAG, primer as f32, scene::BP - 1.0);
    let leading_size = smoothstep(scene::LAG + 1.0, scene::LAG + 1.8, progress);
    show(scene::LEADING_POLYMERASE, beside(LEADING, 0, lead, fork, 0.008), [0.0, 0.0, 0.0, 1.0], leading_size);
    show(scene::LEADING_CLAMP, axis(LEADING, lead - 0.6, fork), [0.0, 0.0, 0.0, 1.0], leading_size);

    let mut active = 0;
    for k in 0..FRAGMENTS.len() {
        if progress >= lagging_start(k) {
            active = k;
        }
    }
    let finish = lagging_start(active) + POLYMERASE_STEP * lagging_steps(active);
    let index = lagging_index(active, progress);
    let mut at = beside(LAGGING, 1, index, fork, 0.008);
    let mut ring = axis(LAGGING, index + 0.6, fork);
    if progress < lagging_start(0) {
        let wait = lagging_index(0, 0.0);
        at = beside(LAGGING, 1, wait, fork, 0.008);
        ring = axis(LAGGING, wait + 0.6, fork);
    } else if progress > finish && active + 1 < FRAGMENTS.len() {
        let t = smoothstep(finish, lagging_start(active + 1), progress);
        let next = lagging_index(active + 1, 0.0);
        at = lerp(at, beside(LAGGING, 1, next, fork, 0.008), t);
        ring = lerp(ring, axis(LAGGING, next + 0.6, fork), t);
    }
    let lagging_size = smoothstep(4.0, 5.0, progress);
    show(scene::LAGGING_POLYMERASE, at, [0.0, 0.0, 0.0, 1.0], lagging_size);
    show(scene::LAGGING_CLAMP, ring, [0.0, 0.0, 0.0, 1.0], lagging_size);

    let ahead = fork + 6.0;
    show(scene::TOPOISOMERASE, axis(LEADING, ahead, fork), turn([0.0, 1.0, 0.0], -progress * 1.5), 1.0 - smoothstep(scene::BP - 2.5, scene::BP - 1.0, ahead));
    publish()
}
