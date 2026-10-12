/// Which planes turn, as bits: 1 xw, 2 yw, 4 zw, 8 xy; live data names them, as `xw`, `zw+xy` or `{"planes":"yw"}`.
static mut PLANES: u8 = 1 | 8;
static mut PARSED: bool = false;
static mut VERTICES: [usize; 16] = [0; 16];
static mut EDGES: [(usize, usize, usize); 32] = [(0, 0, 0); 32];

#[no_mangle]
pub extern "C" fn data(length: i32) -> i32 {
    receive(length)
}

fn digits(text: &[u8]) -> (usize, &[u8]) {
    let end = text.iter().position(|b| !b.is_ascii_digit()).unwrap_or(text.len());
    (text[..end].iter().fold(0, |value, b| value * 10 + (b - b'0') as usize), &text[end..])
}

/// Node indices by name, `v<corner>` and `e<a>_<b>`, so the geometry's own order is the one source.
fn parse() {
    let (vertices, edges) = unsafe { (&mut *core::ptr::addr_of_mut!(VERTICES), &mut *core::ptr::addr_of_mut!(EDGES)) };
    let mut count = 0;
    for (node, name) in scene::NAMES.iter().enumerate() {
        let name = name.as_bytes();
        match name.first() {
            Some(b'v') => vertices[digits(&name[1..]).0] = node,
            Some(b'e') if count < 32 => {
                let (a, rest) = digits(&name[1..]);
                let (b, _) = digits(&rest[1..]);
                edges[count] = (node, a, b);
                count += 1;
            }
            _ => {}
        }
    }
}

fn planes(text: &[u8]) -> u8 {
    let mut planes = 0;
    for (name, bit) in [(b"xw", 1), (b"yw", 2), (b"zw", 4), (b"xy", 8)] {
        if text.windows(2).any(|window| window == name) {
            planes |= bit;
        }
    }
    planes
}

fn rotate(p: &mut [f32; 4], a: usize, b: usize, angle: f32) {
    let (c, s) = (cos(angle), sin(angle));
    let (x, y) = (p[a], p[b]);
    p[a] = c * x - s * y;
    p[b] = s * x + c * y;
}

#[no_mangle]
pub extern "C" fn tick(time: f32, _x: f32, _y: f32, _z: f32, _touching: i32) -> i32 {
    unsafe {
        if !PARSED {
            parse();
            PARSED = true;
        }
        let (text, fresh) = inbox();
        if fresh && planes(text) != 0 {
            PLANES = planes(text);
        }
    }
    let chosen = unsafe { PLANES };
    let angle = time / scene::DURATION * TAU;
    let mut drawn = [[0.0f32; 3]; 16];
    let mut depth = [0.0f32; 16];
    for v in 0..16 {
        let mut p = [0.0f32; 4];
        for (axis, value) in p.iter_mut().enumerate() {
            *value = if v >> axis & 1 == 1 { 1.0 } else { -1.0 };
        }
        for (bit, a, b) in [(1, 0, 3), (2, 1, 3), (4, 2, 3), (8, 0, 1)] {
            if chosen & bit != 0 {
                rotate(&mut p, a, b, angle);
            }
        }
        let k = (scene::EYE_W - 1.0) / (scene::EYE_W - p[3]);
        drawn[v] = [p[0] * k * scene::UNIT, p[1] * k * scene::UNIT, p[2] * k * scene::UNIT];
        depth[v] = k;
    }
    let (vertices, edges) = unsafe { (&*core::ptr::addr_of!(VERTICES), &*core::ptr::addr_of!(EDGES)) };
    for v in 0..16 {
        place(rest(vertices[v]), drawn[v], [0.0, 0.0, 0.0, 1.0], [depth[v]; 3]);
    }
    for &(node, a, b) in edges {
        let (pa, pb) = (drawn[a], drawn[b]);
        let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let length = sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
        let thickness = (depth[a] + depth[b]) / 2.0;
        place(rest(node), [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0, (pa[2] + pb[2]) / 2.0], from_up([d[0] / length, d[1] / length, d[2] / length]), [thickness, length, thickness]);
    }
    publish()
}
