use core::f32::consts::{FRAC_PI_2, PI, TAU};
use core::ptr::{addr_of, addr_of_mut};

pub type Record = [f32; 10];

static mut OUT: [Record; scene::NODES] = scene::REST;
static mut INBOX: [u8; 1024] = [0; 1024];
static mut LENGTH: usize = 0;
static mut FRESH: bool = false;

/// This tick's records, starting from whatever the last tick left; `rest` puts a node back to the file's own transform.
pub fn nodes() -> &'static mut [Record; scene::NODES] {
    unsafe { &mut *addr_of_mut!(OUT) }
}

pub fn rest(node: usize) -> &'static mut Record {
    let records = nodes();
    records[node] = scene::REST[node];
    &mut records[node]
}

pub fn publish() -> i32 {
    addr_of!(OUT) as i32
}

/// For a program's `data` export: where the host writes the next update.
pub fn receive(length: i32) -> i32 {
    unsafe {
        LENGTH = (length.max(0) as usize).min(1024);
        FRESH = true;
        addr_of_mut!(INBOX) as i32
    }
}

/// The newest update, and whether it arrived since the last call.
pub fn inbox() -> (&'static [u8], bool) {
    unsafe {
        let fresh = FRESH;
        FRESH = false;
        (&(&*addr_of!(INBOX))[..LENGTH], fresh)
    }
}

/// The number after `key` in the bytes, as in `{"gpu":0.83}` or `gpu=0.83`; with an empty key, the first number.
pub fn number(bytes: &[u8], key: &[u8]) -> Option<f32> {
    let mut from = 0;
    if !key.is_empty() {
        from = bytes.windows(key.len()).position(|window| window == key)? + key.len();
    }
    let rest = &bytes[from..];
    let start = rest.iter().position(|b| b.is_ascii_digit() || *b == b'-')?;
    let (mut value, mut scale, mut fraction, mut sign) = (0.0f32, 1.0f32, false, 1.0f32);
    for (at, &b) in rest[start..].iter().enumerate() {
        match b {
            b'-' if at == 0 => sign = -1.0,
            b'0'..=b'9' if fraction => {
                scale /= 10.0;
                value += (b - b'0') as f32 * scale;
            }
            b'0'..=b'9' => value = value * 10.0 + (b - b'0') as f32,
            b'.' if !fraction => fraction = true,
            _ => break,
        }
    }
    Some(sign * value)
}

pub fn sin(x: f32) -> f32 {
    let wrapped = (x + PI) % TAU;
    let x = wrapped + if wrapped < 0.0 { TAU } else { 0.0 } - PI;
    let x = if x > FRAC_PI_2 { PI - x } else if x < -FRAC_PI_2 { -PI - x } else { x };
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0))))
}

pub fn cos(x: f32) -> f32 {
    sin(x + FRAC_PI_2)
}

pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1fbd_1df5);
    for _ in 0..3 {
        y = 0.5 * (y + x / y);
    }
    y
}

pub fn clamp(x: f32, low: f32, high: f32) -> f32 {
    if x < low { low } else if x > high { high } else { x }
}

pub fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = clamp((x - low) / (high - low), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn fract(x: f32) -> f32 {
    x - floor(x)
}

pub fn floor(x: f32) -> f32 {
    let truncated = x as i32 as f32;
    if truncated > x { truncated - 1.0 } else { truncated }
}

pub fn atan2(y: f32, x: f32) -> f32 {
    let (ax, ay) = (if x < 0.0 { -x } else { x }, if y < 0.0 { -y } else { y });
    if ax == 0.0 && ay == 0.0 {
        return 0.0;
    }
    let (small, large) = if ax > ay { (ay, ax) } else { (ax, ay) };
    let t = small / large;
    let t2 = t * t;
    let mut angle = t * (0.999_866 + t2 * (-0.330_299_5 + t2 * (0.180_141 + t2 * (-0.085_133 + t2 * 0.020_835_1))));
    if ay > ax {
        angle = FRAC_PI_2 - angle;
    }
    if x < 0.0 {
        angle = PI - angle;
    }
    if y < 0.0 { -angle } else { angle }
}

/// A rotation of `angle` radians about the unit `axis`, as x, y, z, w.
pub fn turn(axis: [f32; 3], angle: f32) -> [f32; 4] {
    let s = sin(angle / 2.0);
    [axis[0] * s, axis[1] * s, axis[2] * s, cos(angle / 2.0)]
}

pub fn compose(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// The shortest rotation taking +y to the unit vector `to`.
pub fn from_up(to: [f32; 3]) -> [f32; 4] {
    let w = 1.0 + to[1];
    if w < 1e-6 {
        return [1.0, 0.0, 0.0, 0.0];
    }
    let q = [to[2], 0.0, -to[0], w];
    let n = sqrt(q[0] * q[0] + q[2] * q[2] + q[3] * q[3]);
    [q[0] / n, 0.0, q[2] / n, q[3] / n]
}

pub fn place(record: &mut Record, translation: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) {
    *record = [translation[0], translation[1], translation[2], rotation[0], rotation[1], rotation[2], rotation[3], scale[0], scale[1], scale[2]];
}

pub fn set_translation(record: &mut Record, translation: [f32; 3]) {
    record[..3].copy_from_slice(&translation);
}

pub fn set_rotation(record: &mut Record, rotation: [f32; 4]) {
    record[3..7].copy_from_slice(&rotation);
}

pub fn set_scale(record: &mut Record, scale: [f32; 3]) {
    record[7..].copy_from_slice(&scale);
}
