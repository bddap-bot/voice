/// Seconds since midnight, mod twelve hours, at the start of the current swing; always even, so a whole swing advances the wheel one tooth.
static mut BASE: i32 = 10 * 3600 + 10 * 60;
/// Which extreme the swing starts from, chosen so the shown time lands on a tooth.
static mut PHASE: f32 = 0.0;
static mut LAST: f32 = 0.0;

/// Live data is the time of day, `14:07:31` or `{"time":"14:07:31"}`.
#[no_mangle]
pub extern "C" fn data(length: i32) -> i32 {
    receive(length)
}

fn time_of_day(text: &[u8]) -> Option<i32> {
    let start = text.iter().position(|b| b.is_ascii_digit())?;
    let mut fields = [0i32; 3];
    let mut field = 0;
    for &b in &text[start..] {
        match b {
            b'0'..=b'9' => fields[field] = fields[field] * 10 + (b - b'0') as i32,
            b':' if field < 2 => field += 1,
            _ => break,
        }
    }
    Some(fields[0] * 3600 + fields[1] * 60 + fields[2])
}

fn swing(beat: f32) -> f32 {
    scene::AMPLITUDE * cos(PI * beat)
}

/// Seconds the wheel has advanced into the swing: dead still while a pallet's arc holds a tooth, following the impulse face, then falling free through the drop.
fn advance(beat: f32) -> f32 {
    let second = floor(beat);
    let within = beat - second;
    let across = scene::AMPLITUDE * cos(PI * within);
    let impulse = clamp((scene::UNLOCK - across) / scene::LIFT, 0.0, 1.0);
    let falling = clamp((within - scene::IMPULSE_END) / scene::DROP_TIME, 0.0, 1.0);
    second + impulse * scene::IMPULSE_SHARE + (1.0 - scene::IMPULSE_SHARE) * falling * falling
}

fn beat(time: f32, phase: f32) -> f32 {
    let beat = time + phase;
    if beat >= 2.0 { beat - 2.0 } else { beat }
}

#[no_mangle]
pub extern "C" fn tick(time: f32, _x: f32, _y: f32, _z: f32, touching: i32) -> i32 {
    let (text, fresh) = inbox();
    unsafe {
        if fresh {
            if let Some(seconds) = time_of_day(text) {
                PHASE = 0.0;
                let mut base = seconds - (advance(beat(time, 0.0)) + 0.5) as i32;
                if base % 2 != 0 {
                    PHASE = 1.0;
                    base = seconds - (advance(beat(time, 1.0)) + 0.5) as i32;
                }
                BASE = base.rem_euclid(43200);
                LAST = beat(time, PHASE);
            }
        }
        let beat = beat(time, PHASE);
        if touching == 0 && beat < LAST - 1.0 {
            BASE = (BASE + 2) % 43200;
        }
        LAST = beat;
        let advanced = advance(beat);
        let theta = swing(beat);
        set_rotation(rest(scene::ANCHOR), turn([0.0, 0.0, 1.0], theta));
        set_rotation(rest(scene::PENDULUM), turn([0.0, 0.0, 1.0], theta));
        let seconds = (BASE % 60) as f32 + advanced;
        set_rotation(rest(scene::ESCAPE_WHEEL), turn([0.0, 0.0, 1.0], -seconds / 60.0 * TAU));
        set_rotation(rest(scene::SECOND_HAND), turn([0.0, 0.0, 1.0], -seconds / 60.0 * TAU));
        set_rotation(rest(scene::MINUTE_HAND), turn([0.0, 0.0, 1.0], -((BASE % 3600) as f32 + advanced) / 3600.0 * TAU));
        set_rotation(rest(scene::HOUR_HAND), turn([0.0, 0.0, 1.0], -(BASE as f32 + advanced) / 43200.0 * TAU));
    }
    publish()
}
