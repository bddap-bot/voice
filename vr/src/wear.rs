use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::board::Board;
use crate::placement::{facing, Pose, Vec3};
use crate::point::Pointing;
use crate::relay::{Relay, Token};
use crate::render::{eye_projection, eye_texels, Renderer, NEAREST, PAGE_HEIGHT};
use crate::vulkan::Gpu;
use crate::{action, load, preview, FLOOR, HEIGHT, QUAD};

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
/// Every how many frames the clip keeps one: 30 a second.
const CLIP_EVERY: usize = 3;
/// What the puppet points at: where the overlay's first object stands beside a hand-held puppet.
const TARGET: Vec3 = [0.22, 0.12, 0.0];

/// Wears each named appearance of the catalog, or all of it, as the host does off any headset: its idle, carried by a controller swinging and twisting it and then held still, each gesture the page can reach while speaking, and a point at a spot beside it, each frame posed, skinned and drawn for both eyes through the host's own path.
/// Writes the revealed appearance library, a picture of each phase and a 30 frames a second clip of the whole run per appearance under the out directory, and reports each appearance's per-frame work and, where the kernel counts it, its GPU time.
pub fn run(token: &Token, state: &Path, args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr wear <out-dir> [<catalog id>...]";
    let (out, ids) = args.split_first().ok_or(usage)?;
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
    library(&catalog.avatars, worn[0], Relay::connect(token, &cache)?, &out.join("library.png"))?;
    let mut renderer = Renderer::new(gpu, [eye_texels(QUAD, NEAREST); 2], HEIGHT / PAGE_HEIGHT)?;
    for index in worn {
        let avatar = &catalog.avatars[index];
        let pictures = out.join(&avatar.id);
        let clip = pictures.join("clip");
        std::fs::create_dir_all(&clip).map_err(|error| format!("{}: {error}", clip.display()))?;
        let mut puppet = load(&mut relay, avatar, &mut renderer, &Default::default())?;
        let mut pointing = Pointing::default();
        let mut work = Vec::new();
        let mut played = Vec::new();
        let gpu_before = gpu_time();
        let mut clipped = 0;
        let mut motion = Motion::default();
        let mut constraints = Motion::default();
        let mut frame = |puppet: &mut crate::Puppet, pointing: &mut Pointing, stand: Pose, mouth: f32, picture: Option<&Path>| -> Result<(), String> {
            let started = Instant::now();
            let (posed, body, _) = puppet.step(FRAME.as_secs_f32(), |_, _| stand, 1.0, HEAD, mouth, pointing, started, |_| Some(TARGET));
            let quad = facing(stand.apply([0.0, -FLOOR, 0.0]), HEAD);
            let local = quad.inverse();
            puppet.draw(&posed, local.mat4() * body);
            let eyes = [-EYES, EYES].map(|x| eye_projection(local.apply([HEAD[0] + x, HEAD[1], HEAD[2]]).into(), QUAD / 2.0, QUAD / 2.0));
            renderer.render(&puppet.drawn, eyes, renderer.eye[0])?;
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
                let stereo = renderer.read()?;
                for path in picture.into_iter().chain(kept.as_deref()) {
                    save(&stereo, renderer.eye[0], path)?;
                }
                clipped += kept.is_some() as usize;
            }
            if let Some(wait) = (started + FRAME).checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            Ok(())
        };
        let frames = |span: Duration| (span.as_secs_f64() / FRAME.as_secs_f64()) as usize;
        for at in 0..frames(IDLE) {
            frame(&mut puppet, &mut pointing, HELD, 0.0, (at + 1 == frames(IDLE)).then(|| pictures.join("idle.png")).as_deref())?;
        }
        for at in 0..frames(CARRIED + SETTLE) {
            frame(&mut puppet, &mut pointing, carried(at as f32 / frames(CARRIED) as f32), 0.0, None)?;
        }
        for gesture in action::gestures() {
            if !puppet.animator.gesture(gesture) {
                continue;
            }
            played.push(gesture);
            for at in 0..frames(GESTURE) {
                frame(&mut puppet, &mut pointing, HELD, speaking(at), (at == frames(GESTURE_PICTURE)).then(|| pictures.join(format!("gesture-{gesture}.png"))).as_deref())?;
            }
        }
        pointing.start("target".into(), Instant::now());
        for at in 0..frames(POINT) {
            frame(&mut puppet, &mut pointing, HELD, speaking(at), (at == frames(POINT_PICTURE)).then(|| pictures.join("point.png")).as_deref())?;
        }
        write_motion(motion, clipped, &pictures.join("spring-motion.json"))?;
        write_motion(constraints, clipped, &pictures.join("constraint-motion.json"))?;
        let pointed = pointing.hand().map_or("no reach".to_owned(), |hand| format!("{hand:?} hand"));
        let gpu = gpu_before.zip(gpu_time()).map_or("not counted".to_owned(), |(before, after)| format!("{:.2} ms", (after - before) as f64 / 1e6 / work.len() as f64));
        let measured: Vec<f64> = work.iter().skip(45).map(|time| time.as_secs_f64() * 1e3).collect();
        let mut sorted = measured.clone();
        sorted.sort_by(f64::total_cmp);
        let report = format!(
            "{}: {} vertices, {} joints, {} spring joints; gestures {played:?}; pointed with the {pointed}; {} frames past the first 45: work mean {:.2} p95 {:.2} max {:.2} ms; GPU {gpu} per frame",
            avatar.id,
            puppet.skinned.vertices.len(),
            puppet.skinned.joints(),
            puppet.model.spring_joints(),
            measured.len(),
            measured.iter().sum::<f64>() / measured.len() as f64,
            sorted[sorted.len() * 95 / 100],
            sorted[sorted.len() - 1],
        );
        println!("{report}");
        std::fs::write(pictures.join("cost.txt"), report + "\n").map_err(|error| format!("{}: {error}", pictures.display()))?;
    }
    Ok(())
}

const HELD: Pose = Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

/// The controller `progress` of the way through two swings side to side, each twisting the puppet about its upright, and held still past the end.
fn carried(progress: f32) -> Pose {
    let phase = std::f32::consts::TAU * 2.0 * progress.min(1.0);
    let (sin, cos) = (TWIST * phase.sin()).sin_cos();
    Pose::from_axes([cos, 0.0, -sin], [0.0, 1.0, 0.0], [sin, 0.0, cos], [SWING * phase.sin(), 0.0, 0.0])
}

/// A mouth opening and closing as speech does.
fn speaking(frame: usize) -> f32 {
    0.3 + 0.25 * (frame as f32 * 0.7).sin()
}

/// The board with its appearance library revealed and every preview drawn, `active` framed.
fn library(avatars: &[crate::relay::Avatar], active: usize, relay: Relay, path: &Path) -> Result<(), String> {
    let mut board = Board::new(avatars.len(), active);
    board.reveal(Instant::now());
    for (index, image) in preview::spawn(relay, avatars.to_vec()) {
        board.preview(index, image);
    }
    let canvas = board.take_image().ok_or("the board drew nothing")?;
    let image = image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.pixels).ok_or("the board's pixels do not fit its size")?;
    image.save(path).map_err(|error| format!("{}: {error}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn save(stereo: &[u8], eye_texels: u32, path: &Path) -> Result<(), String> {
    eye(stereo, eye_texels).save(path).map_err(|error| format!("{}: {error}", path.display()))
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
