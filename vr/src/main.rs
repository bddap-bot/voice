mod audio;
mod board;
mod conversation;
mod gaze;
mod identity;
mod session;
mod speaker;
mod voice;
mod gesture;
mod hub;
mod motion;
mod openvr;
mod placement;
mod preview;
mod vulkan;
mod vrm;
mod render;
mod relay;

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use openvr::{Runtime, Signal};
use board::{Board, Press};
use placement::{above_hand, below_wrist, local_tip, Hand, Pose};
use gaze::Gaze;
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
const DORMANT_POLL: Duration = Duration::from_micros(11_111);

fn directory(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(fallback)).join("voice-vr")
}

#[derive(Default)]
struct Meter {
    window: Option<Instant>,
    interval: Vec<Duration>,
    work: Vec<Duration>,
}

fn summary(samples: &mut [Duration]) -> String {
    samples.sort();
    let mean = samples.iter().sum::<Duration>().as_secs_f64() * 1e3 / samples.len() as f64;
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)].as_secs_f64() * 1e3;
    format!("mean {mean:.1} p95 {p95:.1} ms")
}

impl Meter {
    fn frame(&mut self, interval: Duration, work: Duration) {
        let now = Instant::now();
        let window = *self.window.get_or_insert(now);
        self.interval.push(interval);
        self.work.push(work);
        let elapsed = now - window;
        if elapsed >= Duration::from_secs(5) {
            let rate = self.interval.len() as f64 / elapsed.as_secs_f64();
            eprintln!("overlay {rate:.1} fps; frame interval {}; animate+draw+submit {}", summary(&mut self.interval), summary(&mut self.work));
            *self = Meter { window: Some(now), ..Meter::default() };
        }
    }
}

struct Puppet {
    model: Model,
    skinned: Skinned,
    fit: Fit,
    standing: Humanoid,
    animator: Animator,
    gaze: Gaze,
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
    Ok(Puppet { model, skinned, fit, standing, animator: Animator::new(clips, Random::seeded()), gaze: Gaze::default(), drawn })
}

/// Loads the picked appearance beside the shown one, then makes it the server's selection, as the page's picker does.
fn switch(token: &Token, cache: &std::path::Path, avatar: &Avatar, renderer: &mut Renderer) -> Result<Puppet, String> {
    let mut relay = Relay::connect(token, cache)?;
    let loaded = puppet(&mut relay, avatar, renderer)?;
    relay.select(&avatar.id)?;
    Ok(loaded)
}

fn host(runtime: &Runtime, token: &Token, state: &std::path::Path) -> Result<(), String> {
    let gesture_file = state.join("gesture.json");
    let mut recognizer = Recognizer::new(Templates::parse(&std::fs::read(&gesture_file).map_err(|error| format!("{}: {error}", gesture_file.display()))?)?);
    let gpu = Rc::new(vulkan::Gpu::new(Some(runtime))?);
    let mut renderer = Renderer::new(gpu.clone(), [EYE, EYE], HEIGHT / render::PAGE_HEIGHT)?;
    let cache = state.join("assets");
    let mut relay = Relay::connect(token, &cache)?;
    let catalog = relay.catalog()?;
    let active = catalog.avatars.iter().position(|avatar| avatar.id == catalog.active).ok_or_else(|| format!("the selected appearance {} is not in the catalog", catalog.active))?;
    let mut puppet = puppet(&mut relay, &catalog.avatars[active], &mut renderer)?;
    let previews = preview::spawn(relay, catalog.avatars.clone());
    let mut board = Board::new(catalog.avatars.len(), active);
    let [board_width, board_height] = board.pixels();
    let mut board_image = vulkan::Flat::new(gpu, board_width, board_height)?;
    let mut overlay = runtime.create_overlay("voice.puppet", "Puppet", QUAD, true)?;
    let mut board_overlay = runtime.create_overlay("voice.board", "Board", board::WIDTH, false)?;
    let eye_offsets = runtime.eye_offsets();
    let mut meter = Meter::default();
    let epoch = Instant::now();
    let mut last = epoch;
    let mut voice = Voice::new(token.clone(), cache.clone(), state.join("voiceprint.json"));
    eprintln!("dormant");
    loop {
        if let Some(Signal::Quit) = runtime.poll() {
            return Ok(());
        }
        for (index, image) in previews.try_iter() {
            board.preview(index, image);
        }
        let started = Instant::now();
        let interval = started - last;
        last = started;
        let poses = runtime.poses();
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
                    puppet.gaze = Gaze::default();
                    board.reset();
                    meter = Meter::default();
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
        let board_pose = below_wrist(&left, &head, board.height());
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
            Some(Press::VoiceId) => voice.toggle_voice_id(),
            Some(Press::Learn) => voice.toggle_learning(),
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
        if board.voice != voice.voice_id() {
            board.voice = voice.voice_id();
            board.mark();
        }
        if let Some(image) = board.take_image() {
            board_overlay.submit(&mut board_image.upload(&image)?)?;
        }
        let delta = interval.as_secs_f32().min(0.05);
        if let Some(idle) = puppet.animator.update(delta) {
            eprintln!("idle {idle}");
        }
        let quad = above_hand(&left, &head, -FLOOR);
        let local = quad.inverse();
        let mut pose = puppet.model.pose(&puppet.animator.humanoid(&puppet.standing, puppet.model.rest_hips()));
        let worlds = puppet.model.worlds(&pose);
        let placement = puppet.fit.placement(&puppet.model, &worlds, FLOOR);
        puppet.gaze.look(&puppet.model, &mut pose, &worlds, placement.inverse().transform_point3(local.apply(head.t).into()), delta);
        puppet.model.express(&mut pose, "blink", puppet.animator.blink());
        puppet.model.express(&mut pose, "aa", voice.mouth());
        let changed = puppet.skinned.morph(&pose.weights);
        let palette = puppet.skinned.palette(&puppet.model.worlds(&pose), placement);
        puppet.drawn.update(&palette, &puppet.skinned.vertices, &changed);
        overlay.place_on(device, &left.inverse().then(&quad));
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
    host(&runtime, &Token::decode(&token)?, &state)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("voice-vr: {error}");
        std::process::exit(1);
    }
}
