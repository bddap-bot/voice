use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use glam::Mat4;

use crate::motion::{family, Animator, Clip, Random, IDLE};
use crate::placement::facing;
use crate::render::{eye_projection, Renderer, PAGE_HEIGHT};
use crate::vrm::body::{from_axis, Body, Held};
use crate::vrm::{Fit, Model, Pose, Skinned};
use crate::vulkan::Gpu;
use crate::{played, FLOOR, HEIGHT, QUAD};

const RATE: f32 = 30.0;
/// The deepest vertices dropped from each part pair's depth, so one stray vertex does not stand for the pair.
const STRAYS: usize = 2;
const TEXELS: u32 = 560;
/// How long past a gesture's end its return to the idle is watched, seconds.
const GESTURE_TAIL: f32 = 0.6;

/// How deep the figure's skin sinks into itself in one pose: the depth of its vertices inside the flesh capsule of a part they do not join, past how deep each lies there at rest or standing.
struct Depth {
    metres: f32,
    pair: Option<(usize, usize)>,
}

struct Mesh<'a> {
    model: &'a Model,
    skinned: &'a Skinned,
    body: &'a Body,
    /// How deep each vertex lies in a part's flesh at rest or standing: contact the mesh itself makes.
    allowed: HashMap<(usize, usize), f32>,
}

impl Mesh<'_> {
    /// Each vertex in a part it does not join, with that part and how deep.
    fn inside(&self, pose: &Pose) -> Vec<(usize, usize, usize, f32)> {
        let worlds = self.model.worlds(pose);
        let positions = self.skinned.positions(&self.skinned.palette(&worlds, Mat4::IDENTITY));
        let capsules = self.body.capsules(&worlds);
        let mut inside = Vec::new();
        for (vertex, (point, owner)) in positions.iter().zip(&self.body.owners).enumerate() {
            let Some(a) = *owner else { continue };
            for (b, (ends, radius)) in capsules.iter().enumerate() {
                if a != b && self.body.apart(a, b) {
                    let depth = radius - from_axis(*point, *ends);
                    if depth > 0.0 {
                        inside.push((vertex, a, b, depth));
                    }
                }
            }
        }
        inside
    }

    fn depth(&self, pose: &Pose) -> Depth {
        let parts = self.body.parts();
        let mut found: Vec<Vec<f32>> = vec![Vec::new(); parts * parts];
        for (vertex, a, b, depth) in self.inside(pose) {
            let past = depth - self.allowed.get(&(vertex, b)).copied().unwrap_or(0.0);
            if past > 0.0 {
                found[a.min(b) * parts + a.max(b)].push(past);
            }
        }
        found
            .into_iter()
            .enumerate()
            .filter(|(_, depths)| depths.len() > STRAYS)
            .map(|(at, mut depths)| {
                depths.sort_by(|x, y| y.total_cmp(x));
                Depth { metres: depths[STRAYS], pair: Some((at / parts, at % parts)) }
            })
            .fold(Depth { metres: 0.0, pair: None }, |worst, next| if next.metres > worst.metres { next } else { worst })
    }
}

struct Frame {
    time: f32,
    before: Depth,
    after: Depth,
    turn: f32,
    micros: f32,
}

/// Scores every played clip on each figure for self-penetration before and after `Body::separate`, played as the overlay plays it, and draws the worst clips side by side.
/// usage: voice-vr contact <out-dir> <motion-dir> [--videos <n>] [--clip <name>] [--at <seconds>] <figure.vrm>...; `<motion-dir>/<clip>.json` holds each catalog clip's retargeted motion.
pub fn run(args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr contact <out-dir> <motion-dir> [--videos <n>] [--clip <name>] [--at <seconds>] <figure.vrm>...";
    let [out, motions, rest @ ..] = args else { return Err(usage.into()) };
    let (mut videos, mut only, mut at, mut figures) = (0, None, None, Vec::new());
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--videos" => videos = rest.next().and_then(|count| count.parse::<usize>().ok()).ok_or(usage)?,
            "--clip" => only = Some(rest.next().ok_or(usage)?.clone()),
            "--at" => at = Some(rest.next().and_then(|seconds| seconds.parse::<f32>().ok()).ok_or(usage)?),
            _ => figures.push(arg.clone()),
        }
    }
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    let mut names: Vec<String> = std::fs::read_dir(motions)
        .map_err(|error| format!("{motions}: {error}"))?
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.strip_suffix(".json").map(str::to_owned))
        .filter(|name| played(name))
        .collect();
    names.sort();
    let mut table = String::from("| figure | clip | frames | before: deepest cm | before: frames > 1 cm | after: deepest cm | after: frames > 1 cm | largest turn ° | separate µs mean / max |\n|---|---|---|---|---|---|---|---|---|\n");
    let mut ranked = Vec::new();
    for figure in &figures {
        let id = Path::new(figure).file_stem().and_then(|stem| stem.to_str()).unwrap_or(figure).to_owned();
        let model = Model::parse(&std::fs::read(figure).map_err(|error| format!("{figure}: {error}"))?).map_err(|error| format!("{figure}: {error}"))?;
        let skinned = model.skinned()?;
        let fitted = Instant::now();
        let body = Body::fit(&model, &skinned);
        eprintln!("{id}: {} parts fitted in {:.1} ms\n{}", body.parts(), fitted.elapsed().as_secs_f64() * 1e3, body.describe(&model));
        let standing = model.standing();
        let mut mesh = Mesh { model: &model, skinned: &skinned, body: &body, allowed: HashMap::new() };
        let touching: Vec<(usize, usize, usize, f32)> = [model.rest(), model.pose(&standing)].iter().flat_map(|pose| mesh.inside(pose)).collect();
        for (vertex, _, b, depth) in touching {
            let allowed = mesh.allowed.entry((vertex, b)).or_insert(0.0);
            *allowed = allowed.max(depth);
        }
        for name in names.iter().filter(|name| only.as_ref().is_none_or(|only| only == *name)) {
            let (mut animator, warm, count) = play(&model, Path::new(motions), name)?;
            let mut frames = Vec::new();
            let mut held = Held::default();
            for at in 0..warm + count {
                let authored = model.pose(&animator.humanoid(&standing, model.rest_hips()));
                let mut pose = authored.clone();
                let started = Instant::now();
                let turn = body.separate(&model, &mut pose, &mut held, 1.0 / RATE);
                let micros = started.elapsed().as_secs_f32() * 1e6;
                if at >= warm {
                    frames.push(Frame { time: (at - warm) as f32 / RATE, before: mesh.depth(&authored), after: mesh.depth(&pose), turn, micros });
                }
                if at + 1 == warm {
                    animator.gesture(family(name));
                }
                animator.update(1.0 / RATE);
            }
            let deepest = |depth: fn(&Frame) -> &Depth| frames.iter().map(|frame| depth(frame).metres).fold(0.0, f32::max) * 100.0;
            let over = |depth: fn(&Frame) -> &Depth| frames.iter().filter(|frame| depth(frame).metres > 0.01).count();
            let (before, after) = (deepest(|frame| &frame.before), deepest(|frame| &frame.after));
            let turn = frames.iter().map(|frame| frame.turn).fold(0.0, f32::max).to_degrees();
            let mean = frames.iter().map(|frame| frame.micros).sum::<f32>() / frames.len() as f32;
            let max = frames.iter().map(|frame| frame.micros).fold(0.0, f32::max);
            table += &format!("| {id} | {name} | {} | {before:.1} | {} | {after:.1} | {} | {turn:.0} | {mean:.0} / {max:.0} |\n", frames.len(), over(|frame| &frame.before), over(|frame| &frame.after));
            let worst = frames.iter().max_by(|a, b| a.before.metres.total_cmp(&b.before.metres)).unwrap();
            let lines: Vec<String> = frames.iter().map(|frame| format!("{:.3}\t{:.4}\t{}\t{:.4}\t{}\t{:.2}", frame.time, frame.before.metres, named(&body, frame.before.pair), frame.after.metres, named(&body, frame.after.pair), frame.turn.to_degrees())).collect();
            std::fs::write(out.join(format!("{id}-{name}.tsv")), "time\tbefore_m\tbefore_pair\tafter_m\tafter_pair\tturn_deg\n".to_owned() + &lines.join("\n") + "\n").map_err(|error| error.to_string())?;
            ranked.push((before, id.clone(), figure.clone(), name.to_string(), worst.time));
            eprintln!("{id} {name}: deepest {before:.1} cm at {:.2} s → {after:.1} cm", worst.time);
        }
    }
    std::fs::write(out.join("table.md"), &table).map_err(|error| error.to_string())?;
    print!("{table}");
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    std::fs::write(out.join("ranked.tsv"), ranked.iter().map(|(depth, id, _, name, time)| format!("{depth:.2}\t{id}\t{name}\t{time:.2}\n")).collect::<String>()).map_err(|error| error.to_string())?;
    if videos > 0 {
        let gpu = Rc::new(Gpu::new(None)?);
        let mut renderer = Renderer::new(gpu, [TEXELS; 2], HEIGHT / PAGE_HEIGHT)?;
        for (_, id, figure, name, _) in ranked.iter().take(videos) {
            film(&mut renderer, figure, Path::new(motions), name, at, &out.join(format!("{id}-{name}")))?;
        }
    }
    Ok(())
}

/// Every frame of one clip, or only the one `at` seconds in, as four pictures in a row: front then three-quarter view, each as authored and separated.
fn film(renderer: &mut Renderer, figure: &str, motions: &Path, name: &str, at: Option<f32>, out: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    let model = Model::parse(&std::fs::read(figure).map_err(|error| error.to_string())?)?;
    let mut skinned = model.skinned()?;
    let body = Body::fit(&model, &skinned);
    let fit = Fit::new(&model, &skinned, HEIGHT);
    let standing = model.standing();
    let (mut animator, warm, count) = play(&model, motions, name)?;
    let mut drawn = renderer.appearance(&model, &skinned)?;
    let middle = -FLOOR;
    let mut held = Held::default();
    let views = [0.0f32, 0.6].map(|azimuth| [0.55 * azimuth.sin(), middle + 0.05, 0.55 * azimuth.cos()]);
    let only = at.map(|seconds| warm + (seconds * RATE).round() as usize);
    for at in 0..warm + count {
        let authored = model.pose(&animator.humanoid(&standing, model.rest_hips()));
        let mut separated = authored.clone();
        body.separate(&model, &mut separated, &mut held, 1.0 / RATE);
        if at + 1 == warm {
            animator.gesture(family(name));
        }
        animator.update(1.0 / RATE);
        if at < warm || only.is_some_and(|only| only != at) {
            continue;
        }
        let mut panels = Vec::new();
        for head in views {
            for pose in [&authored, &separated] {
                let changed = skinned.morph(&pose.weights);
                let worlds = model.worlds(pose);
                let placed = fit.placement(&model, &worlds, 0.0);
                let quad = facing([0.0, middle, 0.0], head);
                let local = quad.inverse();
                drawn.update(&skinned.palette(&worlds, local.mat4() * placed), &skinned.vertices, &changed);
                let eye = local.apply(head);
                renderer.render(&drawn, [eye_projection(eye.into(), QUAD / 2.0, QUAD / 2.0), None])?;
                panels.push(crate::wear::eye(&renderer.read()?, TEXELS));
            }
        }
        let mut row = image::RgbImage::new(TEXELS * panels.len() as u32, TEXELS);
        for (index, panel) in panels.iter().enumerate() {
            image::imageops::replace(&mut row, panel, index as i64 * TEXELS as i64, 0);
        }
        let path = out.join(format!("{:04}.png", at - warm));
        row.save(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

/// An animator playing `name` as the overlay does: an idle loops alone; a gesture or a mood's clip fades in over the first idle after `warm` frames and back out to it, `count` frames in all.
fn play(model: &Model, motions: &Path, name: &str) -> Result<(Animator, usize, usize), String> {
    let standing = model.standing();
    let load = |name: &str| -> Result<Clip, String> {
        let file = motions.join(format!("{name}.json"));
        let mut clip = Clip::parse(&std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?, model.version, model.rest_hips())?;
        if family(name) == IDLE {
            clip.anchor(&standing);
        }
        Ok(clip)
    };
    let clip = load(name)?;
    let seconds = clip.duration();
    if family(name) == IDLE {
        return Ok((Animator::new(HashMap::from([(name.to_owned(), clip)]), Vec::new(), Random::seeded()), 0, (seconds * RATE).ceil() as usize));
    }
    let clips = HashMap::from([(IDLE.to_owned(), load(IDLE)?), (name.to_owned(), clip)]);
    Ok((Animator::new(clips, Vec::new(), Random::seeded()), RATE as usize, ((seconds + GESTURE_TAIL) * RATE).ceil() as usize))
}

fn named(body: &Body, pair: Option<(usize, usize)>) -> String {
    pair.map_or("-".to_owned(), |(a, b)| format!("{}~{}", body.name(a), body.name(b)))
}
