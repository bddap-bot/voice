use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use glam::{Mat4, Quat, Vec3};

use crate::motion::{family, Clip, IDLE, WALK};
use crate::relay::{ClipEntry, Relay, Token};
use crate::render::{eye_projection, Renderer, PAGE_HEIGHT};
use crate::vrm::arms::{self, Clearance, Spread, RATE, TOUCH};
use crate::vrm::{Fit, Model};
use crate::vulkan::Gpu;

const TEXELS: u32 = 640;
/// Each capture's views: its turn from facing the viewer, and the square it frames in figure heights with that square's centre.
const VIEWS: [(f32, f32, f32); 3] = [(0.0, 0.5, 0.47), (0.7, 0.5, 0.47), (0.0, 1.05, 0.5)];

/// The clips an arm spread is measured against: every standing idle and the walk.
pub fn reference(name: &str) -> bool {
    matches!(family(name), IDLE | WALK)
}

/// The clips a spread is measured against, as the server serves them for `model`, or would serve them from the clip directory `staged`, in name order, with their digest.
fn clips(relay: &mut Relay, model: &Model, staged: Option<&Path>) -> Result<(Vec<(String, Clip)>, String), String> {
    let (served, digest, unread) = match staged {
        None => crate::served(relay, model)?,
        Some(dir) => {
            let entries = staged_entries(dir)?;
            crate::played_clips(&entries, |entry| if entry.format == "motion" { std::fs::read(dir.join(&entry.name)).map_err(|error| format!("{}: {error}", entry.name)) } else { relay.motion(entry) }, model)?
        }
    };
    if !unread.is_empty() {
        return Err(format!("clips a spread is measured against did not read: {}", unread.join("; ")));
    }
    let mut clips: Vec<(String, Clip)> = served.into_iter().filter(|(name, _)| reference(name)).collect();
    clips.push((WALK.to_owned(), Clip::parse(arms::WALK, model.version, model.rest_hips())?));
    clips.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((clips, digest))
}

/// The clip catalog the server's `clipCatalog` would list from directory `dir`, read by its rules: a motion clip is served as written, and any other is fetched from the server, which must already serve it.
fn staged_entries(dir: &Path) -> Result<Vec<ClipEntry>, String> {
    let manifest = dir.join("clips.manifest.json");
    let source: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).map_err(|error| format!("{}: {error}", manifest.display()))?).map_err(|error| format!("{}: {error}", manifest.display()))?;
    let mut entries = Vec::new();
    for item in source["clips"].as_array().ok_or(format!("{}: no clips", manifest.display()))? {
        let (Some(action), Some(name)) = (item["action"].as_str().filter(|action| !action.is_empty()), item["name"].as_str()) else { continue };
        let Some((_, extension)) = name.rsplit_once('.').filter(|(stem, extension)| !stem.is_empty() && !name.contains(['/', '\\']) && ["fbx", "vrma", "motion"].contains(&extension.to_ascii_lowercase().as_str())) else { continue };
        let Ok(bytes) = std::fs::read(dir.join(name)) else { continue };
        if bytes.is_empty() {
            continue;
        }
        entries.push(ClipEntry { action: action.to_owned(), name: name.to_owned(), format: extension.to_ascii_lowercase(), content_hash: arms::source_hash(&bytes) });
    }
    if entries.is_empty() {
        return Err(format!("{}: no usable clips, so the server would serve none", manifest.display()));
    }
    Ok(entries)
}

fn figure(path: &str) -> Result<(Vec<u8>, Model), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let model = Model::parse(&bytes).map_err(|error| format!("{path}: {error}"))?;
    Ok((bytes, model))
}

/// Measures a figure's arm spread against the standing idles the server plays now and the bundled walk, printing the extension that carries it.
/// usage: voice-vr arms [--clips <dir>] <figure.vrm>, with `--clips` against those the server would play from that clip directory instead.
/// usage: voice-vr arms --table <out-dir> <figure.vrm>... scores each of those clips on each figure with no spread and with its measured one, and captures each figure's deepest frame both ways.
pub fn run(token: &Token, state: &Path, args: &[String]) -> Result<(), String> {
    let mut relay = Relay::connect(token, &state.join("assets"))?;
    match args {
        [flag, out, figures @ ..] if flag == "--table" && !figures.is_empty() => table(&mut relay, Path::new(out), figures),
        [flag, dir, path] if flag == "--clips" => measure(&mut relay, path, Some(Path::new(dir))),
        [path] => measure(&mut relay, path, None),
        _ => Err("usage: voice-vr arms [--clips <dir>] <figure.vrm> | voice-vr arms --table <out-dir> <figure.vrm>...".into()),
    }
}

fn measure(relay: &mut Relay, path: &str, staged: Option<&Path>) -> Result<(), String> {
    let (bytes, model) = figure(path)?;
    let (clips, digest) = clips(relay, &model, staged)?;
    let skinned = model.skinned()?;
    let started = Instant::now();
    let needs = arms::needs(&model, &skinned, &clips.iter().map(|(_, clip)| clip).collect::<Vec<_>>())?;
    let angle = arms::spread(&needs);
    for ((name, _), need) in clips.iter().zip(&needs) {
        if need.is_none() {
            eprintln!("{path}: no spread up to the widest keeps {name} out of the thighs; it lays the hands on the body");
        }
    }
    eprintln!("{path}: spread {:.2}° measured over {} clips in {:.1} s", angle.to_degrees(), clips.len(), started.elapsed().as_secs_f64());
    let spread = Spread { angle, body: arms::body_of(&bytes)?, clips: digest, revision: arms::REVISION };
    println!("{}", spread.json());
    Ok(())
}

struct Scored {
    deepest: f32,
    at: f32,
    clipped: usize,
}

fn score(depths: &[f32]) -> Scored {
    let (frame, deepest) = depths.iter().copied().enumerate().fold((0, 0.0), |worst, (frame, depth)| if depth > worst.1 { (frame, depth) } else { worst });
    Scored { deepest, at: frame as f32 / RATE, clipped: depths.iter().filter(|&&depth| depth > TOUCH).count() }
}

fn table(relay: &mut Relay, out: &Path, figures: &[String]) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    let mut table = format!("Depth is how far forearm or hand skin sinks into thigh or hip skin, mm; a frame is clipped past {:.0} mm; 50 reads as 50 or deeper. A figure's spread is the most any clip it can clear needs, up to 25°. Sampled at {RATE} frames a second.\n\n| figure | clip | frames | clip needs ° | spread ° | before: deepest mm | before: clipped frames | after: deepest mm | after: clipped frames |\n|---|---|---|---|---|---|---|---|---|\n", TOUCH * 1e3);
    let mut worst = Vec::new();
    for path in figures {
        let id = Path::new(path).file_stem().and_then(|stem| stem.to_str()).unwrap_or(path).to_owned();
        let (_, model) = figure(path)?;
        let (clips, digest) = clips(relay, &model, None)?;
        let skinned = model.skinned()?;
        let carried = model.arms.clone();
        let started = Instant::now();
        let needs = arms::needs(&model, &skinned, &clips.iter().map(|(_, clip)| clip).collect::<Vec<_>>())?;
        let angle = arms::spread(&needs);
        let searched = started.elapsed().as_secs_f64();
        match &carried {
            Ok(spread) if spread.clips == digest && (spread.angle - angle).abs() > 1e-6 => return Err(format!("{id} carries spread {:.2}° but measures {:.2}°", spread.angle.to_degrees(), angle.to_degrees())),
            Ok(spread) => eprintln!("{id}: carries {:.2}°, measures {:.2}° in {searched:.1} s", spread.angle.to_degrees(), angle.to_degrees()),
            Err(why) => eprintln!("{id}: measures {:.2}° in {searched:.1} s; carries none: {why}", angle.to_degrees()),
        }
        let clearance = Clearance::new(&model, &skinned)?;
        let standing = model.standing();
        let mut figure_worst: Option<(f32, String, f32)> = None;
        let depths = |spread: f32| std::thread::scope(|scope| {
            let runs: Vec<_> = clips.iter().map(|(_, clip)| scope.spawn(|| score(&clearance.over(&model, &skinned, &standing, clip, spread)))).collect();
            runs.into_iter().map(|run| run.join().unwrap()).collect::<Vec<_>>()
        });
        let (befores, afters) = (depths(0.0), depths(angle));
        for ((((name, clip), before), after), need) in clips.iter().zip(befores).zip(afters).zip(&needs) {
            let frames = (clip.duration() * RATE).ceil() as usize;
            table += &format!("| {id} | {name} | {frames} | {} | {:.1} | {:.1} | {} | {:.1} | {} |\n", need.map_or("none clears".to_owned(), |need| format!("{:.1}", need.to_degrees())), angle.to_degrees(), before.deepest * 1e3, before.clipped, after.deepest * 1e3, after.clipped);
            if figure_worst.as_ref().is_none_or(|(deepest, _, _)| before.deepest > *deepest) {
                figure_worst = Some((before.deepest, name.clone(), before.at));
            }
        }
        if let Some((deepest, clip, at)) = figure_worst {
            worst.push((deepest, id, path.clone(), clips.into_iter().find(|(name, _)| *name == clip).map(|(_, clip)| clip).unwrap(), clip, at, angle));
        }
    }
    std::fs::write(out.join("table.md"), &table).map_err(|error| error.to_string())?;
    print!("{table}");
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    let gpu = Rc::new(Gpu::new(None)?);
    let mut renderer = Renderer::new(gpu, [TEXELS; 2], 1.0 / PAGE_HEIGHT)?;
    let mut ranked = String::from("deepest_mm\tfigure\tclip\tseconds\tspread_deg\tcapture\n");
    for (deepest, id, path, motion, clip, at, angle) in &worst {
        let capture = out.join(format!("{id}.png"));
        film(&mut renderer, path, motion, *at, *angle, &capture)?;
        ranked += &format!("{:.1}\t{id}\t{clip}\t{at:.2}\t{:.2}\t{}\n", deepest * 1e3, angle.to_degrees(), capture.display());
    }
    std::fs::write(out.join("worst.tsv"), ranked).map_err(|error| error.to_string())?;
    Ok(())
}

/// The figure at `at` seconds into `clip`: its hips front and three-quarter, then whole, each with no spread then with `angle`.
fn film(renderer: &mut Renderer, path: &str, clip: &Clip, at: f32, angle: f32, out: &PathBuf) -> Result<(), String> {
    let (_, model) = figure(path)?;
    let mut skinned = model.skinned()?;
    let fit = Fit::new(&model, &skinned, 1.0);
    let mut appearance = renderer.appearance(&model, &skinned)?;
    let sampled = clip.sample(at);
    let mut humanoid = model.standing();
    humanoid.rotations.extend(sampled.rotations);
    humanoid.hips = sampled.hips;
    let mut panels = Vec::new();
    for (view, window, centre) in VIEWS {
        for spread in [0.0, angle] {
            let pose = model.pose_spread(&humanoid, spread);
            let changed = skinned.morph(&pose.weights);
            let worlds = model.worlds(&pose);
            let placed = Mat4::from_translation(Vec3::new(0.0, -centre, 0.0)) * Mat4::from_quat(Quat::from_rotation_y(view)) * fit.placement(&model, &worlds, 0.0);
            appearance.update(&skinned, &worlds, placed, &changed);
            renderer.render(&appearance, [eye_projection(Vec3::new(0.0, 0.0, 3.0), window / 2.0, window / 2.0), None], Mat4::IDENTITY, renderer.eye[0])?;
            panels.push(crate::wear::eye(&renderer.read()?, TEXELS));
        }
    }
    let mut row = image::RgbImage::new(TEXELS * panels.len() as u32, TEXELS);
    for (index, panel) in panels.iter().enumerate() {
        image::imageops::replace(&mut row, panel, index as i64 * TEXELS as i64, 0);
    }
    row.save(out).map_err(|error| format!("{}: {error}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_staged_clip_directory_lists_what_the_server_would_serve_from_it_hashed_as_it_hashes() {
        let dir = std::env::temp_dir().join(format!("voice-vr-staged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in [("idle-1.motion", &b"one"[..]), ("Wave.FBX", b"wave"), ("empty.motion", b""), ("notes.txt", b"x")] {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        let manifest = serde_json::json!({"clips": [
            {"action": "idle-1", "name": "idle-1.motion"}, {"action": "wave", "name": "Wave.FBX"}, {"action": "idle-2", "name": "gone.motion"},
            {"action": "idle-3", "name": "empty.motion"}, {"action": "", "name": "idle-1.motion"}, {"action": "note", "name": "notes.txt"}, {"action": "up", "name": "../idle-1.motion"},
        ]});
        std::fs::write(dir.join("clips.manifest.json"), manifest.to_string()).unwrap();
        let entries = staged_entries(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let listed: Vec<_> = entries.iter().map(|entry| (entry.action.as_str(), entry.name.as_str(), entry.format.as_str(), entry.content_hash.as_str())).collect();
        let one = arms::source_hash(b"one");
        let wave = arms::source_hash(b"wave");
        assert_eq!(listed, [("idle-1", "idle-1.motion", "motion", one.as_str()), ("wave", "Wave.FBX", "fbx", wave.as_str())]);
    }

    #[test]
    fn a_staged_clip_directory_the_server_would_serve_nothing_from_is_refused() {
        let dir = std::env::temp_dir().join(format!("voice-vr-staged-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("clips.manifest.json"), r#"{"clips": [{"action": "idle", "name": "gone.motion"}]}"#).unwrap();
        let refused = staged_entries(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(refused, Err(why) if why.contains("no usable clips")));
    }
}
