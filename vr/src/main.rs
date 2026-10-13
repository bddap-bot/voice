mod action;
mod audio;
mod bench;
mod board;
mod calls;
mod chosen;
mod conversation;
mod device;
mod gaze;
mod identity;
mod session;
mod speaker;
mod voice;
mod gesture;
mod hub;
mod motion;
mod object;
mod program;
mod openvr;
mod panel;
mod placement;
mod point;
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
use placement::{beside, facing, local_tip, marker, under_controller, window, Anchor, Glide, Hand, Pose, Vec3, TIP};
use gaze::Gaze;
use motion::{Animator, Clip, Random, IDLES};
use gesture::{Recognizer, Templates};
use relay::{Avatar, Relay, Token};
use reveal::Reveal;
use voice::Voice;
use render::{eye_projection, eye_texels, Appearance, Renderer};
use vrm::spring::Springs;
use vrm::{Fit, Humanoid, Model, Skinned};

const QUAD: f32 = 0.4;
const HEIGHT: f32 = 0.3;
const MARGIN: f32 = 0.03;
const FLOOR: f32 = -QUAD / 2.0 + MARGIN;
/// The tallest and shortest a model may stand set down full size, against a model whose units are not metres.
const FULL_HEIGHT: [f32; 2] = [0.5, 2.5];
/// How far a grown puppet's surface reaches past its joints, over its height: the crown of the head past the head joint.
const PAD: f32 = 0.1;
const DORMANT_POLL: Duration = Duration::from_micros(11_111);
const SPOTTER_RETRY: Duration = Duration::from_secs(10);

/// Where an object's centre sits in the hand-held puppet's quad frame for a puppet `size` times hand size: right of the figure, about chest high.
fn object_beside(size: f32) -> Vec3 {
    [object::SIZE / 2.0 + 0.1 * size, 0.0, 0.0]
}

/// A grown puppet's window: wide enough for both eyes to see every joint of `worlds` with `PAD` of its height around each.
fn grown_window(worlds: &[glam::Mat4], body: glam::Mat4, size: f32, head: Vec3, eyes: &[Vec3]) -> (Pose, f32) {
    let joints: Vec<Vec3> = worlds.iter().map(|world| body.transform_point3(world.w_axis.truncate()).into()).collect();
    window(&joints, PAD * HEIGHT * size, head, eyes)
}

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
    /// Its own height over the hand-held `HEIGHT`: how much it grows set down on the floor.
    full: f32,
    standing: Humanoid,
    animator: Animator,
    gaze: Gaze,
    springs: Springs,
    drawn: Appearance,
}

fn load(relay: &mut Relay, avatar: &Avatar, renderer: &mut Renderer) -> Result<Puppet, String> {
    let model = Model::parse(&relay.puppet(avatar)?).map_err(|error| format!("{}: {error}", avatar.id))?;
    let standing = model.standing();
    let mut clips = HashMap::new();
    for entry in relay.clips()?.iter().filter(|entry| played(&entry.action)) {
        if entry.format != "fbx" {
            eprintln!("{}: {} clips are not played natively", entry.name, entry.format);
            continue;
        }
        match relay.motion(entry).and_then(|bytes| Clip::parse(&bytes, model.version, model.rest_hips())) {
            Ok(mut clip) => {
                if IDLES.contains(&entry.action.as_str()) {
                    clip.anchor(&standing);
                }
                clips.insert(entry.action.clone(), clip);
            }
            Err(error) => eprintln!("{}: {error}", entry.name),
        }
    }
    let idles = clips.keys().filter(|name| IDLES.contains(&name.as_str())).count();
    eprintln!("appearance {} with {idles} standing idle clips, gestures {:?}, {} spring joints at gravity {}", avatar.id, clips.keys().filter(|name| !IDLES.contains(&name.as_str())).collect::<Vec<_>>(), model.spring_joints(), vrm::spring::GRAVITY);
    let skinned = model.skinned()?;
    let fit = Fit::new(&model, &skinned, HEIGHT);
    let height = (HEIGHT / fit.scale).clamp(FULL_HEIGHT[0], FULL_HEIGHT[1]);
    eprintln!("appearance {} stands {height:.2} m full size", avatar.id);
    let drawn = renderer.appearance(&model, &skinned)?;
    Ok(Puppet { model, skinned, fit, full: height / HEIGHT, standing, animator: Animator::new(clips, Random::seeded()), gaze: Gaze::default(), springs: Springs::default(), drawn })
}

/// The catalog clips the overlay plays: its idles and every gesture the page's classifier can reach.
fn played(clip: &str) -> bool {
    IDLES.contains(&clip) || action::gestures().any(|gesture| gesture == clip)
}

/// Acts out one of the page's decisions as the page's `applyTranscriptAction` does; the page's moods are faces the overlay does not draw yet.
fn act(animator: &mut Animator, action: &action::Action) {
    let outcome = match action.kind {
        action::Kind::Gesture if animator.gesture(&action.name) => "played",
        action::Kind::Gesture => "refused, no clip",
        action::Kind::Mood => "not drawn",
        action::Kind::None => "ignored",
    };
    eprintln!("action {:?} {} {:.3} from {:?}: {outcome}", action.kind, action.name, action.score, action.sentence);
}

fn host(runtime: &Runtime, token: &Token, state: &std::path::Path) -> Result<(), String> {
    let gesture_file = state.join("gesture.json");
    let mut recognizer = Recognizer::new(Templates::parse(&std::fs::read(&gesture_file).map_err(|error| format!("{}: {error}", gesture_file.display()))?)?);
    let gpu = Rc::new(vulkan::Gpu::new(Some(runtime))?);
    eprintln!("display {:.0} px/rad, windows sized for {:.0} px/rad at {} m: puppet {} texels per eye, object {}", runtime.display_density(), render::DISPLAY_DENSITY, render::NEAREST, eye_texels(QUAD), eye_texels(object::QUAD));
    let mut renderer = Renderer::new(gpu.clone(), [eye_texels(QUAD); 2], HEIGHT / render::PAGE_HEIGHT)?;
    let cache = state.join("assets");
    let mut relay = Relay::connect(token, &cache)?;
    let catalog = relay.catalog()?;
    let active = chosen::read(&cache, &catalog.avatars)?;
    let mut puppet = load(&mut relay, &catalog.avatars[active], &mut renderer)?;
    let mut previews = None;
    let mut board = Board::new(catalog.avatars.len(), active);
    let mut reveal = Reveal::new(HEIGHT);
    let mut board_image = vulkan::Flat::new(gpu.clone());
    let mut overlay = runtime.create_overlay("voice.puppet", "Puppet", QUAD, true)?;
    let mut board_overlay = runtime.create_overlay("voice.board", "Board", board::WIDTH, false)?;
    let mut marker_overlay = runtime.create_overlay("voice.marker", "Fingertip", board::MARKER_WIDTH, false)?;
    marker_overlay.above_others();
    let mut marker_texture = vulkan::Flat::new(gpu.clone());
    marker_overlay.texture(&mut marker_texture.upload(&board::marker_image())?)?;
    let mut display_overlay = runtime.create_overlay("voice.display", "Display", panel::WIDTH, false)?;
    let mut display_image = vulkan::Flat::new(gpu.clone());
    let mut display: Option<[f32; 2]> = None;
    let mut object: object::Slot<openvr::Overlay> = object::Slot::default();
    let mut pointing = point::Pointing::default();
    let eye_offsets = runtime.eye_offsets();
    let mut meter = Meter::default();
    let epoch = Instant::now();
    let mut last = epoch;
    let mut pressing: Option<(u32, Vec3)> = None;
    let mut anchor = Anchor::Riding;
    let mut glide = Glide::default();
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
            object.clear("summoned");
            pointing.stop();
            meter = Meter::default();
        }
        voice.step();
        while let Some(action) = voice.take_action() {
            act(&mut puppet.animator, &action);
        }
        let (true, Some(head), Some(left), Some(device)) = (voice.awake(), head, hand(Hand::Left), runtime.hand_index(Hand::Left)) else {
            if voice.awake() {
                object.hide();
            } else {
                object.clear("the session ended");
            }
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
        let held = anchor.stand(started, &left, head.t);
        let (stand, size) = glide.step(started, held, if anchor.grounded() { puppet.full } else { 1.0 });
        let figure = facing(stand.apply([0.0, -FLOOR * size, 0.0]), head.t);
        board.tick(started);
        if reveal.hold(started, hand(Hand::Right).map(|right| placement::scale(local_tip(&stand, &right, tip), 1.0 / size))) {
            eprintln!("appearance library revealed");
            board.reveal(started);
            if previews.is_none() {
                match Relay::connect(token, &cache) {
                    Ok(relay) => previews = Some(preview::spawn(relay, catalog.avatars.clone())),
                    Err(error) => eprintln!("previews: {error}"),
                }
            }
        }
        let mount = under_controller(board.height());
        let board_pose = left.then(&mount);
        board_overlay.place_on(device, &mount);
        let mut fresh = None;
        if let Some(mut shown) = voice.take_display() {
            let in_place = shown.addresses_object();
            let about_object = shown.point.is_some() || shown.data.is_some() || shown.hold.is_some();
            if let Some(name) = shown.point.take() {
                pointing.start(name, started);
            }
            let card = shown.card();
            match shown.object.take() {
                Some(scene) => object.show(|| object::Object::spawn(runtime.create_overlay("voice.object", "Object", object::QUAD, true)?, gpu.clone(), scene, object::BUDGET, started)),
                None if card && !about_object => object.clear("replaced by a display without one"),
                None => {}
            }
            if let Some(data) = shown.data.take() {
                object.feed(data.into_bytes());
            }
            if let Some(hold) = shown.hold.take() {
                object.hold(hold);
            }
            if card {
                let picture = panel::draw(&shown);
                display = Some(panel::size(&picture));
                fresh = Some(picture);
            } else if !in_place {
                display = None;
                display_overlay.hide();
            }
        }
        if let Some(size) = display {
            display_overlay.place_on(device, &beside(&mount, [board::WIDTH, board.height()], size));
            if let Some(picture) = fresh {
                display_overlay.texture(&mut display_image.upload(&picture)?)?;
                display_overlay.show();
            }
        }
        let right = hand(Hand::Right).zip(right_device);
        object.step(started, &object::View { center: figure.apply(object_beside(size)), head, eyes: eye_offsets, tip: right.map(|(right, _)| right.apply(tip)) });
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
            Some(Press::Calls) => board.toggle_log(),
            Some(Press::SetDown) => anchor.toggle(started),
            Some(Press::Appearance(index)) if index != board.active => {
                let avatar = &catalog.avatars[index];
                eprintln!("picked appearance {}", avatar.id);
                board.pending = Some(index);
                board.mark();
                if let Some(image) = board.take_image() {
                    board_overlay.texture(&mut board_image.upload(&image)?)?;
                    board_overlay.show();
                }
                match Relay::connect(token, &cache).and_then(|mut relay| load(&mut relay, avatar, &mut renderer)) {
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
        if board.calls != *voice.calls() {
            board.calls = voice.calls().clone();
            board.mark();
        }
        if board.anchor != anchor {
            board.anchor = anchor;
            board.mark();
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
        let mut pose = puppet.model.pose(&puppet.animator.humanoid(&puppet.standing, puppet.model.rest_hips()));
        let worlds = puppet.model.worlds(&pose);
        let body = stand.mat4() * glam::Mat4::from_scale(glam::Vec3::splat(size)) * puppet.fit.placement(&puppet.model, &worlds, 0.0);
        let to_model = body.inverse();
        puppet.gaze.look(&puppet.model, &mut pose, &worlds, to_model.transform_point3(head.t.into()), delta);
        puppet.model.express(&mut pose, "blink", puppet.animator.blink());
        let mouth = voice.mouth();
        if let Some((target, weight)) = pointing.step(started, delta, mouth, |name| object.locate(name)) {
            puppet.model.point_at(&mut pose, to_model.transform_point3(target.into()), weight);
        }
        puppet.model.express(&mut pose, "aa", mouth);
        puppet.springs.step(&puppet.model, &mut pose, body, delta);
        let posed = puppet.model.worlds(&pose);
        let eye_points = eye_offsets.map(|eye: Pose| head.then(&eye).t);
        let (quad, half) = if size > 1.0 { grown_window(&posed, body, size, head.t, &eye_points) } else { (figure, QUAD / 2.0) };
        let local = quad.inverse();
        let changed = puppet.skinned.morph(&pose.weights);
        let palette = puppet.skinned.palette(&posed, local.mat4() * body);
        puppet.drawn.update(&palette, &puppet.skinned.vertices, &changed);
        match anchor {
            Anchor::Stuck(_) | Anchor::Grounded(_) => overlay.place_in_world(&quad),
            _ => overlay.place_on(device, &left.inverse().then(&quad)),
        }
        overlay.resize(2.0 * half);
        renderer.outline_scale = HEIGHT * size / render::PAGE_HEIGHT;
        let eyes = eye_points.map(|eye| eye_projection(local.apply(eye).into(), half, half));
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

/// A markdown file drawn as the display card and held `seconds` before the headset at the nearest a window is seen from, to check a card on the device without a session.
fn card(args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr card <file.md> <seconds>";
    let [file, seconds] = args else { return Err(usage.into()) };
    let seconds: u64 = seconds.parse().map_err(|_| usage)?;
    let markdown = std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?;
    let picture = panel::draw(&hub::Display { markdown: Some(markdown), ..hub::Display::default() });
    let runtime = Runtime::init()?;
    let mut image = vulkan::Flat::new(Rc::new(vulkan::Gpu::new(Some(&runtime))?));
    let mut overlay = runtime.create_overlay("voice.card", "Card", panel::WIDTH, false)?;
    overlay.place_on(openvr_sys::k_unTrackedDeviceIndex_Hmd as u32, &Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0, 0.0, -0.35] });
    overlay.texture(&mut image.upload(&picture)?)?;
    overlay.show();
    std::thread::sleep(Duration::from_secs(seconds));
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|mode| mode == "object") {
        return bench::run(&args[2..]);
    }
    if args.get(1).is_some_and(|mode| mode == "card") {
        return card(&args[2..]);
    }
    let config = directory("XDG_CONFIG_HOME", ".config");
    let state = directory("XDG_STATE_HOME", ".local/state");
    std::fs::create_dir_all(&state).map_err(|error| format!("{}: {error}", state.display()))?;
    if let [_, mode, recording] = &args[..] {
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
