use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::Vec3;

use crate::board::Board;
use crate::placement::{facing, Pose};
use crate::openvr::{Overlay, Runtime};
use crate::relay::{Relay, Token};
use crate::render::{eye_projection, eye_texels, Renderer, NEAREST, PAGE_HEIGHT};
use crate::settings::Settings;
use crate::vrm::expressions::{Control, Controller, Kind};
use crate::vulkan::{Flat, Gpu};
use crate::{load, operate, Puppet, FLOOR, HEIGHT, QUAD};

const SETTLE: usize = 90;
const DELTA: f32 = 0.011_111;
const MOVED: u8 = 8;

pub fn run(token: &Token, state: &Path, args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr menu <out-dir> [--eye <x> <y> <z>] [<catalog id>...]";
    let (out, mut ids) = args.split_first().ok_or(usage)?;
    let mut head = [0.0, -FLOOR + 0.12, 0.5];
    if ids.first().is_some_and(|arg| arg == "--eye") {
        for (coordinate, arg) in head.iter_mut().zip(ids.get(1..4).ok_or(usage)?) {
            *coordinate = arg.parse::<f32>().map_err(|_| usage)?;
            if !coordinate.is_finite() { return Err(usage.into()); }
        }
        if head[0] * head[0] + head[2] * head[2] < 0.0001 { return Err("the camera must be outside the avatar's vertical axis".into()); }
        ids = &ids[4..];
    }
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    let cache = state.join("assets");
    let mut relay = Relay::connect(token, &cache)?;
    let catalog = relay.catalog()?;
    let gpu = std::rc::Rc::new(Gpu::new(Some(Runtime::init()?))?);
    let runtime = gpu.vr.as_ref().unwrap();
    let mut figure = runtime.create_overlay("voice.menu-check.figure", "Avatar menu check", QUAD, true)?;
    let mut board = runtime.create_overlay("voice.menu-check.board", "Avatar menu", crate::board::WIDTH, false)?;
    for (overlay, x) in [(&figure, 0.0), (&board, -0.25)] {
        overlay.place_on(0, &Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [x, 0.0, -0.5]));
    }
    let mut board_image = Flat::new(gpu.clone());
    let mut renderer = Renderer::new(gpu.clone(), [eye_texels(QUAD, NEAREST); 2], HEIGHT / PAGE_HEIGHT)?;
    let mut lines = Vec::new();
    for avatar in catalog.avatars.iter().filter(|avatar| ids.is_empty() || ids.contains(&avatar.id)) {
        let pictures = out.join(&avatar.id);
        std::fs::create_dir_all(&pictures).map_err(|error| format!("{}: {error}", pictures.display()))?;
        let mut settings = Settings::read(&pictures);
        let configuration = settings.of(&avatar.id);
        let mut puppet = load(&mut relay, &avatar.id, &mut renderer, &configuration)?;
        let Some(menu) = puppet.model.menu.clone() else {
            println!("{}: no menu", avatar.id);
            continue;
        };
        board_pages(&menu.menu, &mut Vec::new(), &puppet, &pictures, &mut board, &mut board_image)?;
        let mut controls = Vec::new();
        flatten(&menu.menu, "", &mut controls);
        for (number, (path, control)) in controls.iter().enumerate() {
            puppet.controller = Some(Controller::new(&menu, &configuration));
            let controller = puppet.controller.as_mut().unwrap();
            if control.kind == Kind::Toggle && control.value == 0.0 {
                if let Some((_, alternative)) = controls.iter().find(|(_, candidate)| candidate.parameter == control.parameter && candidate.value != control.value) {
                    controller.set(&control.parameter, alternative.value);
                }
            }
            if matches!(control.kind, Kind::Radial | Kind::TwoAxis | Kind::FourAxis) {
                operate(controller, control, if control.kind == Kind::Radial { [0.0, 0.0] } else { [0.5, 0.5] });
            }
            let before = still(&mut puppet, &mut renderer, &mut figure, runtime, head)?;
            operate(puppet.controller.as_mut().unwrap(), control, if control.kind == Kind::Radial { [0.5, 0.5] } else { [1.0, 1.0] });
            let after = still(&mut puppet, &mut renderer, &mut figure, runtime, head)?;
            let difference = changed(&before, &after);
            let picture = pictures.join(format!("{number:03}.png"));
            pair(&before, &after, &picture)?;
            let line = serde_json::json!({"avatar": avatar.id, "control": path, "kind": format!("{:?}", control.kind), "changed": difference, "picture": picture});
            println!("{line}");
            lines.push(line.to_string());
            if matches!(control.kind, Kind::Toggle | Kind::Radial | Kind::TwoAxis | Kind::FourAxis) {
                settings.keep(&avatar.id, &menu.menu, puppet.controller.as_ref().unwrap())?;
                let values = &puppet.controller.as_ref().unwrap().values;
                let mut again = load(&mut relay, &avatar.id, &mut renderer, &Settings::read(&pictures).of(&avatar.id))?;
                let worn = still(&mut again, &mut renderer, &mut figure, runtime, head)?;
                let differs = changed(&after, &worn);
                let parameters: Vec<_> = values.iter().zip(&again.controller.as_ref().unwrap().values).zip(&menu.parameters).filter(|((before, after), _)| before != after).map(|((before, after), parameter)| serde_json::json!({"name": parameter.name, "before": before, "after": after})).collect();
                let line = serde_json::json!({"avatar": avatar.id, "rewear": path, "differs": differs, "parametersChanged": parameters, "hiddenBefore": puppet.drawn.hidden, "hiddenAfter": again.drawn.hidden});
                println!("{line}");
                lines.push(line.to_string());
                pair(&after, &worn, &pictures.join(format!("rewear-{number:03}.png")))?;
            }
        }
        let mut kept = load(&mut relay, &avatar.id, &mut renderer, &Settings::read(&pictures).of(&avatar.id))?;
        let before = still(&mut kept, &mut renderer, &mut figure, runtime, head)?;
        kept.defaults();
        settings.forget(&avatar.id)?;
        let after = still(&mut kept, &mut renderer, &mut figure, runtime, head)?;
        let worn = still(&mut load(&mut relay, &avatar.id, &mut renderer, &Settings::read(&pictures).of(&avatar.id))?, &mut renderer, &mut figure, runtime, head)?;
        let picture = pictures.join("defaults.png");
        pair(&before, &after, &picture)?;
        let line = serde_json::json!({"avatar": avatar.id, "defaults": picture, "changed": changed(&before, &after), "rewearDiffers": changed(&after, &worn)});
        println!("{line}");
        lines.push(line.to_string());
    }
    let file = out.join("menu.jsonl");
    std::fs::write(&file, lines.join("\n") + "\n").map_err(|error| format!("{}: {error}", file.display()))
}

fn flatten<'a>(controls: &'a [Control], prefix: &str, out: &mut Vec<(String, &'a Control)>) {
    for control in controls {
        let path = if prefix.is_empty() { control.name.clone() } else { format!("{prefix} / {}", control.name) };
        if control.kind == Kind::Submenu {
            flatten(&control.controls, &path, out);
        } else {
            out.push((path, control));
        }
    }
}

fn board_pages(controls: &[Control], path: &mut Vec<usize>, puppet: &Puppet, out: &Path, overlay: &mut Overlay, texture: &mut Flat) -> Result<(), String> {
    let mut board = Board::new(1, 0);
    board.wear(puppet.menu(), puppet.settings());
    board.toggle_menu();
    for &at in path.iter() {
        board.control(at);
    }
    let canvas = board.take_image().ok_or("the board drew nothing")?;
    overlay.texture(&mut texture.upload(&canvas)?)?;
    overlay.show();
    let name = if path.is_empty() { "board.png".to_owned() } else { format!("board-{}.png", path.iter().map(usize::to_string).collect::<Vec<_>>().join("-")) };
    let image = image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.pixels).ok_or("the board's pixels do not fit its size")?;
    image.save(out.join(&name)).map_err(|error| format!("{name}: {error}"))?;
    for (index, control) in controls.iter().enumerate().filter(|(_, control)| control.kind == Kind::Submenu) {
        path.push(index);
        board_pages(&control.controls, path, puppet, out, overlay, texture)?;
        path.pop();
    }
    Ok(())
}

fn still(puppet: &mut Puppet, renderer: &mut Renderer, overlay: &mut Overlay, runtime: &Runtime, head: [f32; 3]) -> Result<Vec<u8>, String> {
    let eyes = posed(puppet, head, SETTLE);
    overlay.texture(&mut renderer.render(&puppet.drawn, eyes, glam::Mat4::IDENTITY, renderer.eye[0])?)?;
    overlay.show();
    runtime.wait_frame();
    left(renderer)
}

fn left(renderer: &Renderer) -> Result<Vec<u8>, String> {
    let stereo = renderer.read()?;
    let eye = renderer.eye[0];
    Ok((0..eye).flat_map(|y| (0..eye).map(move |x| ((y * 2 * eye + x) * 4) as usize)).flat_map(|at| stereo[at..at + 4].to_vec()).collect())
}

/// Steps the puppet's menu `settle` frames and draws it standing, seen from `head`; the eyes to render it for.
fn posed(puppet: &mut Puppet, head: [f32; 3], settle: usize) -> [Option<glam::Mat4>; 2] {
    let stand = Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]);
    let mut pose = puppet.model.pose(&puppet.standing);
    let (retextured, _) = puppet.express(&mut pose, settle, DELTA);
    let worlds = puppet.model.worlds(&pose);
    let body = stand.mat4() * puppet.fit.placement(&puppet.model, &worlds, 0.0);
    puppet.changed = puppet.skinned.morph(&pose.weights);
    puppet.changed.extend(retextured);
    let quad = facing(stand.apply([0.0, -FLOOR, 0.0]), head);
    let local = quad.inverse();
    puppet.draw(&worlds, local.mat4() * body);
    let eye = local.apply(head);
    [-0.032, 0.032].map(|x| eye_projection([eye[0] + x, eye[1], eye[2]].into(), QUAD / 2.0, QUAD / 2.0))
}

/// Frames of each named control worked off any headset: the avatar in a file as the overlay draws it on the left, the board with the
/// tip's dot on the right. A fingertip presses a radial near its start, slides along it to its end and draws back; a toggle or button
/// is off, then on, then off again in real time while the figure sways side to side, so what springs on it swings or holds still.
pub fn sweep(args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr sweep <out-dir> <avatar.vrm> <radial, toggle or button name>...";
    let [out, file, names @ ..] = args else { return Err(usage.into()) };
    if names.is_empty() { return Err(usage.into()); }
    let gpu = std::rc::Rc::new(Gpu::new(None)?);
    let mut renderer = Renderer::new(gpu, [eye_texels(QUAD, NEAREST); 2], HEIGHT / PAGE_HEIGHT)?;
    let bytes = std::fs::read(file).map_err(|error| format!("{file}: {error}"))?;
    let model = crate::vrm::Model::parse(&bytes)?;
    let mut puppet = crate::dressed(model, Default::default(), file, &mut renderer, &Default::default())?;
    let menu = puppet.model.menu.clone().ok_or("the avatar has no menu")?;
    let head = [0.0, -FLOOR + 0.12, 0.5];
    for name in names {
        let path = find(&menu.menu, name).ok_or(format!("no radial, toggle or button named {name}"))?;
        let frames = PathBuf::from(out).join(name.replace('/', "-"));
        std::fs::create_dir_all(&frames).map_err(|error| format!("{}: {error}", frames.display()))?;
        let mut board = Board::new(1, 0);
        board.wear(puppet.menu(), puppet.settings());
        board.toggle_menu();
        for &at in &path[..path.len() - 1] {
            board.control(at);
        }
        let index = *path.last().unwrap();
        let control = board.control(index).ok_or("the control left the page")?;
        if control.kind != Kind::Radial {
            worked(&mut puppet, &control, &mut board, index, &mut renderer, head, &frames)?;
            println!("{name}: {}", frames.display());
            continue;
        }
        let mut tips: Vec<(f32, f32)> = (0..10).map(|step| (0.05, 0.05 - 0.005 * step as f32)).collect();
        tips.extend((0..=SWEEP).map(|step| (0.05 + 0.9 * step as f32 / SWEEP as f32, -0.01)));
        tips.extend((0..10).map(|step| (0.95, -0.01 + 0.006 * step as f32)));
        for (number, (share, depth)) in tips.into_iter().enumerate() {
            let [x, y] = board.spot(index, [share, 0.5]).ok_or("the radial left the page")?;
            let touch = board.touch(Some([x, y, depth]));
            let controller = puppet.controller.as_mut().unwrap();
            if let (Some(crate::board::Press::Control(pressed)), Some(crossing)) = (touch.press, touch.crossing) {
                if let Some(control) = board.control(pressed) {
                    operate(controller, &control, board.place_on(pressed, crossing));
                }
            }
            if let Some((control, at)) = &touch.slide {
                operate(controller, control, *at);
            }
            board.set_values(puppet.settings());
            let eyes = posed(&mut puppet, head, 2);
            renderer.render(&puppet.drawn, eyes, glam::Mat4::IDENTITY, renderer.eye[0])?;
            let figure = left(&renderer)?;
            let canvas = board.picture();
            let tip = board.pixel_of([x, y]);
            frame(&figure, &canvas, tip, depth <= 0.0, &frames.join(format!("{number:04}.png")))?;
        }
        println!("{name}: {}", frames.display());
    }
    Ok(())
}

const SWEEP: usize = 120;
const CLOCK: f32 = 1.0 / 30.0;
/// Seconds off, on and off again.
const WORKED: [f32; 3] = [1.5, 3.0, 1.5];
const SWAY: f32 = 0.05;
const SWAY_PERIOD: f32 = 1.2;

/// Steps the puppet as the overlay does, a frame each `CLOCK`, through a toggle or button turned off, on and off again.
fn worked(puppet: &mut Puppet, control: &Control, board: &mut Board, index: usize, renderer: &mut Renderer, head: [f32; 3], frames: &Path) -> Result<(), String> {
    let [x, y] = board.spot(index, [0.5, 0.5]).ok_or("the control left the page")?;
    let [off, on, _] = WORKED.map(|seconds| (seconds / CLOCK).round() as usize);
    let mut pointing = crate::point::Pointing::default();
    puppet.springs.reset();
    for number in 0..(WORKED.iter().sum::<f32>() / CLOCK).round() as usize {
        let controller = puppet.controller.as_mut().unwrap();
        let pressed = number == off || number == off + on;
        if pressed && (control.kind == Kind::Toggle || number == off) {
            operate(controller, control, [0.5, 0.5]);
        } else if pressed {
            controller.set(&control.parameter, 0.0);
        }
        board.set_values(puppet.settings());
        let sway = SWAY * (std::f32::consts::TAU * number as f32 * CLOCK / SWAY_PERIOD).sin();
        let stand = Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [sway, 0.0, 0.0]);
        let (worlds, body, _) = puppet.step(CLOCK, |_, _| stand, 1.0, head, &[], 0.0, &mut pointing, std::time::Instant::now(), |_| None);
        let local = facing(stand.apply([0.0, -FLOOR, 0.0]), head).inverse();
        puppet.draw(&worlds, local.mat4() * body);
        let eye = local.apply(head);
        let eyes = [-0.032, 0.032].map(|x| eye_projection([eye[0] + x, eye[1], eye[2]].into(), QUAD / 2.0, QUAD / 2.0));
        renderer.render(&puppet.drawn, eyes, glam::Mat4::IDENTITY, renderer.eye[0])?;
        frame(&left(renderer)?, &board.picture(), board.pixel_of([x, y]), pressed, &frames.join(format!("{number:04}.png")))?;
    }
    Ok(())
}

fn find(controls: &[Control], name: &str) -> Option<Vec<usize>> {
    controls.iter().enumerate().find_map(|(at, control)| match control.kind {
        Kind::Radial | Kind::Toggle | Kind::Button if control.name == name => Some(vec![at]),
        Kind::Submenu => find(&control.controls, name).map(|mut path| { path.insert(0, at); path }),
        _ => None,
    })
}

fn frame(figure: &[u8], board: &crate::board::Canvas, tip: [u32; 2], through: bool, path: &Path) -> Result<(), String> {
    let eye = ((figure.len() / 4) as f64).sqrt() as u32;
    let scale = (eye / board.height).max(1);
    let width = eye + board.width * scale;
    let height = eye.max(board.height * scale);
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        if x < eye {
            if y >= eye { return image::Rgb([30, 30, 30]); }
            let at = ((y * eye + x) * 4) as usize;
            let alpha = figure[at + 3] as f32 / 255.0;
            return image::Rgb([0, 1, 2].map(|channel| ((figure[at + channel] as f32 / 255.0 + 0.12 * (1.0 - alpha)).clamp(0.0, 1.0) * 255.0).round() as u8));
        }
        let (bx, by) = ((x - eye) / scale, y / scale);
        if by >= board.height { return image::Rgb([30, 30, 30]); }
        let near = bx.abs_diff(tip[0]).pow(2) + by.abs_diff(tip[1]).pow(2) <= 25;
        if near { return if through { image::Rgb([255, 214, 90]) } else { image::Rgb([240, 240, 240]) }; }
        let at = ((by * board.width + bx) * 4) as usize;
        image::Rgb([0, 1, 2].map(|channel| board.pixels[at + channel].saturating_add(30 - (30 * board.pixels[at + 3] as u32 / 255) as u8)))
    });
    image.save(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn changed(before: &[u8], after: &[u8]) -> f64 {
    let (mut covered, mut moved) = (0usize, 0usize);
    for (a, b) in before.chunks_exact(4).zip(after.chunks_exact(4)) {
        if a[3] == 0 && b[3] == 0 {
            continue;
        }
        covered += 1;
        moved += a.iter().zip(b).any(|(x, y)| x.abs_diff(*y) > MOVED) as usize;
    }
    if covered == 0 { 0.0 } else { moved as f64 / covered as f64 }
}

fn pair(before: &[u8], after: &[u8], path: &Path) -> Result<(), String> {
    let eye = ((before.len() / 4) as f64).sqrt() as u32;
    let image = image::RgbImage::from_fn(2 * eye, eye, |x, y| {
        let (picture, x) = if x < eye { (before, x) } else { (after, x - eye) };
        let at = ((y * eye + x) * 4) as usize;
        let alpha = picture[at + 3] as f32 / 255.0;
        image::Rgb([0, 1, 2].map(|channel| ((picture[at + channel] as f32 / 255.0 + 0.12 * (1.0 - alpha)).clamp(0.0, 1.0) * 255.0).round() as u8))
    });
    image.save(path).map_err(|error| format!("{}: {error}", path.display()))
}

const RECORDED_DELTA: f32 = 1.0 / 60.0;
/// The tip VRChat adds past a chain's last bone, which a converted file carries as that bone's only child.
const LEAF_TIP: &str = "VRCLeafTipBone";

/// Replays a reference recording of an avatar against the overlay's own runtime, off any headset: the root's motion, each
/// step's parameters at its start frame, the springs' tips and the recording's camera views.
pub fn reference(args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr reference <recording-dir> <avatar.vrm> <out-dir>";
    let [recording, file, out] = args else { return Err(usage.into()) };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(out.join("render")).map_err(|error| format!("{}: {error}", out.display()))?;
    let mut meta = serde_json::json!({"runtime": "voice-vr", "avatar": file, "matched": [], "unmatched": [], "notes": []});
    let result = replay_reference(Path::new(recording), file, &out, &mut meta);
    if let Err(error) = &result {
        meta["error"] = error.as_str().into();
    }
    let path = out.join("meta.json");
    std::fs::write(&path, serde_json::to_string_pretty(&meta).unwrap() + "\n").map_err(|error| format!("{}: {error}", path.display()))?;
    result
}

fn replay_reference(recording: &Path, file: &str, out: &Path, meta: &mut serde_json::Value) -> Result<(), String> {
    let table = |name: &str| -> Result<Vec<Vec<String>>, String> {
        let path = recording.join(name);
        let text = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(text.lines().filter(|line| !line.is_empty()).map(|line| line.split('\t').map(str::to_owned).collect()).collect())
    };
    let number = |field: &str| field.trim().parse::<f32>().map_err(|_| format!("not a number: {field}"));
    let frame_of = |field: &str| field.trim().parse::<i64>().map_err(|_| format!("not a frame: {field}"));
    let mut track = Vec::new();
    for row in table("track.tsv")?.iter().skip(1) {
        let values = row.get(1).ok_or("track.tsv: a row without a root")?.split(',').map(number).collect::<Result<Vec<_>, _>>()?;
        let &[x, y, z, qx, qy, qz, qw] = &values[..] else { return Err(format!("track.tsv: root {values:?} is not a position and rotation")) };
        track.push((frame_of(&row[0])?, from_unity(Vec3::new(x, y, z), glam::Quat::from_xyzw(qx, qy, qz, qw))));
    }
    let mut steps: HashMap<i64, Vec<(String, f32)>> = HashMap::new();
    let mut window = (i64::MAX, i64::MIN);
    for row in table("steps.tsv")? {
        let [_, start, frames, _, label, rest @ ..] = &row[..] else { return Err(format!("steps.tsv: short row {row:?}")) };
        let (start, frames) = (frame_of(start)?, frame_of(frames)?);
        match label.as_str() {
            "motion" => window.0 = start,
            "settle" => window.1 = start + frames,
            _ => {}
        }
        for assignment in rest.first().map_or("", String::as_str).split(',').filter(|assignment| !assignment.is_empty()) {
            let (name, value) = assignment.rsplit_once('=').ok_or(format!("steps.tsv: assignment {assignment} has no value"))?;
            steps.entry(start).or_default().push((name.to_owned(), number(value)?));
        }
    }
    let mut renders: HashMap<i64, Vec<String>> = HashMap::new();
    for row in table("renders.tsv")? {
        let [name, frame, ..] = &row[..] else { return Err(format!("renders.tsv: short row {row:?}")) };
        renders.entry(frame_of(frame)?).or_default().push(name.clone());
    }
    let camera: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(recording.join("camera.json")).map_err(|error| format!("camera.json: {error}"))?).map_err(|error| format!("camera.json: {error}"))?;
    let floats = |key: &str| -> Result<Vec<f32>, String> { camera[key].as_array().ok_or(format!("camera.json: no {key}"))?.iter().map(|value| value.as_f64().map(|value| value as f32).ok_or(format!("camera.json: {key} is not numbers"))).collect() };
    let (position, euler, background) = (floats("position")?, floats("rotation_euler")?, floats("background")?);
    let (width, height) = (camera["width"].as_u64().ok_or("camera.json: no width")? as u32, camera["height"].as_u64().ok_or("camera.json: no height")? as u32);
    let field = (camera["fov_vertical"].as_f64().ok_or("camera.json: no fov_vertical")? as f32).to_radians();
    let [ex, ey, ez] = euler[..] else { return Err("camera.json: rotation_euler is not three angles".into()) };
    let [px, py, pz] = position[..] else { return Err("camera.json: position is not three coordinates".into()) };
    let viewer = from_unity(Vec3::new(px, py, pz), glam::Quat::from_euler(glam::EulerRot::YXZ, ey.to_radians(), ex.to_radians(), ez.to_radians()));
    let (eye, ahead, up) = (viewer.transform_point3(Vec3::ZERO), viewer.transform_vector3(Vec3::Z), viewer.transform_vector3(Vec3::Y));
    let half = (field / 2.0).tan();
    let view = eye_projection(Vec3::Z, half, half).unwrap() * glam::Mat4::from_translation(Vec3::Z) * glam::Mat4::look_to_rh(eye, ahead, up);
    meta["notes"].as_array_mut().unwrap().push(format!("the overlay's own lights (a warm key, a blue rim and a sky/ground hemisphere) instead of the recording's ambient {} and white light at Unity euler {}", camera["ambient"], camera["light_euler"]).into());
    meta["notes"].as_array_mut().unwrap().push("no tracked head or hands: the figure holds the overlay's standing pose, moved by the recorded root".into());

    let bytes = std::fs::read(file).map_err(|error| format!("{file}: {error}"))?;
    let model = crate::vrm::Model::parse(&bytes)?;
    let side = height.max(width);
    let gpu = std::rc::Rc::new(Gpu::new(None)?);
    let mut renderer = Renderer::new(gpu, [side; 2], 1.0)?;
    let mut puppet = crate::dressed(model, Default::default(), file, &mut renderer, &Default::default())?;
    let names: HashMap<String, usize> = (0..puppet.model.nodes()).rev().filter_map(|node| Some((normalized(puppet.model.name(node)?), node))).collect();
    let mut tips = Vec::new();
    for row in table("chains.tsv")?.iter().skip(1) {
        let id = row[0].clone();
        let mut path = id.rsplit(['|', '/']).map(unnumbered);
        let (leaf, parent) = (path.next().unwrap_or_default(), path.next().unwrap_or_default());
        let only_child = || {
            let parent = *names.get(&normalized(parent))?;
            let mut children = (0..puppet.model.nodes()).filter(|&node| puppet.model.parent(node) == Some(parent));
            children.next().filter(|_| children.next().is_none())
        };
        let named = || names.get(&normalized(leaf)).copied();
        let tip = if leaf == LEAF_TIP { only_child().or_else(named) } else { named() };
        match tip.as_ref() {
            Some(&node) => {
                meta["matched"].as_array_mut().unwrap().push(id.as_str().into());
                tips.push((id, node));
            }
            None => meta["unmatched"].as_array_mut().unwrap().push(id.into()),
        }
    }
    let mut listing = String::new();
    if let Some(menu) = &puppet.model.menu {
        list(&menu.menu, "", &mut listing);
    }
    let path = out.join("menu.tsv");
    std::fs::write(&path, listing).map_err(|error| format!("{}: {error}", path.display()))?;
    match &puppet.model.menu {
        Some(menu) => {
            let mut unknown: Vec<&str> = steps.values().flatten().map(|(name, _)| name.as_str()).filter(|name| !menu.parameters.iter().any(|parameter| parameter.name == *name)).collect();
            unknown.sort();
            unknown.dedup();
            if !unknown.is_empty() {
                meta["notes"].as_array_mut().unwrap().push(format!("parameters the avatar's menu lacks, ignored: {}", unknown.join(", ")).into());
            }
        }
        None => meta["notes"].as_array_mut().unwrap().push("no expression menu: assignments ignored".into()),
    }

    let to_root = glam::Mat4::from_quat(puppet.model.facing());
    let mut written = String::new();
    for (frame, root) in track {
        if let Some(controller) = puppet.controller.as_mut() {
            for (name, value) in steps.get(&frame).into_iter().flatten() {
                controller.set(name, *value);
            }
        }
        let mut pose = puppet.model.pose(&puppet.standing);
        let (retextured, stilled) = puppet.express(&mut pose, 1, RECORDED_DELTA);
        puppet.settle(&mut pose, root * to_root, &[], RECORDED_DELTA, &stilled);
        let worlds = puppet.model.worlds(&pose);
        if (window.0..window.1).contains(&frame) {
            for (id, node) in &tips {
                let at = to_root.transform_point3(worlds[*node].w_axis.truncate());
                written.push_str(&format!("{frame}\t{id}\t{}\t{}\t{}\n", -at.x, at.y, at.z));
            }
        }
        let Some(names) = renders.get(&frame) else { continue };
        puppet.changed = puppet.skinned.morph(&pose.weights);
        puppet.changed.extend(retextured);
        puppet.draw(&worlds, to_root);
        renderer.render(&puppet.drawn, [Some(view), None], glam::Mat4::IDENTITY, side)?;
        let picture = left(&renderer)?;
        let (left_edge, top) = ((side - width) / 2, (side - height) / 2);
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            let at = (((y + top) * side + x + left_edge) * 4) as usize;
            let cover = picture[at + 3] as f32 / 255.0;
            image::Rgb([0, 1, 2].map(|channel| (picture[at + channel] as f32 + background[channel] * 255.0 * (1.0 - cover)).round().min(255.0) as u8))
        });
        for name in names {
            let path = out.join(name);
            image.save(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    let path = out.join("tips.tsv");
    std::fs::write(&path, written).map_err(|error| format!("{}: {error}", path.display()))
}

/// A transform in Unity's left-handed axes, as the same transform in the overlay's right-handed ones.
fn from_unity(position: Vec3, rotation: glam::Quat) -> glam::Mat4 {
    glam::Mat4::from_rotation_translation(glam::Quat::from_xyzw(rotation.x, -rotation.y, -rotation.z, rotation.w), Vec3::new(-position.x, position.y, position.z))
}

/// Each control's name path, its names joined by `/`, and kind, a submenu before what it holds.
fn list(controls: &[Control], prefix: &str, out: &mut String) {
    for control in controls {
        let path = if prefix.is_empty() { control.name.clone() } else { format!("{prefix}/{}", control.name) };
        out.push_str(&format!("{path}\t{:?}\n", control.kind));
        list(&control.controls, &path, out);
    }
}

/// A recorded path segment without the `#<n>` that tells a later sibling of the same name apart.
fn unnumbered(segment: &str) -> &str {
    segment.rsplit_once('#').filter(|(_, number)| !number.is_empty() && number.bytes().all(|digit| digit.is_ascii_digit())).map_or(segment, |(name, _)| name)
}

fn normalized(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|letter| letter.to_ascii_lowercase()).collect()
}
