use std::{collections::{BTreeMap, HashMap}, path::Path, rc::Rc};
use glam::{Mat4, Vec3};
use crate::{motion::{Clip, Animator, Random, IDLE}, vrm::{Model, Fit}, render::{Renderer, eye_projection, PAGE_HEIGHT}, vulkan::Gpu, placement::facing, FLOOR, HEIGHT, QUAD};

pub fn run(args: &[String]) -> Result<(), String> {
    let [out, motions, figure, name] = args else { return Err("usage: voice-vr playback <out> <motions> <figure.vrm> <clip>".into()) };
    let out = Path::new(out);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(figure).map_err(|e| e.to_string())?;
    let model = Model::parse(&bytes)?;
    let size = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let document: serde_json::Value = serde_json::from_slice(&bytes[20..20+size]).map_err(|e| e.to_string())?;
    let bones: BTreeMap<String, usize> = if let Some(bones) = document.pointer("/extensions/VRMC_vrm/humanoid/humanBones").and_then(|v| v.as_object()) {
        bones.iter().map(|(name, bone)| (name.clone(), bone["node"].as_u64().unwrap() as usize)).collect()
    } else {
        document.pointer("/extensions/VRM/humanoid/humanBones").and_then(|v| v.as_array()).ok_or("no humanoid")?.iter().map(|bone| (bone["bone"].as_str().unwrap().to_owned(), bone["node"].as_u64().unwrap() as usize)).collect()
    };
    let motion = std::fs::read(Path::new(motions).join(format!("{name}.json"))).map_err(|e| e.to_string())?;
    let raw = Clip::parse(&motion, model.version, model.rest_hips())?;
    let clip = Clip::parse(&motion, model.version, model.rest_hips())?;
    let mut mixed = Animator::new(HashMap::from([(IDLE.to_owned(), clip)]), Vec::new(), Random::seeded());
    let mut skinned = model.skinned()?;
    let mut gaze = crate::gaze::Gaze::default();
    let mut springs = crate::vrm::spring::Springs::default();
    let gpu = Rc::new(Gpu::new(None)?);
    let mut renderer = Renderer::new(gpu, [560; 2], HEIGHT / PAGE_HEIGHT)?;
    let mut drawn = renderer.appearance(&model, &skinned)?;
    let fit = Fit::new(&model, &skinned, HEIGHT);
    let standing = model.standing();
    let mut frames = Vec::new();
    for frame in 0..(raw.duration() * 30.0).ceil() as usize {
        let time = frame as f32 / 30.0;
        let mut stages = vec![("raw", model.pose(&raw.sample(time))), ("blend", model.pose(&mixed.humanoid(&standing, model.rest_hips())))];
        let mut pose = stages.last().unwrap().1.clone();
        stages.push(("point-inactive", pose.clone()));
        let worlds = model.worlds(&pose);
        gaze.look(&model, &mut pose, &worlds, Vec3::new(0.0, 1.5, 3.0), 1.0/30.0);
        stages.push(("gaze", pose.clone()));
        springs.step(&model, &mut pose, Mat4::IDENTITY, &[], 1.0/30.0);
        stages.push(("physics", pose));
        let mut measurements = BTreeMap::new();
        for (label, pose) in &stages {
            let worlds = model.worlds(pose);
            measurements.insert(*label, bones.iter().map(|(bone, &node)| (bone.clone(), worlds[node].to_cols_array())).collect::<BTreeMap<_,_>>());
        }
        frames.push(serde_json::json!({"time": time, "stages": measurements}));
        let head = [0.0, -FLOOR + 0.05, 0.55];
        let local = facing([0.0, -FLOOR, 0.0], head).inverse();
        let mut row = image::RgbImage::new(1120, 560);
        for (panel, pose) in [&stages[0].1, &stages.last().unwrap().1].iter().enumerate() {
            let changed = skinned.morph(&pose.weights);
            let worlds = model.worlds(pose);
            drawn.update(&skinned.palette(&worlds, local.mat4() * fit.placement(&model, &worlds, 0.0)), &skinned, &changed);
            renderer.render(&drawn, [eye_projection(local.apply(head).into(), QUAD/2.0, QUAD/2.0), None], glam::Mat4::IDENTITY, 560)?;
            image::imageops::replace(&mut row, &crate::wear::eye(&renderer.read()?, 560), panel as i64 * 560, 0);
        }
        row.save(out.join(format!("{frame:04}.png"))).map_err(|e| e.to_string())?;
        mixed.update(1.0/30.0);
    }
    std::fs::write(out.join("trace.json"), serde_json::to_vec(&frames).unwrap()).map_err(|e| e.to_string())
}
