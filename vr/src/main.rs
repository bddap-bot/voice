mod audio;
mod board;
mod chosen;
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
mod panel;
mod placement;
mod preview;
mod vulkan;
mod vrm;
mod render;
mod relay;
mod reveal;
mod wake;

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use openvr::{Runtime, Signal};
use board::{Board, Press};
use chosen::Chosen;
use placement::{beside, facing, local_tip, marker, on_controller, under_controller, Hand, Pose, Vec3, TIP};
use gaze::Gaze;
use motion::{Animator, Clip, Random, IDLES};
use gesture::{Recognizer, Templates};
use relay::{Avatar, Relay, Token};
use reveal::Reveal;
use voice::Voice;
use render::{eye_projection, Appearance, Renderer};
use vrm::spring::Springs;
use vrm::{Fit, Humanoid, Model, Skinned};

const EYE: u32 = 768;
const QUAD: f32 = 0.4;
const HEIGHT: f32 = 0.3;
const MARGIN: f32 = 0.03;
const FLOOR: f32 = -QUAD / 2.0 + MARGIN;
const DORMANT_POLL: Duration = Duration::from_micros(11_111);
const SPOTTER_RETRY: Duration = Duration::from_secs(10);

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
    springs: Springs,
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
    Ok(Puppet { model, skinned, fit, standing, animator: Animator::new(clips, Random::seeded()), gaze: Gaze::default(), springs: Springs::default(), drawn })
}

/// Loads the picked appearance beside the shown one, then records it as the overlay's own choice.
fn switch(token: &Token, cache: &std::path::Path, chosen: &Chosen, avatar: &Avatar, renderer: &mut Renderer) -> Result<Puppet, String> {
    let loaded = puppet(&mut Relay::connect(token, cache)?, avatar, renderer)?;
    chosen.write(&avatar.id)?;
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
    let chosen = Chosen::beside(&cache);
    let active = chosen.read(&catalog.avatars)?;
    let mut puppet = puppet(&mut relay, &catalog.avatars[active], &mut renderer)?;
    let mut previews = None;
    let mut board = Board::new(catalog.avatars.len(), active);
    let mut reveal = Reveal::new(HEIGHT);
    let mut board_size = board.pixels();
    let mut board_image = vulkan::Flat::new(gpu.clone(), board_size[0], board_size[1])?;
    let mut overlay = runtime.create_overlay("voice.puppet", "Puppet", QUAD, true)?;
    let mut board_overlay = runtime.create_overlay("voice.board", "Board", board::WIDTH, false)?;
    let mut marker_overlay = runtime.create_overlay("voice.marker", "Fingertip", board::MARKER_WIDTH, false)?;
    marker_overlay.above_others();
    let mut marker_texture = vulkan::Flat::new(gpu.clone(), board::MARKER_PIXELS, board::MARKER_PIXELS)?;
    marker_overlay.texture(&mut marker_texture.upload(&board::marker_image())?)?;
    let mut display_overlay = runtime.create_overlay("voice.display", "Display", panel::WIDTH, false)?;
    let mut display: Option<([f32; 2], vulkan::Flat)> = None;
    let eye_offsets = runtime.eye_offsets();
    let mut meter = Meter::default();
    let epoch = Instant::now();
    let mut last = epoch;
    let mut pressing: Option<(u32, Vec3)> = None;
    let mut calibration = (epoch, String::new());
    let mut voice = Voice::new(token.clone(), cache.clone(), state.join("voiceprint.json"));
    let wake_file = state.join("wake.json");
    let wake_head = match std::fs::read(&wake_file).map_err(|error| format!("{}: {error}", wake_file.display())).and_then(|bytes| wake::Head::parse(&bytes, wake::phrase())) {
        Ok(head) => Some(std::sync::Arc::new(head)),
        Err(error) => {
            eprintln!("no wake word, the gesture alone wakes: {error}");
            None
        }
    };
    let mut spotter: Option<wake::Spotter> = None;
    let mut spotter_retry = epoch;
    eprintln!("dormant");
    loop {
        if let Some(Signal::Quit) = runtime.poll() {
            return Ok(());
        }
        for (index, image) in previews.iter().flat_map(|previews: &std::sync::mpsc::Receiver<_>| previews.try_iter()) {
            board.preview(index, image);
        }
        let started = Instant::now();
        let interval = started - last;
        last = started;
        let poses = runtime.poses();
        let head = runtime.head(&poses);
        let hands = runtime.hands(&poses);
        let hand = |which| hands.iter().find(|hand| hand.hand == which).map(|hand| hand.pose);
        let mut summoned = false;
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
                summoned |= verdict.matched && !voice.awake();
            }
        }
        match &wake_head {
            Some(model) if !voice.awake() && !voice.muted() => {
                if spotter.is_none() && started >= spotter_retry {
                    eprintln!("listening for {:?}", wake::phrase());
                    spotter = Some(wake::Spotter::start(model.clone()));
                }
            }
            _ => spotter = None,
        }
        while let Some(heard) = spotter.as_ref().and_then(wake::Spotter::heard) {
            match heard {
                Ok(wake::Heard::Wake(score)) => {
                    eprintln!("wake word heard: score {score:.3}, waking");
                    summoned = true;
                }
                Ok(wake::Heard::Miss(peak)) => eprintln!("wake word near miss: peak {peak:.3}"),
                Err(error) => {
                    eprintln!("wake spotter: {error}");
                    spotter = None;
                    spotter_retry = started + SPOTTER_RETRY;
                }
            }
        }
        if summoned {
            spotter = None;
            voice.summon();
            puppet.gaze = Gaze::default();
            board.reset();
            display = None;
            meter = Meter::default();
        }
        voice.step();
        let (true, Some(head), Some(left), Some(device)) = (voice.awake(), head, hand(Hand::Left), runtime.hand_index(Hand::Left)) else {
            overlay.hide();
            board_overlay.hide();
            marker_overlay.hide();
            display_overlay.hide();
            board.reset();
            reveal.reset();
            puppet.springs.reset();
            std::thread::sleep(DORMANT_POLL);
            continue;
        };
        let right_device = runtime.hand_index(Hand::Right);
        if let Some(device) = right_device.filter(|&device| pressing.map_or(true, |(known, _)| known != device) && started >= calibration.0) {
            calibration.0 = started + Duration::from_secs(1);
            match runtime.tip(device) {
                Ok(tip) => {
                    eprintln!("pressing point {tip:?} on device {device}");
                    pressing = Some((device, tip));
                }
                Err(error) if error != calibration.1 => {
                    eprintln!("pressing point: {error}; the default until it reports one");
                    calibration.1 = error;
                }
                Err(_) => {}
            }
        }
        let tip = pressing.filter(|&(known, _)| Some(known) == right_device).map_or(TIP, |(_, tip)| tip);
        let stand = on_controller(&left);
        let quad = facing(stand.apply([0.0, -FLOOR, 0.0]), head.t);
        if !board.library() && reveal.hold(started, hand(Hand::Right).map(|right| local_tip(&stand, &right, tip))) {
            eprintln!("appearance library revealed");
            board.reveal();
            if previews.is_none() {
                match Relay::connect(token, &cache) {
                    Ok(relay) => previews = Some(preview::spawn(relay, catalog.avatars.clone())),
                    Err(error) => eprintln!("previews: {error}"),
                }
            }
        }
        if board.pixels() != board_size {
            board_size = board.pixels();
            board_image = vulkan::Flat::new(gpu.clone(), board_size[0], board_size[1])?;
        }
        let mount = under_controller(board.height());
        let board_pose = left.then(&mount);
        board_overlay.place_on(device, &mount);
        let fresh = voice.take_display().map(|shown| panel::draw(&shown));
        if let Some(picture) = &fresh {
            display = Some((picture.size(), vulkan::Flat::new(gpu.clone(), picture.width, picture.height)?));
        }
        if let Some((size, image)) = &mut display {
            display_overlay.place_on(device, &beside(&mount, [board::WIDTH, board.height()], *size));
            if let Some(picture) = fresh {
                display_overlay.texture(&mut image.upload(&picture.rgba)?)?;
                display_overlay.show();
            }
        }
        let right = hand(Hand::Right).zip(right_device);
        let touch = board.touch(right.map(|(right, _)| local_tip(&board_pose, &right, tip)));
        if let Some([x, y]) = touch.crossing {
            eprintln!("board crossed at ({x:.3}, {y:.3}) m: {}", touch.press.map_or("none".to_owned(), |press| format!("{press:?}")));
        }
        match (touch.marker, right) {
            (Some(at), Some((right, right_device))) => {
                marker_overlay.place_on(right_device, &marker(&board_pose, &right, at));
                marker_overlay.show();
            }
            _ => marker_overlay.hide(),
        }
        if let (Some(_), Some((_, right_device))) = (touch.press, right) {
            runtime.pulse(right_device);
        }
        match touch.press {
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
                    board_overlay.texture(&mut board_image.upload(&image)?)?;
                    board_overlay.show();
                }
                match switch(token, &cache, &chosen, avatar, &mut renderer) {
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
            board_overlay.texture(&mut board_image.upload(&image)?)?;
            board_overlay.show();
        }
        let delta = interval.as_secs_f32().min(0.05);
        if let Some(idle) = puppet.animator.update(delta) {
            eprintln!("idle {idle}");
        }
        let local = quad.inverse();
        let mut pose = puppet.model.pose(&puppet.animator.humanoid(&puppet.standing, puppet.model.rest_hips()));
        let worlds = puppet.model.worlds(&pose);
        let placement = puppet.fit.placement(&puppet.model, &worlds, 0.0);
        puppet.gaze.look(&puppet.model, &mut pose, &worlds, placement.inverse().transform_point3(stand.inverse().apply(head.t).into()), delta);
        puppet.model.express(&mut pose, "blink", puppet.animator.blink());
        puppet.model.express(&mut pose, "aa", voice.mouth());
        puppet.springs.step(&puppet.model, &mut pose, stand.mat4() * placement, delta);
        let changed = puppet.skinned.morph(&pose.weights);
        let palette = puppet.skinned.palette(&puppet.model.worlds(&pose), local.then(&stand).mat4() * placement);
        puppet.drawn.update(&palette, &puppet.skinned.vertices, &changed);
        overlay.place_on(device, &left.inverse().then(&quad));
        let eyes = eye_offsets.map(|eye: Pose| eye_projection(local.apply(head.then(&eye).t).into(), QUAD / 2.0, QUAD / 2.0));
        overlay.texture(&mut renderer.render(&puppet.drawn, eyes)?)?;
        overlay.show();
        meter.frame(interval, started.elapsed());
        runtime.wait_frame();
    }
}

/// Feeds a recording (`t,dev,role,valid,m00..m23,...` rows; roles 0 head, 1 left, 2 right) through the recognizer as the host would.
fn replay(state: &std::path::Path, recording: &str) -> Result<(), String> {
    let gesture_file = state.join("gesture.json");
    let mut recognizer = Recognizer::new(Templates::parse(&std::fs::read(&gesture_file).map_err(|error| format!("{}: {error}", gesture_file.display()))?)?);
    let text = std::fs::read_to_string(recording).map_err(|error| format!("{recording}: {error}"))?;
    let mut frame: (f64, [Option<Pose>; 3]) = (f64::NAN, [None; 3]);
    let mut start = None;
    let mut verdicts = Vec::new();
    let mut feed = |(t, poses): (f64, [Option<Pose>; 3]), verdicts: &mut Vec<(f64, gesture::Verdict)>| {
        if let [Some(head), Some(left), Some(right)] = poses {
            let t = t - *start.get_or_insert(t);
            if let Some(verdict) = recognizer.push(t, &head, left.t, right.t) {
                verdicts.push((t, verdict));
            }
        }
    };
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split(',').collect();
        let number = |i: usize| fields.get(i).and_then(|field| field.parse::<f64>().ok()).ok_or_else(|| format!("{recording}: bad row {line}"));
        let (t, role) = (number(0)?, number(2)? as usize);
        if t != frame.0 {
            feed(std::mem::replace(&mut frame, (t, [None; 3])), &mut verdicts);
        }
        if role < 3 && fields.get(3) == Some(&"1") {
            let mut m = [[0.0; 4]; 3];
            for (k, value) in m.iter_mut().flatten().enumerate() {
                *value = number(4 + k)? as f32;
            }
            frame.1[role] = Some(Pose::from_m34(&m));
        }
    }
    feed(frame, &mut verdicts);
    for (t, verdict) in &verdicts {
        println!("{t:8.2} s  distance {:.3}  peak {:.2} m/s  over {:.2} s{}", verdict.distance, verdict.peak, verdict.duration, if verdict.matched { "  MATCH" } else { "" });
    }
    let wakes = verdicts.iter().filter(|(_, verdict)| verdict.matched).count();
    println!("{} segments judged, {wakes} matches", verdicts.len());
    Ok(())
}

fn run() -> Result<(), String> {
    let config = directory("XDG_CONFIG_HOME", ".config");
    let state = directory("XDG_STATE_HOME", ".local/state");
    std::fs::create_dir_all(&state).map_err(|error| format!("{}: {error}", state.display()))?;
    if let [_, mode, recording] = &std::env::args().collect::<Vec<_>>()[..] {
        if mode == "replay" {
            return replay(&state, recording);
        }
    }
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
