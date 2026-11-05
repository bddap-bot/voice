use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::board::Board;
use crate::placement::{facing, Hand, HandPose, Pose, Vec3, TIP};
use crate::point::Pointing;
use crate::relay::{Relay, Token};
use crate::render::{eye_projection, eye_texels, Renderer, NEAREST, PAGE_HEIGHT};
use crate::vulkan::Gpu;
use crate::vrm::spring::{Contact, Contacts};
use crate::{action, grown_window, lit, load, preview, touching, FINGERTIP, FLOOR, HEIGHT, QUAD};

const FRAME: Duration = Duration::from_micros(11_111);
/// The wearer's head in the hand-held puppet's frame, its feet at the origin: a little above and in front, as the controller is held.
const HEAD: Vec3 = [0.0, -FLOOR + 0.12, 0.5];
const EYES: f32 = 0.032;
const IDLE: Duration = Duration::from_secs(3);
const GESTURE: Duration = Duration::from_millis(2500);
/// Into a gesture, where its picture is taken.
const GESTURE_PICTURE: Duration = Duration::from_millis(1000);
const POINT: Duration = Duration::from_secs(3);
const POINT_PICTURE: Duration = Duration::from_millis(1500);
/// The controller swinging the puppet side to side and twisting it, then held still while what swings settles.
const CARRIED: Duration = Duration::from_millis(2000);
const SETTLE: Duration = Duration::from_millis(1500);
const SWING: f32 = 0.15;
const TWIST: f32 = 0.6;
/// The right controller sweeping its hand side to side through the back of the hand-held puppet's head and back, at the height of `REACHED`, then drawn away while what it pushed settles.
const TOUCHED: Duration = Duration::from_millis(3000);
const REACHED: Vec3 = [0.0, 0.26, -0.04];
const SWEEP: f32 = 0.1;
/// The right controller's drawn shape in its own frame, a capsule behind its fingertip at `TIP`, standing in for a render model off any headset.
const OUTLINE: (glam::Vec3, glam::Vec3, f32) = (glam::Vec3::new(0.0, 0.0, -0.01), glam::Vec3::new(0.0, 0.0, 0.06), 0.015);
/// The fingertip's paths toward the puppet, from a start in its standing frame along a direction: at the back of the head, at its side, and down onto its top.
const PROBES: [(Vec3, Vec3); 3] = [([0.0, 0.26, -0.14], [0.0, 0.0, 1.0]), ([0.12, 0.25, 0.0], [-1.0, 0.0, 0.0]), ([0.0, 0.40, 0.0], [0.0, -1.0, 0.0])];
/// How near the fingertip stops before touching, and how long it waits there; how far it then presses on past the first touch, how far it drags across, and how fast it moves.
const SHY: f32 = 0.003;
const WAIT: Duration = Duration::from_millis(1500);
const PRESS: f32 = 0.008;
const DRAG: f32 = 0.03;
const SPEED: f32 = 0.02;
/// Every how many frames the clip keeps one: 30 a second.
const CLIP_EVERY: usize = 3;
/// What the puppet points at: where the overlay's first object stands beside a hand-held puppet.
const TARGET: Vec3 = [0.22, 0.12, 0.0];
/// The animator's seed, so every run draws the same idles at the same times and two runs' pictures compare.
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;
/// The puppet standing full size on the floor, seen from a standing wearer's eye height this far away, metres.
const GROUNDED: [f32; 3] = [2.5, 1.5, 1.0];
const GROUNDED_EYE_HEIGHT: f32 = 1.6;
/// Tangent of half the Frame's view per eye, which a grounded window is clipped to.
const GROUNDED_VIEW: f32 = 1.43;
/// Standing full size before the first grounded view, so what the growth set swinging settles first.
const GROUNDED_SETTLE: Duration = Duration::from_secs(2);

/// Wears each named appearance of the catalog, or all of it, as the host does off any headset: with `--speech`, first speaking a 48 kHz mono float recording, its mouth following the recording's level as it follows a heard reply's; its idle, carried by a controller swinging and twisting it and then held still, touched by the other controller's hand sweeping through the back of its head, each gesture the page can reach while speaking, a point at a spot beside it, and standing full size on the floor seen from each of `GROUNDED`'s distances, each frame posed, skinned and drawn for both eyes through the host's own path.
/// Writes the revealed appearance library, a picture of each phase and a 30 frames a second clip of the hand-held run per appearance under the out directory, and reports each appearance's per-frame work and, where the kernel counts it, its GPU time, the hand-held run's and each grounded view's apart.
/// A grounded view's picture is the left eye as the display shows it: the window's image resampled to the display's density there.
pub fn run(token: &Token, state: &Path, args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr wear <out-dir> [--speech <audio.f32>] [<catalog id>...]";
    let (out, mut ids) = args.split_first().ok_or(usage)?;
    let speech = match ids {
        [flag, path, rest @ ..] if flag == "--speech" => {
            ids = rest;
            let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
            Some(crate::audio::floats(&bytes).collect::<Vec<f32>>())
        }
        _ => None,
    };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    let cache = state.join("assets");
    let mut relay = Relay::connect(token, &cache)?;
    let catalog = relay.catalog()?;
    let worn: Vec<usize> = if ids.is_empty() {
        (0..catalog.avatars.len()).collect()
    } else {
        ids.iter().map(|id| catalog.avatars.iter().position(|avatar| &avatar.id == id).ok_or_else(|| format!("appearance {id} is not in the catalog"))).collect::<Result<_, _>>()?
    };
    // Made first, as the host does: were the preview thread's device the process's only one, the loader would unload the driver before that thread's exit runs the driver's thread-local destructors.
    let gpu = std::rc::Rc::new(Gpu::new(None)?);
    library(&catalog.avatars, worn[0], token, &cache, &out.join("library.png"))?;
    let mut renderer = Renderer::new(gpu, [eye_texels(QUAD, NEAREST); 2], HEIGHT / PAGE_HEIGHT)?;
    for index in worn {
        let avatar = &catalog.avatars[index];
        let pictures = out.join(&avatar.id);
        let clip = pictures.join("clip");
        std::fs::create_dir_all(&clip).map_err(|error| format!("{}: {error}", clip.display()))?;
        let mut puppet = load(&mut relay, &avatar.id, &mut renderer, &Default::default())?;
        puppet.animator.reseed(crate::motion::Random(SEED));
        let mut pointing = Pointing::default();
        let mut work = Vec::new();
        let mut played = Vec::new();
        let gpu_before = gpu_time();
        let mut clipped = 0;
        let mut motion = Motion::default();
        let mut constraints = Motion::default();
        let outlines = HashMap::from([(Hand::Right, OUTLINE)]);
        let mut frame = |puppet: &mut crate::Puppet, pointing: &mut Pointing, stand: Pose, hand: Option<Pose>, mouth: f32, picture: Option<&Path>| -> Result<Vec<glam::Mat4>, String> {
            let started = Instant::now();
            let hands: Vec<HandPose> = hand.into_iter().map(|pose| HandPose { hand: Hand::Right, pose }).collect();
            let (posed, body, _) = puppet.step(FRAME.as_secs_f32(), |_, _| stand, 1.0, HEAD, &touching(&hands, TIP, &outlines), mouth, pointing, started, |_| Some(TARGET));
            let quad = facing(stand.apply([0.0, -FLOOR, 0.0]), HEAD);
            let local = quad.inverse();
            puppet.draw(&posed, local.mat4() * body);
            let eyes = [-EYES, EYES].map(|x| eye_projection(local.apply([HEAD[0] + x, HEAD[1], HEAD[2]]).into(), QUAD / 2.0, QUAD / 2.0));
            renderer.render(&puppet.drawn, eyes, glam::Mat4::IDENTITY, renderer.eye[0])?;
            let touched = puppet.reach.iter().any(|reach| reach.gap < 0.0);
            work.push(started.elapsed());
            let kept = (work.len() % CLIP_EVERY == 1).then(|| clip.join(format!("{clipped:05}.png")));
            if kept.is_some() {
                for (node, name, rotation, position) in puppet.model.spring_motion(&posed) {
                    let position = body.transform_point3(position);
                    record_motion(&mut motion, node, name, rotation, position);
                }
                for (node, name, rotation, position) in puppet.model.constraint_motion(&posed, body) {
                    record_motion(&mut constraints, node, name, rotation, position);
                }
            }
            if picture.is_some() || kept.is_some() {
                let mut image = eye(&renderer.read()?, renderer.eye[0]);
                if let (Some(hand), Some(projection)) = (hand, eyes[0]) {
                    mark(&mut image, projection * local.mat4(), &hand, touched);
                }
                for path in picture.into_iter().chain(kept.as_deref()) {
                    image.save(path).map_err(|error| format!("{}: {error}", path.display()))?;
                }
                clipped += kept.is_some() as usize;
            }
            if let Some(wait) = (started + FRAME).checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            Ok(posed)
        };
        let frames = |span: Duration| (span.as_secs_f64() / FRAME.as_secs_f64()) as usize;
        if let Some(samples) = &speech {
            let mut mouth = 0.0;
            let per_frame = FRAME.as_secs_f64() * crate::audio::RATE as f64;
            for at in 0..=(samples.len() as f64 / per_frame).ceil() as usize {
                mouth = crate::audio::follow(mouth, crate::audio::level_at(samples, (at as f64 * per_frame) as usize));
                frame(&mut puppet, &mut pointing, HELD, None, mouth, None)?;
            }
        }
        for at in 0..frames(IDLE) {
            frame(&mut puppet, &mut pointing, HELD, None, 0.0, (at + 1 == frames(IDLE)).then(|| pictures.join("idle.png")).as_deref())?;
        }
        for at in 0..frames(CARRIED + SETTLE) {
            frame(&mut puppet, &mut pointing, carried(at as f32 / frames(CARRIED) as f32), None, 0.0, None)?;
        }
        for at in 0..frames(TOUCHED + SETTLE) {
            let progress = at as f32 / frames(TOUCHED) as f32;
            frame(&mut puppet, &mut pointing, HELD, (progress < 1.0).then(|| sweeping(progress)), 0.0, (at == frames(TOUCHED) / 4).then(|| pictures.join("touched.png")).as_deref())?;
        }
        for gesture in action::gestures() {
            if !puppet.animator.gesture(gesture) {
                continue;
            }
            played.push(gesture);
            for at in 0..frames(GESTURE) {
                frame(&mut puppet, &mut pointing, HELD, None, speaking(at), (at == frames(GESTURE_PICTURE)).then(|| pictures.join(format!("gesture-{gesture}.png"))).as_deref())?;
            }
        }
        let mut rows = String::from("seconds,probe,phase,gap_mm,bone,turn_deg_since_near\n");
        let mut log = String::new();
        let mut contacts = Contacts::default();
        let mut clock = 0usize;
        for (probe, &(start, toward)) in PROBES.iter().enumerate() {
            let toward = glam::Vec3::from(toward);
            let across = toward.any_orthonormal_vector();
            let held = |at: glam::Vec3| {
                let z = -toward;
                let x = across;
                let pose = Pose::from_axes(x.into(), z.cross(x).into(), z.into(), [0.0; 3]);
                Pose { t: (at - glam::Vec3::from(pose.apply(TIP))).into(), ..pose }
            };
            let step = SPEED * FRAME.as_secs_f32();
            let mut at = glam::Vec3::from(start);
            let mut phase = Phase::Approach(0.0);
            let mut rest: HashMap<usize, glam::Quat> = HashMap::new();
            let mut picture = None;
            while phase != Phase::Done {
                let posed = frame(&mut puppet, &mut pointing, HELD, (!matches!(phase, Phase::Settle(_))).then(|| held(at)), 0.0, picture.take().map(|name: String| pictures.join(name)).as_deref())?;
                clock += 1;
                let seconds = clock as f32 * FRAME.as_secs_f32();
                let tip = puppet.reach.iter().find(|reach| reach.hand == FINGERTIP).copied();
                let gap = tip.map_or(f32::INFINITY, |tip| tip.gap);
                let bone = |node: Option<usize>| node.and_then(|node| puppet.model.name(node)).unwrap_or_default();
                let turn = tip.and_then(|tip| tip.node).and_then(|node| Some((*rest.get(&node)?, node))).and_then(|(rest, node)| puppet.model.spring_motion(&posed).into_iter().find(|&(at, ..)| at == node).map(|(_, _, rotation, _)| rest.angle_between(rotation).to_degrees()));
                for event in contacts.observe(&puppet.reach) {
                    let line = match event {
                        Contact::Touched { hand, gap, node } => format!("{seconds:.3} s probe {probe}: {hand} touched {} {:.2} mm deep", bone(node), -gap * 1e3),
                        Contact::Released { hand } => format!("{seconds:.3} s probe {probe}: {hand} let go"),
                        Contact::Passed { hand, closest, node } => format!("{seconds:.3} s probe {probe}: {hand} passed {} {:.2} mm away without touching", bone(node), closest * 1e3),
                    };
                    log.push_str(&line);
                    log.push('\n');
                }
                rows.push_str(&format!("{seconds:.3},{probe},{},{},{},{}\n", phase.name(), if gap.is_finite() { format!("{:.3}", gap * 1e3) } else { String::new() }, bone(tip.and_then(|tip| tip.node)), turn.map_or(String::new(), |turn| format!("{turn:.3}"))));
                phase = match phase {
                    Phase::Approach(travelled) if gap <= SHY => {
                        rest = puppet.model.spring_motion(&posed).into_iter().map(|(node, _, rotation, _)| (node, rotation)).collect();
                        log.push_str(&format!("{seconds:.3} s probe {probe}: fingertip held {:.2} mm from {} after {:.0} mm\n", gap * 1e3, bone(tip.and_then(|tip| tip.node)), travelled * 1e3));
                        Phase::Wait(0)
                    }
                    Phase::Approach(travelled) if travelled > 0.2 => {
                        log.push_str(&format!("{seconds:.3} s probe {probe}: nothing others may push within 200 mm of its path\n"));
                        Phase::Settle(0)
                    }
                    Phase::Approach(travelled) => {
                        at += toward * step;
                        Phase::Approach(travelled + step)
                    }
                    Phase::Wait(frames) if frames >= (WAIT.as_secs_f32() / FRAME.as_secs_f32()) as usize => Phase::Press(None),
                    Phase::Wait(frames) => {
                        if frames + 1 == (WAIT.as_secs_f32() / FRAME.as_secs_f32()) as usize {
                            picture = Some(format!("probe{probe}-near.png"));
                        }
                        Phase::Wait(frames + 1)
                    }
                    Phase::Press(Some(pressed)) if pressed >= PRESS => Phase::Drag(0.0),
                    Phase::Press(pressed) => {
                        at += toward * step / 2.0;
                        let pressed = pressed.or((gap < 0.0).then_some(0.0)).map(|pressed| pressed + step / 2.0);
                        if pressed.is_none() && gap > 0.05 {
                            Phase::Leave(0.0)
                        } else {
                            Phase::Press(pressed)
                        }
                    }
                    Phase::Drag(dragged) if dragged >= DRAG => {
                        picture = Some(format!("probe{probe}-dragged.png"));
                        Phase::Leave(0.0)
                    }
                    Phase::Drag(dragged) => {
                        at += across * step;
                        Phase::Drag(dragged + step)
                    }
                    Phase::Leave(left) if left >= 0.1 => Phase::Settle(0),
                    Phase::Leave(left) => {
                        at -= toward * step * 2.0;
                        Phase::Leave(left + step * 2.0)
                    }
                    Phase::Settle(frames) if frames >= 90 => Phase::Done,
                    Phase::Settle(frames) => Phase::Settle(frames + 1),
                    Phase::Done => Phase::Done,
                };
            }
        }
        std::fs::write(pictures.join("contact.csv"), rows).map_err(|error| format!("{}: {error}", pictures.display()))?;
        std::fs::write(pictures.join("contact.log"), &log).map_err(|error| format!("{}: {error}", pictures.display()))?;
        print!("{}: contact\n{log}", avatar.id);
        pointing.start("target".into(), Instant::now());
        for at in 0..frames(POINT) {
            frame(&mut puppet, &mut pointing, HELD, None, speaking(at), (at == frames(POINT_PICTURE)).then(|| pictures.join("point.png")).as_deref())?;
        }
        drop(frame);
        let held_gpu = gpu_before.zip(gpu_time()).map(|(before, after)| (after - before) as f64 / 1e6 / work.len() as f64);
        stand_grounded(&mut puppet, &mut renderer, GROUNDED[0], GROUNDED_SETTLE, None)?;
        let mut grounded = Vec::new();
        for distance in GROUNDED {
            grounded.push(stand_grounded(&mut puppet, &mut renderer, distance, Duration::from_secs(1), Some(&pictures.join(format!("grounded-{distance}m.png"))))?);
        }
        write_motion(motion, clipped, &pictures.join("spring-motion.json"))?;
        write_motion(constraints, clipped, &pictures.join("constraint-motion.json"))?;
        let pointed = pointing.hand().map_or("no reach".to_owned(), |hand| format!("{hand:?} hand"));
        let gpu = held_gpu.map_or("not counted".to_owned(), |ms| format!("{ms:.2} ms"));
        let measured: Vec<f64> = work.iter().skip(45).map(|time| time.as_secs_f64() * 1e3).collect();
        let mut sorted = measured.clone();
        sorted.sort_by(f64::total_cmp);
        let report = format!(
            "{}: {} vertices, {} joints, {} spring joints; gestures {played:?}; pointed with the {pointed}; {} frames past the first 45: work mean {:.2} p95 {:.2} max {:.2} ms; GPU {gpu} per frame; {}",
            avatar.id,
            puppet.skinned.vertices.len(),
            puppet.skinned.joints(),
            puppet.model.spring_joints(),
            measured.len(),
            measured.iter().sum::<f64>() / measured.len() as f64,
            sorted[sorted.len() * 95 / 100],
            sorted[sorted.len() - 1],
            grounded.join("; "),
        );
        println!("{report}");
        std::fs::write(pictures.join("cost.txt"), report + "\n").map_err(|error| format!("{}: {error}", pictures.display()))?;
    }
    Ok(())
}

/// The puppet standing full size on the floor `distance` metres ahead of the wearer, drawn as the host draws it there for `span`; with a `path`, the GPU time per frame is measured and the last frame's picture written there, and the window's texels per eye against the display's need said.
fn stand_grounded(puppet: &mut crate::Puppet, renderer: &mut Renderer, distance: f32, span: Duration, path: Option<&Path>) -> Result<String, String> {
    let full = puppet.full;
    let stand = HELD;
    let head = Pose { t: [0.0, GROUNDED_EYE_HEIGHT, distance], ..HELD };
    let eyes = [-EYES, EYES].map(|x| head.apply([x, 0.0, 0.0]));
    let frames = (span.as_secs_f64() / FRAME.as_secs_f64()) as usize;
    let held_outline = renderer.outline_scale;
    renderer.outline_scale = HEIGHT * full / PAGE_HEIGHT;
    let before = gpu_time();
    let mut shown = (0, 0);
    for _ in 0..frames {
        let (posed, body, _) = puppet.step(FRAME.as_secs_f32(), |_, _| stand, full, head.t, &[], 0.0, &mut Pointing::default(), Instant::now(), |_| None);
        let (quad, half) = grown_window(&puppet.skinned, &puppet.drawn.hidden, &posed, body, &head, &eyes, GROUNDED_VIEW);
        let local = quad.inverse();
        let figure = facing(stand.apply([0.0, -FLOOR * full, 0.0]), head.t);
        let (skin, to_window) = lit(&quad, &figure, body);
        puppet.draw(&posed, skin);
        let seen = eyes.map(|eye| local.apply(eye));
        let depth = seen[0][2].min(seen[1][2]);
        let texels = renderer.texels(2.0 * half, depth);
        renderer.render(&puppet.drawn, seen.map(|eye| eye_projection(eye.into(), half, half)), to_window, texels)?;
        shown = (texels, eye_texels(2.0 * half, depth));
    }
    let gpu = before.zip(gpu_time()).map_or("not counted".to_owned(), |(before, after)| format!("{:.2} ms", (after - before) as f64 / 1e6 / frames as f64));
    renderer.outline_scale = held_outline;
    let (texels, needed) = shown;
    let Some(path) = path else { return Ok(String::new()) };
    let window = eye(&renderer.read()?, texels);
    image::imageops::resize(&window, needed, needed, image::imageops::FilterType::Triangle).save(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(format!("grounded at {distance} m: {texels} texels per eye for the display's {needed}, GPU {gpu} per frame"))
}

/// Where a probe of the fingertip toward the puppet is: coming nearer by the distance so far, waiting a little short of touching, pressing on past the first touch, dragging across, drawing away, and its hand gone while what it moved settles.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Approach(f32),
    Wait(usize),
    Press(Option<f32>),
    Drag(f32),
    Leave(f32),
    Settle(usize),
    Done,
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Phase::Approach(_) => "approach",
            Phase::Wait(_) => "near",
            Phase::Press(_) => "press",
            Phase::Drag(_) => "drag",
            Phase::Leave(_) => "leave",
            Phase::Settle(_) | Phase::Done => "settle",
        }
    }
}

/// Draws the right controller into a picture through `to_eye`, the eye's projection of the window: its drawn shape grey and its fingertip green, red while the hand touches.
fn mark(image: &mut image::RgbImage, to_eye: glam::Mat4, hand: &Pose, touched: bool) {
    let size = image.width() as f32;
    let pixel = |at: glam::Vec3| {
        let clip = to_eye * at.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        glam::Vec2::new((ndc.x + 1.0) / 2.0 * size, (ndc.y + 1.0) / 2.0 * size)
    };
    let mut disc = |at: glam::Vec3, radius: f32, colour: [u8; 3], ring: bool| {
        let centre = pixel(at);
        let reach = (pixel(at + glam::Vec3::from(hand.apply([radius, 0.0, 0.0])) - glam::Vec3::from(hand.t)) - centre).length().max(2.0);
        for y in (centre.y - reach).floor().max(0.0) as u32..((centre.y + reach).ceil() as u32).min(image.height()) {
            for x in (centre.x - reach).floor().max(0.0) as u32..((centre.x + reach).ceil() as u32).min(image.width()) {
                let distance = (glam::Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - centre).length();
                if distance <= reach && (!ring || distance >= reach - 1.5) {
                    image.put_pixel(x, y, image::Rgb(colour));
                }
            }
        }
    };
    let (from, to, radius) = OUTLINE;
    for step in 0..=6 {
        disc(hand.apply(from.lerp(to, step as f32 / 6.0).into()).into(), radius, [150, 150, 150], true);
    }
    disc(hand.apply(TIP).into(), crate::FINGER_RADIUS, if touched { [230, 40, 40] } else { [40, 220, 80] }, false);
}

const HELD: Pose = Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

/// The controller `progress` of the way through two swings side to side, each twisting the puppet about its upright, and held still past the end.
fn carried(progress: f32) -> Pose {
    let phase = std::f32::consts::TAU * 2.0 * progress.min(1.0);
    let (sin, cos) = (TWIST * phase.sin()).sin_cos();
    Pose::from_axes([cos, 0.0, -sin], [0.0, 1.0, 0.0], [sin, 0.0, cos], [SWING * phase.sin(), 0.0, 0.0])
}

/// The right controller `progress` of the way through its sweep: from beside the head across behind it and back, upright and pointing at the puppet.
fn sweeping(progress: f32) -> Pose {
    let x = -SWEEP * (std::f32::consts::TAU * progress).cos();
    Pose { t: [REACHED[0] + x, REACHED[1], REACHED[2] - TIP[2]], ..HELD }
}

/// A mouth opening and closing as speech does.
fn speaking(frame: usize) -> f32 {
    0.3 + 0.25 * (frame as f32 * 0.7).sin()
}

/// The board with its appearance library revealed and every preview drawn, `active` framed.
fn library(avatars: &[crate::relay::Avatar], active: usize, token: &Token, cache: &Path, path: &Path) -> Result<(), String> {
    let mut board = Board::new(avatars.len(), active);
    board.reveal(Instant::now());
    for (id, image) in preview::spawn(token.clone(), cache.to_owned()) {
        if let Some(index) = avatars.iter().position(|avatar| avatar.id == id) {
            board.preview(index, image);
        }
    }
    let canvas = board.take_image().ok_or("the board drew nothing")?;
    let image = image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.pixels).ok_or("the board's pixels do not fit its size")?;
    image.save(path).map_err(|error| format!("{}: {error}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

/// The left eye of a stereo render, its premultiplied colour over a dark backdrop.
pub fn eye(stereo: &[u8], eye: u32) -> image::RgbImage {
    image::RgbImage::from_fn(eye, eye, |x, y| {
        let at = ((y * 2 * eye + x) * 4) as usize;
        let alpha = stereo[at + 3] as f32 / 255.0;
        let shade = 0.17 - 0.08 * y as f32 / eye as f32;
        image::Rgb([0, 1, 2].map(|channel| ((stereo[at + channel] as f32 / 255.0 + shade * (1.0 - alpha)).clamp(0.0, 1.0) * 255.0).round() as u8))
    })
}

/// This process's GPU time, in nanoseconds, from the DRM clients' `drm-engine-*` counters in its fdinfo; None where the driver keeps none.
fn gpu_time() -> Option<u64> {
    let mut clients: HashMap<String, u64> = HashMap::new();
    for entry in std::fs::read_dir("/proc/self/fdinfo").ok()?.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        let Some(client) = text.lines().find_map(|line| line.strip_prefix("drm-client-id:")) else { continue };
        let engines = text.lines().filter_map(|line| line.strip_prefix("drm-engine-")).filter_map(|line| line.split_once(':')?.1.trim().strip_suffix("ns")?.trim().parse::<u64>().ok()).sum();
        clients.insert(client.trim().to_owned(), engines);
    }
    (!clients.is_empty()).then(|| clients.values().sum())
}


type Motion = std::collections::BTreeMap<usize, (String, glam::Quat, f32, glam::Vec3, glam::Vec3)>;

fn record_motion(motion: &mut Motion, node: usize, name: String, rotation: glam::Quat, position: glam::Vec3) {
    let entry = motion.entry(node).or_insert((name, rotation, 0.0, position, position));
    entry.2 = entry.2.max(entry.1.angle_between(rotation));
    entry.3 = entry.3.min(position);
    entry.4 = entry.4.max(position);
}

fn write_motion(motion: Motion, samples: usize, path: &Path) -> Result<(), String> {
    let rows: Vec<_> = motion.into_iter().map(|(node, (name, _, turn, min, max))| serde_json::json!({"node": node, "name": name, "maxTurnRad": turn, "worldMin": min.to_array(), "worldMax": max.to_array(), "samples": samples})).collect();
    std::fs::write(path, serde_json::to_vec_pretty(&rows).map_err(|error| error.to_string())?).map_err(|error| error.to_string())
}
