/// Live data is JSON or `key=value` text with any of `gpu` and `vram` as fractions and `queued` and `running` as counts, as in `{"gpu":0.83,"vram":0.61,"queued":12,"running":3}`; until the first arrives it shows a made-up load.
struct State {
    live: bool,
    target: [f32; 4],
    needle: f32,
    needle_speed: f32,
    vram: f32,
    fan: f32,
    orbit: f32,
    last: f32,
    workers: [f32; 8],
    heights: [f32; 20],
    speeds: [f32; 20],
    shown: [f32; 20],
}

static mut STATE: State = State { live: false, target: [0.0; 4], needle: 0.0, needle_speed: 0.0, vram: 0.0, fan: 0.0, orbit: 0.0, last: 0.0, workers: [0.0; 8], heights: [0.3; 20], speeds: [0.0; 20], shown: [0.0; 20] };

#[no_mangle]
pub extern "C" fn data(length: i32) -> i32 {
    receive(length)
}

fn demo(time: f32) -> [f32; 4] {
    [0.55 + 0.4 * sin(time * 0.6), 0.6 + 0.25 * sin(time * 0.23), 7.0 + 6.0 * sin(time * 0.35), 3.0 + 2.0 * sin(time * 0.17)]
}

#[no_mangle]
pub extern "C" fn tick(time: f32, _x: f32, _y: f32, _z: f32, _touching: i32) -> i32 {
    let state = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
    let (text, fresh) = inbox();
    if fresh {
        for (at, key) in [b"gpu".as_slice(), b"vram", b"queued", b"running"].into_iter().enumerate() {
            if let Some(value) = number(text, key) {
                state.target[at] = value;
                state.live = true;
            }
        }
    }
    let dt = clamp(time - state.last, 0.0, 0.05);
    state.last = time;
    let [gpu, vram, queued, running] = if state.live { state.target } else { demo(time) };
    let (gpu, vram) = (clamp(gpu, 0.0, 1.0), clamp(vram, 0.0, 1.0));
    let queued = clamp(queued + 0.5, 0.0, scene::QUEUE_SLOTS) as usize;
    let running = clamp(running + 0.5, 0.0, scene::WORKER_COUNT) as usize;

    state.needle_speed += ((gpu - state.needle) * 60.0 - state.needle_speed * 7.0) * dt;
    state.needle += state.needle_speed * dt;
    set_rotation(rest(scene::GPU_NEEDLE), turn([0.0, 0.0, 1.0], PI / 2.0 - clamp(state.needle, -0.05, 1.05) * PI));

    state.fan += (0.2 + 2.6 * gpu) * TAU * dt;
    set_rotation(rest(scene::FAN_ROTOR), turn([0.0, 0.0, 1.0], -state.fan));

    state.vram += (vram - state.vram) * clamp(dt * 3.0, 0.0, 1.0);
    set_scale(rest(scene::VRAM_FILL), [1.0, clamp(state.vram, 0.002, 1.0) * scene::COLUMN, 1.0]);

    state.orbit += 0.5 * TAU * dt;
    for i in 0..8 {
        let goal = if i < running { 1.0 } else { 0.0 };
        state.workers[i] += (goal - state.workers[i]) * clamp(dt * 5.0, 0.0, 1.0);
        let angle = state.orbit + i as f32 * TAU / running.max(1) as f32;
        let bob = 1.0 + 0.15 * sin(time * 6.0 + i as f32);
        let record = rest(scene::WORKER0 + i);
        place(record, [scene::FAN_ORBIT * cos(angle), scene::FAN_ORBIT * sin(angle), 0.0], [0.0, 0.0, 0.0, 1.0], [state.workers[i] * bob; 3]);
    }

    for i in 0..20 {
        let record = rest(scene::JOB0 + i);
        let resting = record[1];
        if i < queued {
            state.speeds[i] -= 2.5 * dt;
            state.heights[i] += state.speeds[i] * dt;
            if state.heights[i] < 0.0 {
                state.heights[i] = 0.0;
                state.speeds[i] = -state.speeds[i] * 0.35;
            }
            state.shown[i] = 1.0;
        } else {
            state.shown[i] = clamp(state.shown[i] - dt * 6.0, 0.0, 1.0);
            state.heights[i] = 0.06 + 0.01 * i as f32;
            state.speeds[i] = 0.0;
        }
        set_translation(record, [record[0], resting + state.heights[i], record[2]]);
        set_scale(record, [state.shown[i]; 3]);
    }
    publish()
}
