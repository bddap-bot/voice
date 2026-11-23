pub struct Cell {
    pub height: f32,
    pub wide: f32,
    pub tall: f32,
}

pub struct Model {
    pub name: &'static [u8],
    pub root: usize,
    pub left: f32,
    pub span: f32,
    pub modules: [usize; 2],
    pub chunks: [usize; 2],
    pub maps: [usize; 2],
    pub streams: usize,
    pub poles: [f32; 2],
    pub tokens: [usize; 2],
    pub beads: [usize; 2],
    pub side: f32,
    pub pitch: f32,
    pub rung: f32,
    pub floor: f32,
    pub top: f32,
}

pub struct Module {
    pub node: usize,
    pub at: [f32; 2],
    pub cell: Cell,
    pub read: [f32; 2],
    pub write: [f32; 2],
}

pub struct Chunk {
    pub node: usize,
    pub visitor: usize,
    pub at: [f32; 2],
    pub cell: Cell,
}

pub struct Frame {
    pub node: usize,
    pub model: usize,
    pub module: usize,
    pub chunk: Option<usize>,
}

pub struct Tie {
    pub node: usize,
    pub model: usize,
    pub module: usize,
    pub chunk: usize,
}

pub struct Pip {
    pub node: usize,
    pub model: usize,
    pub module: usize,
    pub chunk: usize,
    pub stream: usize,
}

pub struct Map {
    pub name: &'static [u8],
    pub table: Option<usize>,
}

pub struct Pass {
    pub node: usize,
    pub model: usize,
    pub map: usize,
}

const EASE: f32 = 0.07;
/// Ticks each chunk is lit for while `shared` walks through them.
const DWELL: u32 = 160;
/// What the prelude's inbox holds; the host writes every byte it was given an address for.
const ROOM: i32 = 1024;

#[derive(Clone, Copy, PartialEq)]
enum Shared {
    Off,
    Each,
    One(usize),
}

struct State {
    map: [usize; scene::MODELS.len()],
    weigh: bool,
    weighed: f32,
    shared: Shared,
    token: u32,
    shown: [bool; scene::MODELS.len()],
    seen: [f32; scene::MODELS.len()],
    at: [f32; scene::MODELS.len()],
    hot: [f32; scene::CHUNKS.len()],
    hottest: [f32; scene::MODELS.len()],
    frames: [f32; scene::FRAMES.len()],
    ties: [f32; scene::TIES.len()],
    passes: [f32; scene::PASSES.len()],
    ticks: u32,
    started: bool,
}

static mut STATE: State = State {
    map: [0; scene::MODELS.len()],
    weigh: false,
    weighed: 0.0,
    shared: Shared::Off,
    token: 0,
    shown: [true; scene::MODELS.len()],
    seen: [1.0; scene::MODELS.len()],
    at: [0.0; scene::MODELS.len()],
    hot: [0.0; scene::CHUNKS.len()],
    hottest: [0.0; scene::MODELS.len()],
    frames: [0.0; scene::FRAMES.len()],
    ties: [0.0; scene::TIES.len()],
    passes: [0.0; scene::PASSES.len()],
    ticks: 0,
    started: false,
};

#[no_mangle]
pub extern "C" fn data(length: i32) -> i32 {
    if length > ROOM {
        return 0;
    }
    receive(length)
}

fn words(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    text.split(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_')).filter(|word| !word.is_empty())
}

fn whole(word: &[u8]) -> Option<usize> {
    word.iter().try_fold(0usize, |sum, byte| byte.is_ascii_digit().then(|| sum.saturating_mul(10).saturating_add((byte - b'0') as usize)))
}

fn chunk(word: &[u8]) -> Option<usize> {
    whole(word.strip_prefix(b"chunk".as_slice()).filter(|digits| !digits.is_empty()).unwrap_or(word))
}

impl State {
    fn read(&mut self, text: &[u8]) {
        let mut words = words(text).peekable();
        while let Some(word) = words.next() {
            match word {
                b"map" => {
                    let Some(name) = words.next_if(|next| !scene::COMMANDS.contains(next)) else { continue };
                    for (model, entry) in scene::MODELS.iter().enumerate() {
                        if let Some(map) = (entry.maps[0]..entry.maps[1]).find(|&map| scene::MAPS[map].name == name) {
                            self.map[model] = map;
                        }
                    }
                }
                b"weigh" => self.weigh = words.next_if(|next| *next == b"off".as_slice()).is_none(),
                b"shared" => {
                    let stored = |chunk: usize| scene::MODELS.iter().any(|model| chunk < model.chunks[1] - model.chunks[0]);
                    match words.next_if(|next| *next == b"off".as_slice() || chunk(next).is_some()) {
                        Some(b"off") => self.shared = Shared::Off,
                        Some(next) => {
                            if let Some(chunk) = chunk(next).filter(|chunk| stored(*chunk)) {
                                self.shared = Shared::One(chunk);
                            }
                        }
                        None => self.shared = Shared::Each,
                    }
                }
                b"token" => {
                    if let Some(token) = words.next_if(|next| whole(next).is_some()).and_then(whole) {
                        self.token = token as u32;
                    }
                }
                b"show" => {
                    let mut named = [false; scene::MODELS.len()];
                    let mut all = false;
                    while let Some(next) = words.next_if(|next| *next == b"all".as_slice() || scene::MODELS.iter().any(|model| model.name == *next)) {
                        match scene::MODELS.iter().position(|model| model.name == next) {
                            Some(model) => named[model] = true,
                            None => all = true,
                        }
                    }
                    if all || named.contains(&true) {
                        self.shown = if all { [true; scene::MODELS.len()] } else { named };
                    }
                }
                _ => {}
            }
        }
    }
}

/// Which chunk `module` of a model of `chunks` uses for this token; a map without a table picks one per token, made up.
fn chosen(map: &Map, module: usize, token: u32, chunks: usize) -> usize {
    match map.table {
        Some(table) => scene::TABLE[table + module],
        None => {
            let mut hash = token.wrapping_mul(0x9E37_79B1) ^ (module as u32 + 1).wrapping_mul(0x85EB_CA6B);
            hash ^= hash >> 15;
            hash = hash.wrapping_mul(0x2C1B_3C6D);
            hash ^= hash >> 12;
            (hash % chunks as u32) as usize
        }
    }
}

fn ease(value: &mut f32, target: f32) -> f32 {
    *value += (target - *value) * EASE;
    *value
}

fn blend(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

fn bump(phase: f32, edge: f32) -> f32 {
    smoothstep(0.0, edge, phase) * (1.0 - smoothstep(1.0 - edge, 1.0, phase))
}

fn climb(model: &Model, clock: f32) -> f32 {
    let modules = (model.modules[1] - model.modules[0]) as f32;
    let low = scene::MODULES[model.modules[0]].at[1] - model.pitch / 2.0;
    let step = clock - 1.0;
    if step < 0.0 {
        blend(model.floor, low, smoothstep(0.0, 1.0, clock))
    } else if step < modules {
        low + step * model.pitch
    } else {
        blend(low + modules * model.pitch, model.top, smoothstep(0.0, 1.0, step - modules))
    }
}

fn away(tip: [f32; 2], at: [f32; 2], cell: &Cell, weighed: f32) -> f32 {
    let (dx, dy) = (tip[0] - at[0], tip[1] - blend(at[1], cell.height, weighed));
    sqrt(dx * dx + dy * dy)
}

fn nearest(count: usize, away: impl Fn(usize) -> f32) -> Option<(f32, usize)> {
    (0..count).map(|index| (away(index), index)).filter(|(away, _)| *away < scene::REACH).reduce(|near, next| if next.0 < near.0 { next } else { near })
}

#[no_mangle]
pub extern "C" fn tick(time: f32, x: f32, y: f32, _z: f32, touching: i32) -> i32 {
    let state = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
    if !state.started {
        state.started = true;
        for (model, entry) in scene::MODELS.iter().enumerate() {
            state.map[model] = entry.maps[0];
            state.at[model] = scene::REST[entry.root][0];
        }
        for (at, frame) in scene::FRAMES.iter().enumerate() {
            state.frames[at] = scene::REST[frame.node][7];
        }
        for (at, tie) in scene::TIES.iter().enumerate() {
            state.ties[at] = scene::REST[tie.node][8];
        }
        for (at, pass) in scene::PASSES.iter().enumerate() {
            state.passes[at] = scene::REST[pass.node][7];
        }
    }
    let (text, fresh) = inbox();
    if fresh {
        state.read(text);
    }
    state.ticks = state.ticks.wrapping_add(1);
    let weighed = ease(&mut state.weighed, if state.weigh { 1.0 } else { 0.0 });
    let plain = 1.0 - weighed;
    let turn = floor(time / scene::DURATION);
    let clock = time - turn * scene::DURATION;
    let token = (turn as i32 as u32).wrapping_add(state.token);
    let life = bump(clock / scene::DURATION, 0.3 / scene::DURATION) * plain;
    let step = clock - 1.0;
    let phase = step - floor(step);

    let shown: f32 = scene::MODELS.iter().zip(&state.shown).filter(|(_, shown)| **shown).map(|(model, _)| model.span + scene::ROW[1]).sum::<f32>() - scene::ROW[1];
    let mut cursor = scene::ROW[0] - shown / 2.0;

    for (index, model) in scene::MODELS.iter().enumerate() {
        if state.shown[index] {
            ease(&mut state.at[index], cursor - model.left);
            cursor += model.span + scene::ROW[1];
        }
        let seen = ease(&mut state.seen[index], if state.shown[index] { 1.0 } else { 0.0 });
        place(rest(model.root), [state.at[index], 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [seen; 3]);

        let map = &scene::MAPS[state.map[index]];
        let (first_module, first_chunk) = (model.modules[0], model.chunks[0]);
        let (modules, chunks) = (model.modules[1] - first_module, model.chunks[1] - first_chunk);
        let at = if step >= 0.0 && step < modules as f32 { Some(step as usize) } else { None };
        let height = climb(model, clock);

        let touched = if touching != 0 && state.shown[index] {
            let tip = [x - state.at[index], y];
            let plate = nearest(chunks, |chunk| away(tip, scene::CHUNKS[first_chunk + chunk].at, &scene::CHUNKS[first_chunk + chunk].cell, weighed));
            let frame = map.table.and_then(|_| nearest(modules, |module| away(tip, scene::MODULES[first_module + module].at, &scene::MODULES[first_module + module].cell, weighed)));
            match (plate, frame) {
                (Some(plate), Some(frame)) if frame.0 < plate.0 => Some(chosen(map, frame.1, token, chunks)),
                (Some(plate), _) => Some(plate.1),
                (None, frame) => frame.map(|frame| chosen(map, frame.1, token, chunks)),
            }
        } else {
            None
        };
        let walked = (state.ticks / DWELL) as usize % chunks;
        let mut hottest = 0.0f32;
        for chunk in 0..chunks {
            let lit = match (touched, state.shared) {
                (Some(touched), _) => touched == chunk,
                (None, Shared::Each) => walked == chunk,
                (None, Shared::One(one)) => one == chunk,
                (None, Shared::Off) => false,
            };
            hottest = hottest.max(ease(&mut state.hot[first_chunk + chunk], if lit { 1.0 } else { 0.0 }));
        }
        state.hottest[index] = hottest;

        for stream in 0..model.streams {
            place(rest(model.tokens[stream]), [model.poles[stream], height, 0.0], [0.0, 0.0, 0.0, 1.0], [life; 3]);
            let bead = rest(model.beads[stream]);
            let Some(module) = at else { continue };
            let entry = &scene::MODULES[first_module + module];
            let reach = model.rung / model.pitch;
            let (leave, join) = (0.5 - reach, 0.5 + reach);
            let inside = entry.at[0] + if stream == 0 { -0.2 } else { 0.2 } * model.side;
            let (pole, lower, upper) = (model.poles[stream], entry.at[1] - model.rung, entry.at[1] + model.rung);
            let third = (join - leave) / 3.0;
            let (bx, by) = if phase < leave + third {
                (blend(pole, inside, smoothstep(leave, leave + third, phase)), lower)
            } else if phase < join - third {
                (inside, blend(lower, upper, smoothstep(leave + third, join - third, phase)))
            } else {
                (blend(inside, pole, smoothstep(join - third, join, phase)), upper)
            };
            let size = smoothstep(leave, leave + 0.04, phase) * (1.0 - smoothstep(join - 0.04, join, phase)) * (0.4 + 0.6 * entry.read[stream]) * plain;
            place(bead, [bx, by, 0.0], [0.0, 0.0, 0.0, 1.0], [size; 3]);
        }

        for module in 0..modules {
            let entry = &scene::MODULES[first_module + module];
            let size = if at == Some(module) { 1.0 + 0.05 * bump(phase, 0.25) } else { 1.0 };
            place(
                rest(entry.node),
                [entry.at[0], blend(entry.at[1], entry.cell.height, weighed), 0.0],
                [0.0, 0.0, 0.0, 1.0],
                [blend(1.0, entry.cell.wide, weighed) * size, blend(1.0, entry.cell.tall, weighed) * size, size],
            );
        }

        for chunk in 0..chunks {
            let entry = &scene::CHUNKS[first_chunk + chunk];
            let pop = state.hot[first_chunk + chunk];
            let used = at.filter(|&module| chosen(map, module, token, chunks) == chunk);
            let busy = if used.is_some() { bump(phase, 0.25) } else { 0.0 };
            let size = 1.0 + 0.15 * pop - 0.2 * hottest * (1.0 - pop) + 0.08 * busy;
            place(
                rest(entry.node),
                [entry.at[0], blend(entry.at[1], entry.cell.height, weighed), 0.3 * model.side * pop],
                [0.0, 0.0, 0.0, 1.0],
                [blend(1.0, entry.cell.wide, weighed) * size, blend(1.0, entry.cell.tall, weighed) * size, size],
            );
            let visitor = rest(entry.visitor);
            if let Some(module) = used {
                let to = scene::MODULES[first_module + module].at;
                let there = smoothstep(0.0, 0.25, phase) - smoothstep(0.75, 1.0, phase);
                let size = bump(phase, 0.1) * plain;
                place(visitor, [blend(entry.at[0], to[0], there), blend(entry.at[1], to[1], there), 0.0], [0.0, 0.0, 0.0, 1.0], [size; 3]);
            }
        }
    }

    for (at, frame) in scene::FRAMES.iter().enumerate() {
        let model = &scene::MODELS[frame.model];
        let map = &scene::MAPS[state.map[frame.model]];
        let wears = frame.chunk == map.table.map(|table| scene::TABLE[table + frame.module]);
        let hottest = state.hottest[frame.model];
        let pop = frame.chunk.map_or(hottest, |chunk| state.hot[model.chunks[0] + chunk]);
        let size = ease(&mut state.frames[at], if wears { 1.0 } else { 0.0 }) * (1.0 + 0.15 * pop - 0.2 * hottest * (1.0 - pop));
        place(rest(frame.node), [0.0, 0.0, 0.3 * model.side * pop], [0.0, 0.0, 0.0, 1.0], [size; 3]);
    }

    for (at, tie) in scene::TIES.iter().enumerate() {
        let model = &scene::MODELS[tie.model];
        let map = &scene::MAPS[state.map[tie.model]];
        let taken = chosen(map, tie.module, token, model.chunks[1] - model.chunks[0]) == tie.chunk;
        let through = step - tie.module as f32;
        let busy = if taken && through >= 0.0 && through < 1.0 { bump(through, 0.25) } else { 0.0 };
        let held = ease(&mut state.ties[at], if map.table.is_none() { scene::FAINT } else if taken { 1.0 } else { 0.0 });
        let pop = state.hot[model.chunks[0] + tie.chunk];
        let thick = (held * (1.0 + 1.2 * pop - 0.5 * state.hottest[tie.model] * (1.0 - pop)) + 1.5 * busy) * plain;
        let record = rest(tie.node);
        record[8] = thick;
        record[9] = thick;
    }

    for pip in scene::PIPS.iter() {
        let model = &scene::MODELS[pip.model];
        let map = &scene::MAPS[state.map[pip.model]];
        let modules = model.modules[1] - model.modules[0];
        let record = rest(pip.node);
        if chosen(map, pip.module, token, model.chunks[1] - model.chunks[0]) != pip.chunk {
            continue;
        }
        let joined = pip.module as f32 + 0.5 + model.rung / model.pitch;
        let carried = smoothstep(joined - 0.03, joined + 0.12, step) * (0.45 + 0.55 * scene::MODULES[model.modules[0] + pip.module].write[pip.stream]) * life;
        let angle = TAU * pip.module as f32 / modules as f32 + state.ticks as f32 * 0.012;
        place(record, [model.poles[pip.stream] + scene::ORBIT * cos(angle), climb(model, clock) + 0.35 * scene::ORBIT * sin(angle * 2.0), scene::ORBIT * sin(angle)], [0.0, 0.0, 0.0, 1.0], [carried; 3]);
    }

    for (at, pass) in scene::PASSES.iter().enumerate() {
        let size = ease(&mut state.passes[at], if state.map[pass.model] == pass.map { 1.0 } else { 0.0 }) * plain;
        set_scale(rest(pass.node), [size; 3]);
    }

    for node in scene::QUIET {
        let record = rest(node);
        for axis in 7..10 {
            record[axis] *= plain;
        }
    }
    publish()
}
