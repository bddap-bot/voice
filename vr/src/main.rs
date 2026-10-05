mod audio;
mod board;
mod conversation;
mod identity;
mod session;
mod voice;
mod gesture;
mod hub;
mod motion;
mod openvr;
mod page;
mod placement;
mod vulkan;
mod vrm;
mod render;
mod relay;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::json;

use openvr::{Runtime, Signal};
use page::{Frame, Page};
use board::{Board, Press};
use placement::{above_hand, below_wrist, desk_spot, local_tip, Anchor, Hand, Interaction, Pose};
use motion::{Animator, Clip, Random, IDLES};
use gesture::{Recognizer, Templates};
use relay::{Avatar, Relay, Token};
use voice::Voice;
use render::{eye_projection, Appearance, Renderer};
use vrm::{Fit, Humanoid, Model, Skinned};

const EYE: u32 = 768;
const QUAD: f32 = 0.4;
const HEIGHT: f32 = 0.3;
const MARGIN: f32 = 0.03;
const FLOOR: f32 = -QUAD / 2.0 + MARGIN;
const PAGE: &str = "https://bddap-bot.github.io/voice/";
const HEAD_AHEAD: f32 = 0.04;
const REQUEST_LOST: Duration = Duration::from_millis(100);
const IN_FLIGHT: usize = 3;
const DORMANT_POLL: Duration = Duration::from_micros(11_111);

fn directory(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(fallback)).join("voice-vr")
}

fn load(path: &PathBuf) -> Option<Anchor> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn save(path: &PathBuf, anchor: &Anchor) {
    let temporary = path.with_extension("tmp");
    if let Err(error) = std::fs::write(&temporary, serde_json::to_vec(anchor).unwrap()).and_then(|()| std::fs::rename(&temporary, path)) {
        eprintln!("placement not saved: {error}");
    }
}

struct Meter {
    labels: [&'static str; 2],
    window: Option<Instant>,
    first: Vec<Duration>,
    second: Vec<Duration>,
}

fn summary(samples: &mut [Duration]) -> String {
    samples.sort();
    let mean = samples.iter().sum::<Duration>().as_secs_f64() * 1e3 / samples.len() as f64;
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)].as_secs_f64() * 1e3;
    format!("mean {mean:.1} p95 {p95:.1} ms")
}

impl Meter {
    fn new(first: &'static str, second: &'static str) -> Meter {
        Meter { labels: [first, second], window: None, first: Vec::new(), second: Vec::new() }
    }

    fn frame(&mut self, first: Duration, second: Duration) {
        let now = Instant::now();
        let window = *self.window.get_or_insert(now);
        self.first.push(first);
        self.second.push(second);
        let elapsed = now - window;
        if elapsed >= Duration::from_secs(5) {
            let rate = self.first.len() as f64 / elapsed.as_secs_f64();
            let [first, second] = self.labels;
            eprintln!("overlay {rate:.1} fps; {first} {}; {second} {}", summary(&mut self.first), summary(&mut self.second));
            *self = Meter { window: Some(now), ..Meter::new(first, second) };
        }
    }
}

struct Puppet {
    model: Model,
    skinned: Skinned,
    fit: Fit,
    standing: Humanoid,
    animator: Animator,
    drawn: Appearance,
}

fn puppet(relay: &mut Relay, avatar: &Avatar, renderer: &mut Renderer) -> Result<Puppet, String> {
    let model = Model::parse(&relay.puppet(avatar)?).map_err(|error| format!("{}: {error}", avatar.id))?;
    let standing = model.standing();
    let mut clips = HashMap::new();
    for entry in relay.clips()?.iter().filter(|entry| IDLES.contains(&entry.action.as_str())) {
        if entry.format != "fbx" {
            eprintln!("{}: {} clips are not played natively", entry.name, entry.format);
            continue;
        }
        match relay.motion(entry).and_then(|bytes| Clip::parse(&bytes, model.version, model.rest_hips())) {
            Ok(mut clip) => {
                clip.anchor(&standing);
                clips.insert(entry.action.clone(), clip);
            }
            Err(error) => eprintln!("{}: {error}", entry.name),
        }
    }
    eprintln!("appearance {} with {} standing idle clips", avatar.id, clips.len());
    let skinned = model.skinned()?;
    let fit = Fit::new(&model, &skinned, HEIGHT);
    let drawn = renderer.appearance(&model, &skinned)?;
    Ok(Puppet { model, skinned, fit, standing, animator: Animator::new(clips, Random::seeded()), drawn })
}

/// Loads the picked appearance beside the shown one, then makes it the server's selection, as the page's picker does.
fn switch(token: &Token, cache: &std::path::Path, avatar: &Avatar, renderer: &mut Renderer) -> Result<Puppet, String> {
    let mut relay = Relay::connect(token, cache)?;
    let loaded = puppet(&mut relay, avatar, renderer)?;
    relay.select(&avatar.id)?;
    Ok(loaded)
}

fn native(runtime: &Runtime, token: &Token, state: &std::path::Path) -> Result<(), String> {
    let gesture_file = state.join("gesture.json");
    let mut recognizer = Recognizer::new(Templates::parse(&std::fs::read(&gesture_file).map_err(|error| format!("{}: {error}", gesture_file.display()))?)?);
    let gpu = Rc::new(vulkan::Gpu::new(Some(runtime))?);
    let mut renderer = Renderer::new(gpu.clone(), [EYE, EYE], HEIGHT / render::PAGE_HEIGHT)?;
    let cache = state.join("assets");
    let mut relay = Relay::connect(token, &cache)?;
    let catalog = relay.catalog()?;
    let active = catalog.avatars.iter().position(|avatar| avatar.id == catalog.active).ok_or_else(|| format!("the selected appearance {} is not in the catalog", catalog.active))?;
    let mut puppet = puppet(&mut relay, &catalog.avatars[active], &mut renderer)?;
    drop(relay);
    let names = catalog.avatars.iter().map(|avatar| avatar.file.strip_suffix(".vrm").unwrap_or(&avatar.file).to_owned()).collect();
    let mut board = Board::new(names, active);
    let mut board_image = vulkan::Flat::new(gpu, board::PIXELS[0], board::PIXELS[1])?;
    let mut overlay = runtime.create_overlay("voice.puppet", "Puppet", QUAD, true)?;
    let mut board_overlay = runtime.create_overlay("voice.board", "Board", board::WIDTH, false)?;
    let eye_offsets = runtime.eye_offsets();
    let labels = ["frame interval", "animate+draw+submit"];
    let mut meter = Meter::new(labels[0], labels[1]);
    let epoch = Instant::now();
    let mut last = epoch;
    let mut voice = Voice::new(token.clone(), cache.clone());
    eprintln!("dormant");
    loop {
        if let Some(Signal::Quit) = runtime.poll() {
            return Ok(());
        }
        let started = Instant::now();
        let interval = started - last;
        last = started;
        let poses = runtime.poses(0.0);
        let head = runtime.head(&poses);
        let hands = runtime.hands(&poses);
        let hand = |which| hands.iter().find(|hand| hand.hand == which).map(|hand| hand.pose);
        if let (Some(head), Some(left), Some(right)) = (head, hand(Hand::Left), hand(Hand::Right)) {
            if let Some(verdict) = recognizer.push((started - epoch).as_secs_f64(), &head, left.t, right.t) {
                let kind = match (verdict.matched, voice.awake()) {
                    (true, false) => "match, waking",
                    (true, true) => "match while awake, ignored",
                    (false, _) => "near miss",
                };
                if verdict.matched || verdict.near {
                    eprintln!("gesture {kind}: distance {:.3} peak {:.2} m/s over {:.2} s", verdict.distance, verdict.peak, verdict.duration);
                }
                if verdict.matched && !voice.awake() {
                    voice.summon();
                    board.reset();
                    meter = Meter::new(labels[0], labels[1]);
                }
            }
        }
        voice.step();
        let (true, Some(head), Some(left), Some(device)) = (voice.awake(), head, hand(Hand::Left), runtime.hand_index(Hand::Left)) else {
            overlay.hide();
            board_overlay.hide();
            board.reset();
            std::thread::sleep(DORMANT_POLL);
            continue;
        };
        let board_pose = below_wrist(&left, &head, board::HEIGHT);
        board_overlay.place_on(device, &left.inverse().then(&board_pose));
        match board.touch(hand(Hand::Right).map(|right| local_tip(&board_pose, &right))) {
            Some(Press::Dismiss) => {
                voice.dismiss("dismissed");
                continue;
            }
            Some(Press::Mute) => {
                board.muted = voice.toggle_mute();
                board.mark();
            }
            Some(Press::Reset) => {
                voice.reset();
                continue;
            }
            Some(Press::Appearance(index)) if index != board.active => {
                let avatar = &catalog.avatars[index];
                eprintln!("picked appearance {}", avatar.id);
                board.pending = Some(index);
                board.mark();
                if let Some(image) = board.take_image() {
                    board_overlay.submit(&mut board_image.upload(&image)?)?;
                }
                match switch(token, &cache, avatar, &mut renderer) {
                    Ok(loaded) => {
                        puppet = loaded;
                        board.active = index;
                    }
                    Err(error) => eprintln!("appearance {} not shown: {error}", avatar.id),
                }
                board.pending = None;
                board.mark();
            }
            _ => {}
        }
        if let Some(image) = board.take_image() {
            board_overlay.submit(&mut board_image.upload(&image)?)?;
        }
        if let Some(idle) = puppet.animator.update(interval.as_secs_f32().min(0.05)) {
            eprintln!("idle {idle}");
        }
        let mut pose = puppet.model.pose(&puppet.animator.humanoid(&puppet.standing, puppet.model.rest_hips()));
        puppet.model.express(&mut pose, "blink", puppet.animator.blink());
        puppet.model.express(&mut pose, "aa", voice.mouth());
        let changed = puppet.skinned.morph(&pose.weights);
        let worlds = puppet.model.worlds(&pose);
        let palette = puppet.skinned.palette(&worlds, puppet.fit.placement(&puppet.model, &worlds, FLOOR));
        puppet.drawn.update(&palette, &puppet.skinned.vertices, &changed);
        let quad = above_hand(&left, &head, -FLOOR);
        overlay.place_on(device, &left.inverse().then(&quad));
        let local = quad.inverse();
        let eyes = eye_offsets.map(|eye: Pose| eye_projection(local.apply(head.then(&eye).t).into(), QUAD / 2.0, QUAD / 2.0));
        overlay.submit(&mut renderer.render(&puppet.drawn, eyes)?)?;
        meter.frame(interval, started.elapsed());
        runtime.wait_frame();
    }
}

fn run() -> Result<(), String> {
    let config = directory("XDG_CONFIG_HOME", ".config");
    let state = directory("XDG_STATE_HOME", ".local/state");
    std::fs::create_dir_all(&state).map_err(|error| format!("{}: {error}", state.display()))?;
    let token_file = config.join("token");
    let token = std::fs::read_to_string(&token_file).map_err(|error| format!("{}: {error}", token_file.display()))?.trim().to_owned();
    let runtime = Runtime::init()?;
    if std::env::var_os("VOICE_VR_NATIVE").is_some() {
        return native(&runtime, &Token::decode(&token)?, &state);
    }
    let page_address = std::env::var("VOICE_VR_PAGE").unwrap_or_else(|_| PAGE.to_owned());
    let browser = std::env::var("VOICE_VR_BROWSER").unwrap_or_else(|_| "chromium".to_owned());
    let placement_file = state.join("placement.json");

    let mut uploader = vulkan::Uploader::new(vulkan::Gpu::new(Some(&runtime))?, EYE * 2, EYE)?;
    let mut overlay = runtime.create_overlay("voice.puppet", "Puppet", QUAD, true)?;
    let hello = json!({ "type": "hello", "token": token, "eye": [EYE, EYE], "quad": [QUAD, QUAD], "height": HEIGHT, "margin": MARGIN });
    let mut page = Page::open(&browser, &page_address, &state.join("browser"), hello).map_err(|error| format!("{browser}: {error}"))?;
    let eye_offsets = runtime.eye_offsets();
    let mut interaction = Interaction::new([0.0, -QUAD / 2.0 + MARGIN + HEIGHT * 0.55, 0.0]);
    let mut anchor = load(&placement_file);
    let started = Instant::now();
    let period = runtime.display_period()?;
    let mut meter = Meter::new("pose→frame", "upload+submit");
    let mut requested: VecDeque<Instant> = VecDeque::new();
    let mut next_request = Instant::now();

    loop {
        if let Some(Signal::Quit) = runtime.poll() {
            return Ok(());
        }
        if let Some(status) = page.browser_exited() {
            return Err(format!("browser exited: {status}"));
        }
        let poses = runtime.poses(0.0);
        let Some(head) = runtime.head(&poses) else {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let frame = page.latest_frame().map(|(frame, count)| (frame, Instant::now(), requested.drain(..count.min(requested.len())).last()));
        let hands = runtime.hands(&poses);
        let current = *anchor.get_or_insert_with(|| Anchor::World(desk_spot(&head)));
        let hand_pose = |which| hands.iter().find(|hand| hand.hand == which).map(|hand| hand.pose);
        let placed = match (interaction.carrying(), current) {
            (Some((hand, offset)), _) => hand_pose(hand).map(|pose| pose.then(&offset)),
            (None, Anchor::World(pose)) => Some(pose),
            (None, Anchor::Wrist { hand, offset }) => hand_pose(hand).map(|pose| pose.then(&offset)),
        };
        match (interaction.carrying(), current) {
            (Some(_), _) | (None, Anchor::World(_)) => {
                if let Some(pose) = placed {
                    overlay.place_world(&pose);
                }
            }
            (None, Anchor::Wrist { hand, offset }) => {
                if let Some(index) = runtime.hand_index(hand) {
                    overlay.place_on(index, &offset);
                }
            }
        }
        if let Some(placed) = placed {
            match interaction.step(started.elapsed().as_secs_f32(), &current, &placed, &hands, &head) {
                Some(placement::Event::Tap) => page.send(json!({ "type": "tap" })),
                Some(placement::Event::Moved(moved)) => {
                    anchor = Some(moved);
                    save(&placement_file, &moved);
                }
                None => {}
            }
            if requested.front().is_some_and(|at| at.elapsed() >= REQUEST_LOST) {
                requested.clear();
            }
            if requested.len() < IN_FLIGHT && Instant::now() >= next_request {
                let predicted = runtime.head(&runtime.poses(HEAD_AHEAD)).unwrap_or(head);
                let local = placed.inverse();
                let eyes = eye_offsets.map(|eye: Pose| local.apply(predicted.then(&eye).t));
                page.pose(eyes, local.apply(predicted.t));
                let now = Instant::now();
                next_request = (next_request + period).max(now);
                requested.push_back(now);
            }
        }
        if let Some((frame, arrived, asked)) = frame {
            let mut texture = uploader.upload(&Frame::parse(frame, [EYE, EYE])?)?;
            overlay.submit(&mut texture)?;
            if let Some(asked) = asked {
                meter.frame(arrived - asked, arrived.elapsed());
            }
        }
        let due = next_request.saturating_duration_since(Instant::now());
        page.wait(if requested.len() < IN_FLIGHT && !due.is_zero() { due } else { period });
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("voice-vr: {error}");
        std::process::exit(1);
    }
}
