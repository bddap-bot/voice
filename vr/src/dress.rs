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
const HEAD: [f32; 3] = [0.0, -FLOOR + 0.12, 0.5];

pub fn run(token: &Token, state: &Path, args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr menu <out-dir> [<catalog id>...]";
    let (out, ids) = args.split_first().ok_or(usage)?;
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
        let _ = std::fs::remove_file(pictures.join("menus.json"));
        let mut settings = Settings::read(&pictures);
        let mut puppet = load(&mut relay, avatar, &mut renderer, &settings.of(&avatar.id))?;
        let Some(menu) = puppet.model.menu.clone() else {
            println!("{}: no menu", avatar.id);
            continue;
        };
        board_pages(&menu.menu, &mut Vec::new(), &puppet, &pictures, &mut board, &mut board_image)?;
        let mut controls = Vec::new();
        flatten(&menu.menu, "", &mut controls);
        let mut kept = None;
        for (number, (path, control)) in controls.iter().enumerate() {
            puppet.controller = Some(Controller::new(&menu, &Default::default()));
            let controller = puppet.controller.as_mut().unwrap();
            if control.kind == Kind::Toggle && control.value == 0.0 {
                if let Some((_, alternative)) = controls.iter().find(|(_, candidate)| candidate.parameter == control.parameter && candidate.value != control.value) {
                    controller.set(&control.parameter, alternative.value);
                }
            }
            if matches!(control.kind, Kind::Radial | Kind::TwoAxis | Kind::FourAxis) {
                operate(controller, control, if control.kind == Kind::Radial { [0.0, 0.0] } else { [0.5, 0.5] });
            }
            let before = still(&mut puppet, &mut renderer, &mut figure, runtime)?;
            operate(puppet.controller.as_mut().unwrap(), control, if control.kind == Kind::Radial { [0.5, 0.5] } else { [1.0, 1.0] });
            let after = still(&mut puppet, &mut renderer, &mut figure, runtime)?;
            let changed = changed(&before, &after);
            let picture = pictures.join(format!("{number:03}.png"));
            pair(&before, &after, &picture)?;
            let line = serde_json::json!({"avatar": avatar.id, "control": path, "kind": format!("{:?}", control.kind), "changed": changed, "picture": picture});
            println!("{line}");
            lines.push(line.to_string());
            if kept.is_none() && changed > 0.0 && control.kind == Kind::Toggle {
                settings.keep(&avatar.id, &menu.menu, puppet.controller.as_ref().unwrap())?;
                kept = Some((path.clone(), after.clone(), puppet.controller.as_ref().unwrap().values.clone(), puppet.drawn.hidden.clone()));
            }

        }
        if let Some((path, seen, values, hidden)) = kept {
            let mut again = load(&mut relay, avatar, &mut renderer, &Settings::read(&pictures).of(&avatar.id))?;
            let worn = still(&mut again, &mut renderer, &mut figure, runtime)?;
            let differs = changed(&seen, &worn);
            let parameters: Vec<_> = values.iter().zip(&again.controller.as_ref().unwrap().values).zip(&menu.parameters).filter(|((before, after), _)| before != after).map(|((before, after), parameter)| serde_json::json!({"name": parameter.name, "before": before, "after": after})).collect();
            let line = serde_json::json!({"avatar": avatar.id, "rewear": path, "differs": differs, "parametersChanged": parameters, "hiddenBefore": hidden, "hiddenAfter": again.drawn.hidden});
            println!("{line}");
            lines.push(line.to_string());
            pair(&seen, &worn, &pictures.join("rewear.png"))?;
        }
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

fn still(puppet: &mut Puppet, renderer: &mut Renderer, overlay: &mut Overlay, runtime: &Runtime) -> Result<Vec<u8>, String> {
    let stand = Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]);
    let (menu, controller) = (puppet.model.menu.as_ref().unwrap(), puppet.controller.as_mut().unwrap());
    let mut pose = puppet.model.pose(&puppet.standing);
    let mut values = Vec::new();
    for _ in 0..SETTLE {
        values = controller.step(menu, DELTA);
    }
    puppet.drawn.hidden = menu.apply(&values, &mut pose);
    let mut retextured = Vec::new();
    for (region, value) in menu.hues(&values) {
        retextured.extend(puppet.skinned.recolor(region, value));
    }
    for (node, moved) in menu.materials(&values) {
        retextured.extend(puppet.skinned.retexture(node, moved));
    }
    let worlds = puppet.model.worlds(&pose);
    let body = stand.mat4() * puppet.fit.placement(&puppet.model, &worlds, 0.0);
    puppet.changed = puppet.skinned.morph(&pose.weights);
    puppet.changed.extend(retextured);
    let quad = facing(stand.apply([0.0, -FLOOR, 0.0]), HEAD);
    let local = quad.inverse();
    puppet.draw(&worlds, local.mat4() * body);
    let eyes = [-0.032, 0.032].map(|x| eye_projection(local.apply([HEAD[0] + x, HEAD[1], HEAD[2]]).into(), QUAD / 2.0, QUAD / 2.0));
    overlay.texture(&mut renderer.render(&puppet.drawn, eyes, renderer.eye[0])?)?;
    overlay.show();
    runtime.wait_frame();
    let stereo = renderer.read()?;
    let eye = renderer.eye[0];
    Ok((0..eye).flat_map(|y| (0..eye).map(move |x| ((y * 2 * eye + x) * 4) as usize)).flat_map(|at| stereo[at..at + 4].to_vec()).collect())
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
