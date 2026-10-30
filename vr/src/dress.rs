use std::path::{Path, PathBuf};

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
    overlay.texture(&mut renderer.render(&puppet.drawn, eyes, renderer.eye[0])?)?;
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
    let (menu, controller) = (puppet.model.menu.as_ref().unwrap(), puppet.controller.as_mut().unwrap());
    let mut pose = puppet.model.pose(&puppet.standing);
    let mut values = Vec::new();
    for _ in 0..settle {
        values = controller.step(menu, DELTA);
    }
    puppet.drawn.hidden = menu.apply(&values, &mut pose);
    let mut retextured = Vec::new();
    puppet.skinned.surface_values(menu, &values);
    for (node, moved) in menu.materials(&values) {
        retextured.extend(puppet.skinned.retexture(node, moved));
    }
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

/// Frames of a fingertip pressing each named radial near its start, sliding along it to its end and drawing back, off any
/// headset: the avatar in a file as the overlay draws it on the left, the board with the tip's dot on the right.
pub fn sweep(args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr sweep <out-dir> <avatar.vrm> <radial name>...";
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
        let path = find(&menu.menu, name).ok_or(format!("no radial named {name}"))?;
        let frames = PathBuf::from(out).join(name.replace('/', "-"));
        std::fs::create_dir_all(&frames).map_err(|error| format!("{}: {error}", frames.display()))?;
        let mut board = Board::new(1, 0);
        board.wear(puppet.menu(), puppet.settings());
        board.toggle_menu();
        for &at in &path[..path.len() - 1] {
            board.control(at);
        }
        let index = *path.last().unwrap();
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
            renderer.render(&puppet.drawn, eyes, renderer.eye[0])?;
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

fn find(controls: &[Control], name: &str) -> Option<Vec<usize>> {
    controls.iter().enumerate().find_map(|(at, control)| match control.kind {
        Kind::Radial if control.name == name => Some(vec![at]),
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
