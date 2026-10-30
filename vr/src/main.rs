mod action;
mod audio;
mod bench;
mod board;
mod calls;
mod chosen;
mod playback;
mod conversation;
mod device;
mod dress;
mod gaze;
mod identity;
mod session;
mod settings;
mod speaker;
mod telemetry;
mod turn;
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
mod wear;
mod measure;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use openvr::{Place, Runtime, Signal, Surface};
use board::{Board, Press};
use placement::{beside, facing, local_tip, marker, under_controller, window, Anchor, Glide, Hand, HandPose, Pose, Vec3, TIP};
use gaze::Gaze;
use motion::{Animator, Clip, Random, IDLE};
use gesture::{Recognizer, Templates};
use relay::{Avatar, Relay, Token};
use reveal::Reveal;
use voice::Voice;
use render::{eye_projection, eye_texels, Appearance, Renderer};
use vrm::expressions::{Controller, Kind};
use vrm::spring::{self, Springs};
use vrm::{Fit, Humanoid, Model, Skinned};

const QUAD: f32 = 0.4;
const HEIGHT: f32 = 0.3;
const MARGIN: f32 = 0.03;
const FLOOR: f32 = -QUAD / 2.0 + MARGIN;
/// The spheres a controller is to the puppet's springs, as another player's hand is to an avatar's: the hand around its origin and a fingertip at its tip.
const HAND_RADIUS: f32 = 0.04;
const FINGER_RADIUS: f32 = 0.01;
/// The tallest and shortest a model may stand set down full size, against a model whose units are not metres.
const FULL_HEIGHT: [f32; 2] = [0.5, 2.5];
const DORMANT_POLL: Duration = Duration::from_micros(11_111);
const SPOTTER_RETRY: Duration = Duration::from_secs(10);

fn grown_window(skinned: &Skinned, hidden: &HashSet<usize>, worlds: &[glam::Mat4], body: glam::Mat4, head: &Pose, eyes: &[Vec3], view: f32) -> (Pose, f32) {
    let points = skinned.positions(&skinned.palette(worlds, head.inverse().mat4() * body), hidden);
    let (low, high) = vrm::bounds(&points);
    // Bound the posed mesh in the view's axes, with room for its outline. A bone's
    // reach sphere can spend most of the texture on empty space around the figure.
    let corners: Vec<Vec3> = (0..8).map(|corner| {
        let point = std::array::from_fn(|axis| if corner & (1 << axis) == 0 { low[axis] - MARGIN } else { high[axis] + MARGIN });
        head.apply(point)
    }).collect();
    window(&corners, head, eyes, view)
}

/// The skinning placement and the map from it into `window`: the shader's lights are fixed in the frame it skins into,
/// so that frame is `figure`, square to the line from the head, and turning the head in place relights nothing.
fn lit(window: &Pose, figure: &Pose, body: glam::Mat4) -> (glam::Mat4, glam::Mat4) {
    (figure.inverse().mat4() * body, window.inverse().then(figure).mat4())
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
            tracing::info!("overlay {rate:.1} fps; frame interval {}; animate+draw+submit {}", summary(&mut self.interval), summary(&mut self.work));
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
    constraints: vrm::constraint::Constraints,
    drawn: Appearance,
    /// The vertices the last step's morph moved.
    changed: Vec<u32>,
    controller: Option<Controller>,
}

/// Colours across a radial's bar, enough that each authored step of a stepped one shows.
const SWATCHES: usize = 48;

/// An appearance as the server holds it now, with its played clips: everything a pick needs before the frame loop dresses it.
struct Fetched {
    model: Model,
    clips: HashMap<String, Clip>,
}

/// Fetches appearance `id` from the catalog the server serves now: one read when the host started names the files of that moment, so a republish since would be refused or worn stale.
fn fetch(relay: &mut Relay, id: &str) -> Result<Fetched, String> {
    let avatar = relay.catalog()?.avatars.into_iter().find(|avatar| avatar.id == id).ok_or_else(|| format!("appearance {id} is not in the catalog"))?;
    let model = Model::parse(&relay.puppet(&avatar)?).map_err(|error| format!("{id}: {error}"))?;
    let mut clips = HashMap::new();
    for entry in relay.clips()?.iter().filter(|entry| played(&entry.action)) {
        if !["fbx", "motion"].contains(&entry.format.as_str()) {
            tracing::info!("{}: {} clips are not played natively", entry.name, entry.format);
            continue;
        }
        match relay.motion(entry).and_then(|bytes| Clip::parse(&bytes, model.version, model.rest_hips())) {
            Ok(clip) => {
                clips.insert(entry.action.clone(), clip);
            }
            Err(error) => tracing::warn!("{}: {error}", entry.name),
        }
    }
    Ok(Fetched { model, clips })
}

/// A picked appearance on its way: its catalog index, when it was picked, and its fetch.
struct Picking {
    index: usize,
    picked: Instant,
    fetched: std::sync::mpsc::Receiver<Result<Fetched, String>>,
}

/// Fetches appearance `id` on a thread of its own, so the frame loop keeps drawing while it arrives.
fn fetch_aside(connect: impl FnOnce() -> Result<Relay, String> + Send + 'static, id: &str) -> std::sync::mpsc::Receiver<Result<Fetched, String>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let id = id.to_owned();
    std::thread::spawn(move || {
        let _ = sender.send(connect().and_then(|mut relay| fetch(&mut relay, &id)));
    });
    receiver
}

fn load(relay: &mut Relay, id: &str, renderer: &mut Renderer, remembered: &vrm::expressions::Configuration) -> Result<Puppet, String> {
    let fetched = fetch(relay, id)?;
    dressed(fetched.model, fetched.clips, id, renderer, remembered)
}

/// A puppet of `model` with its played `clips`, as `id`.
fn dressed(model: Model, mut clips: HashMap<String, Clip>, id: &str, renderer: &mut Renderer, remembered: &vrm::expressions::Configuration) -> Result<Puppet, String> {
    let standing = model.standing();
    clips.insert(motion::WALK.to_owned(), Clip::parse(include_bytes!("../motions/walk.json"), model.version, model.rest_hips())?);
    let idles = clips.keys().filter(|name| motion::family(name) == IDLE).count();
    let own = model.own_expressions();
    tracing::info!("appearance {} with {idles} standing idle clips, other clips {:?}, own expressions {own:?}, {} spring joints", id, clips.keys().filter(|name| motion::family(name) != IDLE).collect::<Vec<_>>(), model.spring_joints());
    let skinned = model.skinned()?;
    let fit = Fit::new(&model, &skinned, HEIGHT);
    let height = (HEIGHT / fit.scale).clamp(FULL_HEIGHT[0], FULL_HEIGHT[1]);
    tracing::info!("appearance {} stands {height:.2} m full size", id);
    let drawn = renderer.appearance(&model, &skinned)?;
    let controller = model.menu.as_ref().map(|menu| Controller::new(menu, remembered));
    Ok(Puppet { model, skinned, fit, full: height / HEIGHT, standing, animator: Animator::new(clips, own, Random::seeded()), gaze: Gaze::default(), springs: Springs::default(), constraints: vrm::constraint::Constraints::default(), drawn, changed: Vec::new(), controller })
}

impl Puppet {
    fn step(&mut self, delta: f32, place: impl FnOnce(Option<[Vec3; 2]>, bool) -> Pose, size: f32, head: Vec3, hands: &[spring::Hand], mouth: f32, pointing: &mut point::Pointing, now: Instant, locate: impl FnOnce(&str) -> Option<Vec3>) -> (Vec<glam::Mat4>, glam::Mat4, Pose) {
        if let Some(clip) = self.animator.update(delta) {
            tracing::info!("loop {clip}");
        }
        let mut pose = self.model.pose(&self.animator.humanoid(&self.standing, self.model.rest_hips()));
        let mut retextured = Vec::new();
        if let (Some(menu), Some(controller)) = (&self.model.menu, &mut self.controller) {
            let values = controller.step(menu, delta);
            self.drawn.hidden = menu.apply(&values, &mut pose);
            self.skinned.surface_values(menu, &values);
            for (node, moved) in menu.materials(&values) {
                retextured.extend(self.skinned.retexture(node, moved));
            }
        }
        let mut worlds = self.model.worlds(&pose);
        let fitted = self.fit.placement(&self.model, &worlds, 0.0);
        let feet = self.model.feet(&worlds).map(|feet| feet.map(|foot| fitted.transform_point3(foot).to_array()));
        let stand = place(feet, self.animator.walking());
        let body = stand.mat4() * glam::Mat4::from_scale(glam::Vec3::splat(size)) * fitted;
        let to_model = body.inverse();
        if let Some((target, weight)) = pointing.step(now, delta, mouth, locate) {
            if let Some(reach) = self.model.point_at(&mut pose, to_model.transform_point3(target.into()), weight, pointing.hand()) {
                pointing.made(reach);
            }
            worlds = self.model.worlds(&pose);
        }
        self.gaze.look(&self.model, &mut pose, &worlds, to_model.transform_point3(head.into()), delta);
        for (name, weight) in self.animator.faces() {
            self.model.express(&mut pose, name, weight);
        }
        self.model.express(&mut pose, "blink", self.animator.blink());
        self.model.express(&mut pose, "aa", mouth);
        self.springs.restore(&self.model, &mut pose);
        self.constraints.step(&self.model, &mut pose, body);
        self.springs.step(&self.model, &mut pose, body, hands, delta);
        self.changed = self.skinned.morph(&pose.weights);
        self.changed.extend(retextured);
        (self.model.worlds(&pose), body, stand)
    }

    fn draw(&mut self, posed: &[glam::Mat4], to_window: glam::Mat4) {
        let palette = self.skinned.palette(posed, to_window);
        self.drawn.update(&palette, &self.skinned, &self.changed);
    }
}

/// How long a menu button holds its parameter after a press: VRChat holds it while the button is held, and a press here is one touch.
const BUTTON_HOLD: Duration = Duration::from_millis(400);

impl Puppet {
    fn defaults(&mut self) {
        self.controller = self.model.menu.as_ref().map(|menu| Controller::new(menu, &vrm::expressions::Configuration::default()));
    }

    fn menu(&self) -> Option<board::Worn> {
        let menu = self.model.menu.as_ref()?;
        let strips = self.controller.as_ref().map(|controller| self.skinned.swatches(&self.model, menu, controller, SWATCHES)).unwrap_or_default();
        Some((menu.menu.clone(), menu.icons.clone(), strips))
    }

    fn settings(&self) -> HashMap<String, f32> {
        match (&self.model.menu, &self.controller) {
            (Some(menu), Some(controller)) => settings::shown(&menu.menu, controller),
            _ => HashMap::new(),
        }
    }
}

/// Each tracked controller as the hand spheres it pushes the puppet's springs with, the right one's fingertip at its calibrated `tip`.
fn touching(hands: &[HandPose], tip: Vec3) -> Vec<spring::Hand> {
    hands
        .iter()
        .flat_map(|hand| [(hand.pose.t, HAND_RADIUS), (hand.pose.apply(if hand.hand == Hand::Right { tip } else { TIP }), FINGER_RADIUS)])
        .map(|(center, radius)| spring::Hand { center: center.into(), radius })
        .collect()
}

fn operate(controller: &mut Controller, control: &vrm::expressions::Control, [x, y]: [f32; 2]) -> Option<String> {
    let axis = |at: usize| control.axes.get(at).map_or("", String::as_str);
    match control.kind {
        Kind::Toggle => {
            let on = controller.get(&control.parameter) == control.value;
            controller.set(&control.parameter, if on { 0.0 } else { control.value });
        }
        Kind::Button => {
            controller.set(&control.parameter, control.value);
            return Some(control.parameter.clone());
        }
        Kind::Radial | Kind::TwoAxis | Kind::FourAxis if !control.parameter.is_empty() => controller.set(&control.parameter, control.value),
        _ => {}
    }
    match control.kind {
        Kind::Radial => controller.set(axis(0), x),
        Kind::TwoAxis => {
            controller.set(axis(0), 2.0 * x - 1.0);
            controller.set(axis(1), 2.0 * y - 1.0);
        }
        Kind::FourAxis => {
            for (at, value) in [2.0 * y - 1.0, 2.0 * x - 1.0, 1.0 - 2.0 * y, 1.0 - 2.0 * x].into_iter().enumerate() {
                controller.set(axis(at), value.max(0.0));
            }
        }
        _ => {}
    }
    None
}

/// The catalog clips the overlay plays: its idles and every variant of a gesture or mood the page's classifier can reach.
fn played(clip: &str) -> bool {
    let family = motion::family(clip);
    family == IDLE || action::gestures().chain(motion::moods()).any(|name| name == family)
}

/// Acts out one of the page's decisions as the page's `applyTranscriptAction` does.
fn act(animator: &mut Animator, action: &action::Action) {
    let outcome = match action.kind {
        action::Kind::Gesture if animator.gesture(&action.name) => "played",
        action::Kind::Gesture => "refused, no clip",
        action::Kind::Mood => {
            animator.mood(&action.name);
            "drawn"
        }
        action::Kind::None => "ignored",
    };
    tracing::info!("action {:?} {} {:.3} from {:?}: {outcome}", action.kind, action.name, action.score, action.sentence);
}

fn host(runtime: Runtime, token: &Token, state: &std::path::Path) -> Result<(), String> {
    let gesture_file = state.join("gesture.json");
    let mut recognizer = Recognizer::new(Templates::parse(&std::fs::read(&gesture_file).map_err(|error| format!("{}: {error}", gesture_file.display()))?)?);
    let gpu = Rc::new(vulkan::Gpu::new(Some(runtime))?);
    let runtime = gpu.vr.as_ref().expect("the overlay's device was made for SteamVR");
    tracing::info!("display {:.0} px/rad, windows sized for {:.0} px/rad at {} m: puppet {} texels per eye, object {}", runtime.display_density(), render::DISPLAY_DENSITY, render::NEAREST, eye_texels(QUAD, render::NEAREST), eye_texels(object::QUAD, render::NEAREST));
    let mut renderer = Renderer::new(gpu.clone(), [eye_texels(QUAD, render::NEAREST); 2], HEIGHT / render::PAGE_HEIGHT)?;
    let cache = state.join("assets");
    let mut relay = Relay::connect(token, &cache)?;
    let catalog = relay.catalog()?;
    let active = chosen::read(&cache, &catalog.avatars)?;
    let mut settings = settings::Settings::read(state);
    let mut puppet = load(&mut relay, &catalog.avatars[active].id, &mut renderer, &settings.of(&catalog.avatars[active].id))?;
    let mut previews = None;
    let mut board = Board::new(catalog.avatars.len(), active);
    board.wear(puppet.menu(), puppet.settings());
    let mut released: Option<(Instant, String)> = None;
    let mut picking: Option<Picking> = None;
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
    let mut card: Option<panel::Card> = None;
    let mut objects: object::Shelf<openvr::Overlay> = object::Shelf::default();
    let mut pointing = point::Pointing::default();
    let eye_offsets = runtime.eye_offsets();
    let view = runtime.view();
    tracing::info!("the eyes see {:.0}° off the head's axis at the widest", view.atan().to_degrees());
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
            tracing::warn!("no wake word, the gesture alone wakes: {error}");
            None
        }
    };
    let mut spotter: Option<wake::Spotter> = None;
    let mut spotter_retry = epoch;
    tracing::info!("dormant");
    loop {
        if let Some(Signal::Quit) = runtime.poll() {
            tracing::info!("SteamVR asked the overlay to quit");
            return Ok(());
        }
        for (id, image) in previews.iter().flat_map(|previews: &std::sync::mpsc::Receiver<_>| previews.try_iter()) {
            if let Some(index) = catalog.avatars.iter().position(|avatar| avatar.id == id) {
                board.preview(index, image);
            }
        }
        if let Some(Picking { index, picked, fetched }) = picking.take() {
            match fetched.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => picking = Some(Picking { index, picked, fetched }),
                arrived => {
                    let id = &catalog.avatars[index].id;
                    let dressing = Instant::now();
                    match arrived.unwrap_or_else(|_| Err("its fetch ended without an answer".into())).and_then(|fetched| dressed(fetched.model, fetched.clips, id, &mut renderer, &settings.of(id))) {
                        Ok(loaded) => {
                            puppet = loaded;
                            if let Anchor::Grounded(floor) = &mut anchor {
                                *floor = placement::Floor::new(floor.pose);
                            }
                            board.active = index;
                            board.wear(puppet.menu(), puppet.settings());
                            released = None;
                            tracing::info!("appearance {id} shown {:.2} s after its pick, the frame loop held {:.2} s", picked.elapsed().as_secs_f64(), dressing.elapsed().as_secs_f64());
                        }
                        Err(error) => tracing::warn!("appearance {id} not shown: {error}"),
                    }
                    board.pending = None;
                    board.mark();
                }
            }
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
                    tracing::info!("gesture {kind}: distance {:.3} peak {:.2} m/s over {:.2} s", verdict.distance, verdict.peak, verdict.duration);
                }
                summoned |= verdict.matched && !voice.awake();
            }
        }
        match &wake_head {
            Some(model) if !voice.awake() && !voice.muted() => {
                if spotter.is_none() && started >= spotter_retry {
                    tracing::info!("listening for {:?}", wake::phrase());
                    spotter = Some(wake::Spotter::start(model.clone()));
                }
            }
            _ => spotter = None,
        }
        while let Some(heard) = spotter.as_ref().and_then(wake::Spotter::heard) {
            match heard {
                Ok(wake::Heard::Wake(score)) => {
                    tracing::info!("wake word heard: score {score:.3}, waking");
                    summoned = true;
                }
                Ok(wake::Heard::Miss(peak)) => tracing::info!("wake word near miss: peak {peak:.3}"),
                Err(error) => {
                    tracing::warn!("wake spotter: {error}");
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
            card = None;
            objects.clear("summoned");
            pointing.stop();
            meter = Meter::default();
        }
        voice.step();
        while let Some(action) = voice.take_action() {
            act(&mut puppet.animator, &action);
        }
        let (true, Some(head), Some(left), Some(device)) = (voice.awake(), head, hand(Hand::Left), runtime.hand_index(Hand::Left)) else {
            if voice.awake() {
                objects.hide();
            } else {
                objects.clear("the session ended");
            }
            overlay.hide();
            board_overlay.hide();
            marker_overlay.hide();
            display_overlay.hide();
            if let Some(card) = &mut card {
                card.reset();
            }
            board.reset();
            reveal.reset();
            puppet.springs.reset();
            puppet.constraints.reset();
            std::thread::sleep(DORMANT_POLL);
            continue;
        };
        let _frame = tracing::info_span!("frame").entered();
        let right_device = runtime.hand_index(Hand::Right);
        if let Some(device) = right_device.filter(|&device| pressing.map_or(true, |(known, _)| known != device) && started >= calibration.0) {
            calibration.0 = started + Duration::from_secs(1);
            match runtime.tip(device) {
                Ok(tip) => {
                    tracing::info!("pressing point {tip:?} on device {device}");
                    pressing = Some((device, tip));
                }
                Err(error) if error != calibration.1 => {
                    tracing::info!("pressing point: {error}; the default until it reports one");
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
        let in_puppet = hand(Hand::Right).filter(|_| size == 1.0).map(|right| local_tip(&stand, &right, tip));
        if reveal.hold(started, in_puppet) {
            tracing::info!("appearance library revealed by a tip held at {:.3?} m in the puppet's standing frame", in_puppet.unwrap_or_default());
            board.reveal(started);
            if previews.is_none() {
                previews = Some(preview::spawn(token.clone(), cache.clone()));
            }
        }
        let mount = under_controller(board.height());
        let board_pose = left.then(&mount);
        board_overlay.place_on(device, &mount);
        if let Some(mut shown) = voice.take_display() {
            tracing::info!("display shown: {}", shown.items().join(" "));
            let in_place = shown.addresses_object();
            if shown.clear {
                objects.clear("cleared by the hub");
            }
            if let Some(scene) = shown.object.take() {
                objects.show(|place| object::Object::spawn(runtime.create_overlay(&format!("voice.object.{place}"), "Object", object::QUAD, true)?, gpu.clone(), scene, object::BUDGET, started));
            }
            if let Some(data) = shown.data.take() {
                objects.feed(data.into_bytes(), shown.point.as_deref());
            }
            if let Some(hold) = shown.hold.take() {
                objects.hold(hold, shown.point.as_deref());
            }
            if let Some(name) = shown.point.take() {
                pointing.start(name, started);
            }
            if shown.card() {
                card = Some(panel::Card::new(&shown));
            } else if !in_place {
                card = None;
                display_overlay.hide();
            }
        }
        let right = hand(Hand::Right).zip(right_device);
        let touch = board.touch(right.map(|(right, _)| local_tip(&board_pose, &right, tip)));
        if let Some([x, y]) = touch.crossing {
            tracing::info!("board crossed at ({x:.3}, {y:.3}) m: {}", touch.press.map_or("none".to_owned(), |press| format!("{press:?}")));
        }
        let mut touched = touch.marker.map(|at| (board_pose, at));
        let mut turned = false;
        if let Some(card) = &mut card {
            let placed = beside(&mount, [board::WIDTH, board.height()], card.size());
            let card_pose = left.then(&placed);
            display_overlay.place_on(device, &placed);
            let touch = card.touch(right.map(|(right, _)| local_tip(&card_pose, &right, tip)));
            turned = touch.turned;
            touched = touched.or(touch.marker.map(|at| (card_pose, at)));
            if let Some(picture) = card.take_image() {
                display_overlay.texture(&mut display_image.upload(&picture)?)?;
                display_overlay.show();
            }
        }
        match (touched, right) {
            (Some((face, at)), Some((right, right_device))) => {
                marker_overlay.place_on(right_device, &marker(&face, &right, at));
                marker_overlay.show();
            }
            _ => marker_overlay.hide(),
        }
        if let (true, Some((_, right_device))) = (touch.press.is_some() || turned, right) {
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
            Some(Press::Log) => board.toggle_log(),
            Some(Press::SetDown) => anchor.toggle(started),
            Some(Press::Appearance(index)) if index != board.active => {
                if let Some(controller) = &puppet.controller {
                    if let Err(error) = settings.keep(&catalog.avatars[board.active].id, puppet.model.menu.as_ref().map_or(&[][..], |menu| &menu.menu), controller) {
                        tracing::warn!("menu settings not kept: {error}");
                    }
                }
                let avatar = &catalog.avatars[index];
                tracing::info!("picked appearance {}", avatar.id);
                board.pending = Some(index);
                board.mark();
                if let Some(image) = board.take_image() {
                    board_overlay.texture(&mut board_image.upload(&image)?)?;
                    board_overlay.show();
                }
                let (token, cache) = (token.clone(), cache.clone());
                picking = Some(Picking { index, picked: Instant::now(), fetched: fetch_aside(move || Relay::connect(&token, &cache), &avatar.id) });
            }
            Some(Press::Menu) => board.toggle_menu(),
            Some(Press::Back) => board.back(),
            Some(Press::Defaults) => {
                puppet.defaults();
                if let Err(error) = settings.forget(&catalog.avatars[board.active].id) {
                    tracing::warn!("menu settings not forgotten: {error}");
                }
            }
            Some(Press::Control(index)) => {
                let at = touch.crossing.map(|crossing| board.place_on(index, crossing)).unwrap_or([0.5; 2]);
                if let (Some(control), Some(controller)) = (board.control(index), puppet.controller.as_mut()) {
                    tracing::info!("menu {:?} {:?} at {at:.2?}", control.kind, control.name);
                    if let Some(button) = operate(controller, &control, at) {
                        released = Some((started + BUTTON_HOLD, button));
                    }
                    if let Err(error) = settings.keep(&catalog.avatars[board.active].id, puppet.model.menu.as_ref().map_or(&[][..], |menu| &menu.menu), controller) {
                        tracing::warn!("menu settings not kept: {error}");
                    }
                }
            }
            _ => {}
        }
        if let (Some((control, at)), Some(controller)) = (&touch.slide, puppet.controller.as_mut()) {
            operate(controller, control, *at);
        }
        if let (Some(_), Some(controller)) = (&touch.let_go, puppet.controller.as_ref()) {
            if let Err(error) = settings.keep(&catalog.avatars[board.active].id, puppet.model.menu.as_ref().map_or(&[][..], |menu| &menu.menu), controller) {
                tracing::warn!("menu settings not kept: {error}");
            }
        }
        if let Some((_, parameter)) = released.take_if(|(at, _)| started >= *at) {
            if let Some(controller) = puppet.controller.as_mut() {
                controller.set(&parameter, 0.0);
            }
        }
        board.set_values(puppet.settings());
        let log = voice.log(board::LOG_LINES);
        if board.log != log {
            board.log = log;
            board.mark();
        }
        if std::mem::discriminant(&board.anchor) != std::mem::discriminant(&anchor) {
            eprintln!("puppet {} at {:.2} m above the floor", match anchor { Anchor::Riding => "riding", Anchor::Settling(_) => "settling", Anchor::Stuck(_) => "set down", Anchor::Grounded(_) => "standing on the floor" }, held.t[1]);
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
        puppet.animator.walk(matches!(&anchor, Anchor::Grounded(floor) if floor.going()) && size == puppet.full);
        let full = puppet.full;
        let (posed, body, stand) = puppet.step(delta, |feet, walking| {
            match &mut anchor {
                Anchor::Grounded(floor) => {
                    let placed = floor.step(feet.map(|feet| feet.map(|foot| placement::scale(foot, full))), walking && size == full, &head, delta);
                    if size == full { placed } else { stand }
                }
                _ => stand,
            }
        }, size, head.t, &touching(&hands, tip), voice.mouth(), &mut pointing, started, |name| objects.locate(name));
        objects.step(started, &object::View { stand, size, head, eyes: eye_offsets, tip: right.map(|(right, _)| right.apply(tip)) });
        let eye_points = eye_offsets.map(|eye: Pose| head.then(&eye).t);
        let (quad, half) = if size > 1.0 { grown_window(&puppet.skinned, &puppet.drawn.hidden, &posed, body, &head, &eye_points, view) } else { (figure, QUAD / 2.0) };
        let local = quad.inverse();
        let (skin, to_window) = lit(&quad, &figure, body);
        puppet.draw(&posed, skin);
        let place = match anchor {
            Anchor::Stuck(_) | Anchor::Grounded(_) => Place::World(quad),
            _ => Place::On(device, left.inverse().then(&quad)),
        };
        renderer.outline_scale = HEIGHT * size / render::PAGE_HEIGHT;
        let seen = eye_points.map(|eye| local.apply(eye));
        let texels = renderer.texels(2.0 * half, seen[0][2].min(seen[1][2]));
        let mut texture = renderer.render(&puppet.drawn, seen.map(|eye| eye_projection(eye.into(), half, half)), to_window, texels)?;
        overlay.present(&mut texture, &place, 2.0 * half, texels as f32 / renderer.eye[0] as f32)?;
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

/// A hub display, its JSON and the trailer its image or object travels in, before the headset at the nearest a window is seen from: each card view held `seconds`, an object beside it; to check a display on the device without a session.
/// Summons a session as the wake word does, without SteamVR, and holds it open for the given seconds: the conversation path alone, for checking it on a device.
fn session(token: &Token, state: &std::path::Path, args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr session <seconds>";
    let [seconds] = args else { return Err(usage.into()) };
    let hold = Duration::from_secs(seconds.parse().map_err(|_| usage)?);
    let mut voice = Voice::new(token.clone(), state.join("assets"), state.join("voiceprint.json"));
    let summoned = Instant::now();
    voice.summon();
    let mut shown = Vec::new();
    while voice.awake() {
        voice.step();
        let log = voice.log(board::LOG_LINES);
        if log != shown {
            tracing::info!("board log: {log:?}");
            shown = log;
        }
        if voice.opened() && summoned.elapsed() >= hold {
            voice.dismiss("held long enough");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(format!("dormant {:.1} s after the summon without the session opening", summoned.elapsed().as_secs_f64()))
}

fn card(args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr card <display.json> [<trailer>] <seconds>";
    let (file, trailer, seconds) = match args {
        [file, seconds] => (file, None, seconds),
        [file, trailer, seconds] => (file, Some(trailer), seconds),
        _ => return Err(usage.into()),
    };
    let hold = Duration::from_secs(seconds.parse().map_err(|_| usage)?);
    let read = |file: &String| std::fs::read(file).map_err(|error| format!("{file}: {error}"));
    let mut shown = hub::Display::read(&read(file)?, &trailer.map(read).transpose()?.unwrap_or_default())?;
    tracing::info!("display shown: {}", shown.items().join(" "));
    let gpu = Rc::new(vulkan::Gpu::new(Some(Runtime::init()?))?);
    let runtime = gpu.vr.as_ref().expect("the card's device was made for SteamVR");
    let mut image = vulkan::Flat::new(gpu.clone());
    let mut overlay = runtime.create_overlay("voice.card", "Card", panel::WIDTH, false)?;
    overlay.place_on(openvr_sys::k_unTrackedDeviceIndex_Hmd as u32, &Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0, 0.0, -0.35] });
    let mut card = shown.card().then(|| panel::Card::new(&shown));
    let mut objects: object::Shelf<openvr::Overlay> = object::Shelf::default();
    let started = Instant::now();
    if let Some(scene) = shown.object.take() {
        objects.show(|place| object::Object::spawn(runtime.create_overlay(&format!("voice.object.{place}"), "Object", object::QUAD, true)?, gpu.clone(), scene, object::BUDGET, started));
    }
    if let Some(data) = shown.data.take() {
        objects.feed(data.into_bytes(), shown.point.as_deref());
    }
    if let Some(held) = shown.hold.take() {
        objects.hold(held, shown.point.as_deref());
    }
    let eyes = runtime.eye_offsets();
    let mut stand = None;
    let (mut view, mut turned) = (1, started);
    loop {
        runtime.wait_frame();
        let now = Instant::now();
        if now - turned >= hold {
            if !card.as_mut().is_some_and(|card| card.turn(1)) {
                break;
            }
            (view, turned) = (view + 1, now);
        }
        if let Some(card) = &mut card {
            if let Some(picture) = card.take_image() {
                overlay.texture(&mut image.upload(&picture)?)?;
                overlay.show();
                tracing::info!("view {view} of {}", card.views());
            }
        }
        if let Some(head) = runtime.head(&runtime.poses()) {
            let stand = *stand.get_or_insert_with(|| {
                let ahead = head.apply([0.0, 0.0, -0.6]);
                let mut stand = facing(ahead, head.t);
                let center = stand.apply(object::beside(1.0, 0));
                stand.t = [0, 1, 2].map(|axis| stand.t[axis] + ahead[axis] - center[axis]);
                stand
            });
            objects.step(now, &object::View { stand, size: 1.0, head, eyes, tip: None });
        }
    }
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
    if args.get(1).is_some_and(|mode| mode == "playback") {
        return playback::run(&args[2..]);
    }
    if args.get(1).is_some_and(|mode| mode == "turn") {
        return turn::replay(&args[2..]);
    }
    let config = directory("XDG_CONFIG_HOME", ".config");
    let state = directory("XDG_STATE_HOME", ".local/state");
    std::fs::create_dir_all(&state).map_err(|error| format!("{}: {error}", state.display()))?;
    if args.get(1).is_some_and(|mode| mode == "sweep") {
        return dress::sweep(&args[2..]);
    }
    if let [_, mode, recording] = &args[..] {
        if mode == "replay" {
            return replay(&state, recording);
        }
    }
    let token_file = config.join("token");
    let token = std::fs::read_to_string(&token_file).map_err(|error| format!("{}: {error}", token_file.display()))?.trim().to_owned();
    if args.get(1).is_some_and(|mode| mode == "session") {
        return session(&Token::decode(&token)?, &state, &args[2..]);
    }
    if args.get(1).is_some_and(|mode| mode == "wear") {
        return wear::run(&Token::decode(&token)?, &state, &args[2..]);
    }
    if args.get(1).is_some_and(|mode| mode == "load") {
        return measure::run(&Token::decode(&token)?, &state, &args[2..]);
    }
    if args.get(1).is_some_and(|mode| mode == "menu") {
        return dress::run(&Token::decode(&token)?, &state, &args[2..]);
    }
    let runtime = Runtime::init()?;
    host(runtime, &Token::decode(&token)?, &state)
}

fn main() {
    let telemetry = telemetry::init(directory("XDG_STATE_HOME", ".local/state").join("crashes"));
    if let Err(error) = run() {
        tracing::error!("voice-vr: {error}");
        drop(telemetry);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::sub;
    use crate::vulkan::Gpu;

    /// The Frame's half field of view per eye, as its tangent.
    const FIELD: f32 = 1.43;
    const SEEN: u32 = 384;

    struct Shot {
        direct: Vec<u8>,
        through: Vec<u8>,
    }

    fn pitched(at: Vec3, pitch: f32) -> Pose {
        let (s, c) = pitch.sin_cos();
        Pose::from_axes([1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c], at)
    }

    fn left_eye(pixels: &[u8], eye: u32) -> Vec<u8> {
        pixels.chunks_exact((eye * 8) as usize).flat_map(|row| &row[..(eye * 4) as usize]).copied().collect()
    }

    /// The left eye's view of a figure `tall` metres high standing at the origin facing +z: drawn straight into the eye's field, and through the grown window as the headset composites it.
    fn shoot(gpu: &Rc<Gpu>, tall: f32, head: Pose) -> Shot {
        let model = Model::parse(include_bytes!("../golden/figure.vrm")).unwrap();
        let mut skinned = model.skinned().unwrap();
        let fit = Fit::new(&model, &skinned, HEIGHT);
        let pose = model.pose(&model.standing());
        let changed = skinned.morph(&pose.weights);
        let worlds = model.worlds(&pose);
        let size = tall / HEIGHT;
        let body = glam::Mat4::from_scale(glam::Vec3::splat(size)) * fit.placement(&model, &worlds, 0.0);
        let eye = head.apply([-0.032, 0.0, 0.0]);
        let field = Pose { t: head.apply([0.0, 0.0, -1.0]), ..head };
        let draw = |window: &Pose, half: f32, texels: u32| {
            let mut renderer = Renderer::new(gpu.clone(), [texels; 2], HEIGHT / render::PAGE_HEIGHT).unwrap();
            renderer.outline_scale = HEIGHT * size / render::PAGE_HEIGHT;
            let mut drawn = renderer.appearance(&model, &skinned).unwrap();
            let local = window.inverse();
            drawn.update(&skinned.palette(&worlds, local.mat4() * body), &skinned, &changed);
            renderer.render(&drawn, [eye_projection(local.apply(eye).into(), half, half), None], glam::Mat4::IDENTITY, texels).unwrap();
            left_eye(&renderer.read().unwrap(), texels)
        };
        let direct = draw(&field, FIELD, SEEN);
        let (window, half) = grown_window(&skinned, &HashSet::new(), &worlds, body, &head, &[eye, head.apply([0.032, 0.0, 0.0])], FIELD);
        let texels = eye_texels(QUAD, render::NEAREST);
        let quad = draw(&window, half, texels);
        let to_window = window.inverse();
        let origin = to_window.apply(eye);
        let mut through = vec![0; direct.len()];
        for (index, pixel) in through.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((index as u32 % SEEN) as f32, (index as u32 / SEEN) as f32);
            let target = field.apply([FIELD * (2.0 * (x + 0.5) / SEEN as f32 - 1.0), FIELD * (1.0 - 2.0 * (y + 0.5) / SEEN as f32), 0.0]);
            let direction = sub(to_window.apply(target), origin);
            let distance = -origin[2] / direction[2];
            let [u, v] = [0, 1].map(|axis| origin[axis] + distance * direction[axis]);
            let (u, v) = (u / (2.0 * half) + 0.5, 0.5 - v / (2.0 * half));
            if distance > 0.0 && (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
                let at = (((v * texels as f32) as usize * texels as usize) + (u * texels as f32) as usize) * 4;
                pixel.copy_from_slice(&quad[at..at + 4]);
            }
        }
        Shot { direct, through }
    }

    fn covered(pixels: &[u8]) -> Vec<bool> {
        pixels.chunks_exact(4).map(|pixel| pixel[3] > 128).collect()
    }

    /// Covered texels of the eye's own view that the window loses, over all it covers.
    fn lost(shot: &Shot) -> f32 {
        let (direct, through) = (covered(&shot.direct), covered(&shot.through));
        let total = direct.iter().filter(|&&c| c).count();
        direct.iter().zip(&through).filter(|(&d, &t)| d && !t).count() as f32 / total.max(1) as f32
    }

    /// The top and bottom rows the figure covers.
    fn rows(pixels: &[u8]) -> (usize, usize) {
        let covered = covered(pixels);
        let row = |index: usize| index / SEEN as usize;
        (row(covered.iter().position(|&c| c).unwrap()), row(covered.iter().rposition(|&c| c).unwrap()))
    }

    fn save(shot: &Shot, name: &str) {
        let Some(directory) = std::env::var_os("VOICE_VR_CAPTURES") else { return };
        let mut image = image::RgbImage::new(SEEN * 2 + 8, SEEN);
        for (side, pixels) in [&shot.direct, &shot.through].into_iter().enumerate() {
            for (index, pixel) in pixels.chunks_exact(4).enumerate() {
                let (x, y) = (index as u32 % SEEN, index as u32 / SEEN);
                let a = pixel[3] as f32 / 255.0;
                let back = 40.0 + 30.0 * y as f32 / SEEN as f32;
                let mix = |channel: u8| (channel as f32 + back * (1.0 - a)).min(255.0) as u8;
                image.put_pixel(side as u32 * (SEEN + 8) + x, y, image::Rgb([mix(pixel[0]), mix(pixel[1]), mix(pixel[2])]));
            }
        }
        image.save(std::path::Path::new(&directory).join(format!("{name}.png"))).unwrap();
    }

    #[test]
    fn turning_the_head_in_place_does_not_relight_a_grown_puppet() {
        let model = Model::parse(include_bytes!("../golden/figure.vrm")).unwrap();
        let skinned = model.skinned().unwrap();
        let fit = Fit::new(&model, &skinned, HEIGHT);
        let worlds = model.worlds(&model.pose(&model.standing()));
        let body = glam::Mat4::from_scale(glam::Vec3::splat(1.7 / HEIGHT)) * fit.placement(&model, &worlds, 0.0);
        let at = [0.3, 1.6, 2.0];
        let figure = facing([0.0, -FLOOR * 1.7 / HEIGHT, 0.0], at);
        let drawn = |yaw: f32, pitch: f32| {
            let (s, c) = yaw.sin_cos();
            let head = Pose::from_axes([c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], at).then(&pitched([0.0; 3], pitch));
            let eyes = [-0.032, 0.032].map(|x| head.apply([x, 0.0, 0.0]));
            let (window, _) = grown_window(&skinned, &HashSet::new(), &worlds, body, &head, &eyes, FIELD);
            let (skin, to_window) = lit(&window, &figure, body);
            assert!((to_window * skin).abs_diff_eq(window.inverse().mat4() * body, 1e-4), "the window still shows the figure where it stands");
            skin
        };
        let ahead = drawn(0.0, 0.0);
        for (yaw, pitch) in [(0.35, 0.0), (-0.35, 0.0), (0.0, -0.4), (0.2, 0.3)] {
            let turned = drawn(yaw, pitch);
            let normal = |skin: glam::Mat4| skin.transform_vector3(glam::Vec3::Z).normalize();
            let angle = normal(turned).angle_between(normal(ahead)).to_degrees();
            assert!(angle < 0.01, "turning the head (yaw {yaw}, pitch {pitch}) in place turns the figure {angle:.1} degrees under the shader's lights");
        }
    }

    #[test]
    fn a_floor_puppets_window_does_not_spend_its_resolution_on_empty_bone_reach() {
        let model = Model::parse(include_bytes!("../golden/figure.vrm")).unwrap();
        let skinned = model.skinned().unwrap();
        let fit = Fit::new(&model, &skinned, HEIGHT);
        let worlds = model.worlds(&model.pose(&model.standing()));
        let body = glam::Mat4::from_scale(glam::Vec3::splat(1.7 / HEIGHT)) * fit.placement(&model, &worlds, 0.0);
        let head = pitched([0.0, 1.6, 3.0], 0.0);
        let eyes = [-0.032, 0.032].map(|x| head.apply([x, 0.0, 0.0]));
        let (quad, half) = grown_window(&skinned, &HashSet::new(), &worlds, body, &head, &eyes, FIELD);
        assert!(2.0 * half < 2.0, "the 1.7 m fixture, including its projected depth and outline margin, fits two metres: {} m", 2.0 * half);
        let depth = quad.inverse().apply(head.t)[2];
        assert!(eye_texels(2.0 * half, depth) <= eye_texels(QUAD, render::NEAREST), "at three metres the full display density fits the target");
    }

    #[test]
    fn a_grown_puppet_seen_close_shows_whole_in_the_eyes_field() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        for (name, tall, head) in [
            ("tallest-close-looking-up", FULL_HEIGHT[1], pitched([0.0, 1.6, 0.45], 0.6)),
            ("tallest-close-looking-ahead", FULL_HEIGHT[1], pitched([0.0, 1.6, 0.45], 0.0)),
            ("tallest-close-looking-down", FULL_HEIGHT[1], pitched([0.0, 1.6, 0.45], -0.9)),
            ("human-close-looking-down", 1.7, pitched([0.0, 1.6, 0.4], -0.8)),
            ("human-close-looking-up", 1.7, pitched([0.0, 1.5, 0.35], 0.3)),
            ("human-far", 1.7, pitched([0.0, 1.6, 2.0], -0.2)),
        ] {
            let shot = shoot(&gpu, tall, head);
            save(&shot, name);
            let (lost, [top, bottom]) = (lost(&shot), [0, 1].map(|end| [rows(&shot.direct), rows(&shot.through)].map(|rows| if end == 0 { rows.0 } else { rows.1 })));
            eprintln!("{name}: {:.1}% of the figure lost, top row {top:?}, bottom row {bottom:?}", lost * 100.0);
            assert!(lost < 0.04, "{name}: {:.1}% of the figure the eye sees is cut off", lost * 100.0);
            assert!(top[0].abs_diff(top[1]) <= 2 && bottom[0].abs_diff(bottom[1]) <= 2, "{name}: head and feet show where the eye sees them: top rows {top:?}, bottom rows {bottom:?}");
        }
    }

    /// A server whose catalog serves `figure.vrm` as appearance "a" now, with no clips.
    fn republished(cache: &std::path::Path) -> (String, Relay) {
        let (hash, frames) = relay::served("puppet", "a", include_bytes!("../golden/figure.vrm"));
        let catalog = serde_json::json!({ "avatars": [{ "id": "a", "contentHash": hash }] });
        (hash, relay::scripted(cache, move |verb, _| match verb {
            "puppets" => vec![format!("puppets\n{catalog}").into_bytes()],
            "clips" => vec![b"clips\n{\"clips\":[]}".to_vec()],
            "puppet" => frames.clone(),
            _ => Vec::new(),
        }))
    }

    #[test]
    fn a_pick_wears_the_file_served_now_not_the_copy_cached_under_the_hash_the_host_started_with() {
        let cache = std::env::temp_dir().join(format!("voice-vr-pick-{}", std::process::id()));
        let (_, mut relay) = republished(&cache);
        std::fs::write(cache.join("0123abcd.vrm"), b"the copy published before").unwrap();
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [64; 2], 1.0).unwrap();
        let startup = Avatar { id: "a".into(), content_hash: "0123abcd".into() };
        let worn = load(&mut relay, &startup.id, &mut renderer, &Default::default());
        std::fs::remove_dir_all(&cache).unwrap();
        assert!(worn.is_ok(), "{:?}", worn.err());
    }

    #[test]
    fn a_pick_leaves_the_frame_loop_free_while_its_appearance_arrives() {
        let cache = std::env::temp_dir().join(format!("voice-vr-aside-{}", std::process::id()));
        let (_, relay) = republished(&cache);
        let (open, gate) = std::sync::mpsc::channel::<()>();
        let fetched = fetch_aside(move || {
            gate.recv().unwrap();
            Ok(relay)
        }, "a");
        assert!(matches!(fetched.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)), "returned before the server answered");
        open.send(()).unwrap();
        let arrived = fetched.recv().unwrap();
        std::fs::remove_dir_all(&cache).unwrap();
        assert!(arrived.is_ok(), "{:?}", arrived.err());
    }
}
