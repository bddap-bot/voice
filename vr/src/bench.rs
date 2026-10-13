use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use glam::Vec3 as V3;

use crate::object::{Budget, Object, Offscreen, View, BUDGET, QUAD};
use crate::placement::{facing, Pose, Vec3};
use crate::program::Program;
use crate::render::{eye_projection, Renderer, PAGE_HEIGHT};
use crate::vrm::{Fit, Model};
use crate::vulkan::Gpu;

const FRAME: Duration = Duration::from_micros(11_111);
/// Frames in each of the three phases: untouched, the tip deep inside, the tip sliding up through it.
const PHASE: usize = 180;
const EYE: u32 = 512;
/// The wearer's eye in the puppet quad's frame, a little above the object as when it is held near the hands.
const VIEWER: Vec3 = [0.0, 0.12, 0.5];
/// The composite's window onto the puppet quad's plane, in metres: left, right, bottom, top.
const WINDOW: [f32; 4] = [-0.22, 0.47, -0.2, 0.2];
const PIXELS_PER_METRE: f32 = 1800.0;

struct Options {
    file: PathBuf,
    /// Where the pictures go, beside which puppet.
    pictures: Option<(PathBuf, PathBuf)>,
    data: Option<String>,
    at: f32,
    /// Off the headset the overlay's budget says nothing, so a render for its pictures need not hold it.
    unbudgeted: bool,
}

fn options(args: &[String]) -> Result<Options, String> {
    let usage = "usage: voice-vr object [--pictures <out-dir> <puppet.vrm>] [--data <text>] [--at <seconds>] [--unbudgeted] <file.glb>";
    let mut args = args.iter();
    let (mut file, mut pictures, mut data, mut at, mut unbudgeted) = (None, None, None, 0.0, false);
    while let Some(arg) = args.next() {
        let mut value = || args.next().cloned().ok_or(usage);
        match arg.as_str() {
            "--pictures" => pictures = Some((PathBuf::from(value()?), PathBuf::from(value()?))),
            "--data" => data = Some(value()?),
            "--at" => at = value()?.parse().map_err(|_| usage)?,
            "--unbudgeted" => unbudgeted = true,
            _ if file.is_none() => file = Some(PathBuf::from(arg)),
            _ => return Err(usage.into()),
        }
    }
    Ok(Options { file: file.ok_or(usage)?, pictures, data, at, unbudgeted })
}

/// Runs one object as the overlay does, off any headset, and reports what its budgets measure: the render thread's time per frame past the warm-up, and its program's instructions per call and memory.
/// With `--pictures` it also writes the object beside that puppet, as the wearer sees them, untouched at `--at` seconds and spread.
pub fn run(args: &[String]) -> Result<(), String> {
    let options = options(args)?;
    if let Some((out, _)) = &options.pictures {
        std::fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    }
    let bytes = std::fs::read(&options.file).map_err(|error| format!("{}: {error}", options.file.display()))?;
    program(&bytes, options.data.as_deref())?;
    let budget = if options.unbudgeted { Budget { mean: Duration::from_secs(1), frame: Duration::from_secs(10) } } else { BUDGET };
    frames(Rc::new(Gpu::new(None)?), &bytes, &options, budget)
}

fn program(bytes: &[u8], data: Option<&str>) -> Result<(), String> {
    let model = Model::object(bytes)?;
    let (vertices, triangles) = model.drawn_size();
    println!("scene: {} nodes, {vertices} vertices, {triangles} triangles drawn", model.nodes());
    let Some((wasm, duration)) = &model.program else {
        println!("program: none");
        return Ok(());
    };
    let mut program = Program::new(wasm, model.nodes())?;
    let start = program.spent();
    let mut fed = 0;
    if let Some(data) = data {
        program.feed(data.as_bytes())?;
        fed = program.spent();
    }
    let span = if *duration > 0.0 { *duration } else { 10.0 };
    let (mut most, mut total, steps) = (0, 0, 1000);
    for step in 0..steps {
        let time = span * step as f32 / steps as f32;
        let tip = (step % 2 == 1).then(|| [0.02 * (time * 3.0).sin(), 0.02 * (time * 2.0).cos(), 0.01]);
        program.tick(time, tip)?;
        most = most.max(program.spent());
        total += program.spent();
    }
    println!("program: duration {duration} s, start {start}, data {fed}, tick mean {} max {most} instructions of {}, memory {} KiB of {} KiB", total / steps, crate::program::FUEL, program.memory() / 1024, crate::program::MEMORY / 1024);
    Ok(())
}

fn frames(gpu: Rc<Gpu>, bytes: &[u8], options: &Options, budget: Budget) -> Result<(), String> {
    let viewer = Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], VIEWER);
    let eye = Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]);
    let center = crate::object_beside(1.0);
    let view = |tip: Option<Vec3>| View { center, head: viewer, eyes: [eye, eye], tip };
    let started = Instant::now();
    let mut object = Object::spawn(Offscreen, gpu.clone(), bytes.to_vec(), budget, started)?;
    if let Some(data) = &options.data {
        object.feed(data.as_bytes().to_vec());
    }
    let deadline = started + Duration::from_secs(30);
    let mut now = started;
    while object.radius().is_none() {
        if Instant::now() > deadline {
            return Err("the object never loaded".into());
        }
        object.step(now, &view(None))?;
        std::thread::sleep(FRAME);
        now = Instant::now();
    }
    let reach = object.radius().unwrap().max(0.06);
    let puppet = options.pictures.as_ref().map(|(out, file)| Ok::<_, String>((out, puppet(gpu.clone(), file)?))).transpose()?;
    let mut times = Vec::new();
    let untouched = PHASE.max((options.at / FRAME.as_secs_f32()) as usize);
    let schedule = (0..untouched).map(|_| None).chain(std::iter::once(None)).chain((0..PHASE).map(|_| Some([0.0, 0.0, 0.1 * reach]))).chain(std::iter::once(None)).chain((0..PHASE).map(|frame| {
        let rise = frame as f32 / PHASE as f32 * 1.4 - 0.7;
        Some([0.0, rise * reach, 0.6 * reach])
    }));
    for (frame, tip) in schedule.enumerate() {
        let tip = tip.map(|offset: Vec3| [center[0] + offset[0], center[1] + offset[1], center[2] + offset[2]]);
        let before = Instant::now();
        object.step(before, &view(tip)).map_err(|error| format!("frame {frame}: the overlay would remove the object: {error}"))?;
        times.push(before.elapsed());
        if let Some((out, puppet)) = &puppet {
            let name = match frame {
                frame if frame + 1 == untouched => Some("beside"),
                frame if frame == untouched + PHASE => Some("spread"),
                _ => None,
            };
            if let Some(name) = name {
                let path = out.join(format!("{name}.png"));
                composite(puppet, &object.read()?, &facing(center, VIEWER), &path)?;
                println!("wrote {}", path.display());
            }
        }
        let next = before + FRAME;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    let measured: Vec<f64> = times.iter().skip(45).map(|time| time.as_secs_f64() * 1e3).collect();
    let mean = measured.iter().sum::<f64>() / measured.len() as f64;
    let worst = measured.iter().copied().fold(0.0, f64::max);
    let first = times.iter().take(45).map(Duration::as_secs_f64).fold(0.0, f64::max) * 1e3;
    eprintln!("object gone: measured; {}", object.cost());
    println!("render thread: {} frames past the first 45, mean {mean:.2} ms, max {worst:.2} ms (budget {} ms mean, {} ms any frame); first 45 max {first:.2} ms", measured.len(), BUDGET.mean.as_millis(), BUDGET.frame.as_millis());
    Ok(())
}

/// The puppet in its standing pose on its own quad, the left eye's texels, as the host draws it.
fn puppet(gpu: Rc<Gpu>, file: &Path) -> Result<Vec<u8>, String> {
    let model = Model::parse(&std::fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?)?;
    let mut renderer = Renderer::new(gpu, [EYE, EYE], crate::HEIGHT / PAGE_HEIGHT)?;
    let mut skinned = model.skinned()?;
    let fit = Fit::new(&model, &skinned, crate::HEIGHT);
    let pose = model.pose(&model.standing());
    let changed = skinned.morph(&pose.weights);
    let worlds = model.worlds(&pose);
    let palette = skinned.palette(&worlds, fit.placement(&model, &worlds, crate::FLOOR));
    let mut appearance = renderer.appearance(&model, &skinned)?;
    appearance.update(&palette, &skinned.vertices, &changed);
    renderer.render(&appearance, [eye_projection(V3::from(VIEWER), crate::QUAD / 2.0, crate::QUAD / 2.0), None])?;
    renderer.read()
}

/// What the wearer's eye sees through a window on the puppet's plane: each quad's left-eye texels where the ray from the eye crosses it, nearer over farther, on a dark backdrop.
fn composite(puppet: &[u8], object: &[u8], object_quad: &Pose, path: &Path) -> Result<(), String> {
    let [left, right, bottom, top] = WINDOW;
    let (width, height) = (((right - left) * PIXELS_PER_METRE) as u32, ((top - bottom) * PIXELS_PER_METRE) as u32);
    let identity = Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]);
    let quads = [(identity, puppet, crate::QUAD), (*object_quad, object, QUAD)];
    let mut image = image::RgbImage::new(width, height);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let target = [left + (x as f32 + 0.5) / PIXELS_PER_METRE, top - (y as f32 + 0.5) / PIXELS_PER_METRE, 0.0];
        let shade = 0.17 - 0.08 * y as f32 / height as f32;
        let mut colour = [shade, shade, shade + 0.015];
        let mut hits: Vec<(f32, [f32; 4])> = quads.iter().filter_map(|(quad, texels, size)| sample(quad, texels, *size, target)).collect();
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, [r, g, b, a]) in hits {
            colour = [r + colour[0] * (1.0 - a), g + colour[1] * (1.0 - a), b + colour[2] * (1.0 - a)];
        }
        *pixel = image::Rgb(colour.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8));
    }
    image.save(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Where the ray from the viewer through `target` crosses the quad: its distance and premultiplied colour, nearest texel.
fn sample(quad: &Pose, texels: &[u8], size: f32, target: Vec3) -> Option<(f32, [f32; 4])> {
    let inverse = quad.inverse();
    let origin = inverse.apply(VIEWER);
    let toward = inverse.apply(target);
    let direction = [toward[0] - origin[0], toward[1] - origin[1], toward[2] - origin[2]];
    if direction[2].abs() < 1e-6 {
        return None;
    }
    let distance = -origin[2] / direction[2];
    let (u, v) = ((origin[0] + distance * direction[0]) / size + 0.5, 0.5 - (origin[1] + distance * direction[1]) / size);
    if distance <= 0.0 || !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    let at = (((v * EYE as f32) as usize * 2 * EYE as usize) + (u * EYE as f32) as usize) * 4;
    let texel: [u8; 4] = texels.get(at..at + 4)?.try_into().ok()?;
    Some((distance, texel.map(|channel| channel as f32 / 255.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(file: &Path, pictures: Option<&Path>) -> Options {
        let puppet = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/golden/figure.vrm"));
        Options { file: file.into(), pictures: pictures.map(|out| (out.into(), puppet)), data: None, at: 0.0, unbudgeted: true }
    }

    #[test]
    fn an_object_runs_through_touch_and_is_pictured_right_of_the_puppet() {
        let directory = std::env::temp_dir().join(format!("voice-vr-bench-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let loose = Budget { mean: Duration::from_secs(1), frame: Duration::from_secs(10) };
        frames(Rc::new(Gpu::new(None).unwrap()), &crate::object::tests::cube(0.2), &options(Path::new("cube.glb"), Some(&directory)), loose).unwrap();
        let picture = image::open(directory.join("beside.png")).unwrap().to_rgb8();
        let column = |x: f32| ((x - WINDOW[0]) * PIXELS_PER_METRE) as u32;
        let row = ((WINDOW[3] - crate::object_beside(1.0)[1]) * PIXELS_PER_METRE) as u32;
        let drawn = |from: f32, to: f32| (column(from)..column(to)).filter(|&x| picture.get_pixel(x, row).0.iter().any(|&channel| channel > 80)).count();
        assert!(drawn(crate::object_beside(1.0)[0] - 0.03, crate::object_beside(1.0)[0] + 0.03) > 50, "the cube shows at the object's place");
        assert!(drawn(WINDOW[0], -0.15) == 0, "nothing shows left of the puppet");
        assert!(directory.join("spread.png").exists());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn measuring_fails_where_the_overlay_would_remove_or_refuse_the_object() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let over = Budget { mean: Duration::ZERO, frame: Duration::from_secs(5) };
        let error = frames(gpu.clone(), &crate::object::tests::cube(0.1), &options(Path::new("cube.glb"), None), over).unwrap_err();
        assert!(error.contains("would remove"), "{error}");
        assert!(frames(gpu, b"not a model", &options(Path::new("junk.glb"), None), over).is_err(), "a file that is no object is refused");
        assert!(program(b"not a model", None).is_err());
    }
}
