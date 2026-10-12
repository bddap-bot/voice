use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3 as V3};

use crate::openvr::Overlay;
use crate::placement::{facing, length, sub, Pose, Vec3};
use crate::render::{eye_projection, Appearance, Renderer};
use crate::vrm::{bounds, Model, Skinned};
use crate::vulkan::Gpu;

/// The overlay quad an object is drawn on; wider than the object so its near side is not cut off.
pub const QUAD: f32 = 0.4;
const EYE: u32 = 512;
/// The largest side an object is shown at; a smaller one keeps its own size, glTF units being metres.
pub const SIZE: f32 = 0.25;
pub const TRIANGLES: usize = 200_000;
/// How long an object stays once nothing touches it.
pub const LIFETIME: Duration = Duration::from_secs(180);
/// GPU time one object may take a frame; this many frames over it in a row stop the object.
pub const BUDGET: Duration = Duration::from_millis(8);
const OVER_BUDGET: u32 = 90;
/// Tip speed relative to the object that throws it away.
pub const FLICK: f32 = 1.8;
const MIN_REACH: f32 = 0.06;
/// Below this horizontal distance from the axis the tip's bearing is noise, so it does not turn the object.
const AXIS: f32 = 0.02;

/// Where an object is drawn: a SteamVR overlay in the host, a counted fake in tests.
pub trait Surface {
    fn place_in_world(&self, pose: &Pose);
    fn texture(&self, texture: &mut openvr_sys::VRVulkanTextureData_t) -> Result<(), String>;
    fn show(&mut self);
    fn hide(&mut self);
}

impl Surface for Overlay<'_> {
    fn place_in_world(&self, pose: &Pose) {
        Overlay::place_in_world(self, pose)
    }
    fn texture(&self, texture: &mut openvr_sys::VRVulkanTextureData_t) -> Result<(), String> {
        Overlay::texture(self, texture)
    }
    fn show(&mut self) {
        Overlay::show(self)
    }
    fn hide(&mut self) {
        Overlay::hide(self)
    }
}

/// What a program publishes for the render thread to draw.
struct Geometry {
    model: Model,
    skinned: Skinned,
    fit: Mat4,
    radius: f32,
}

fn load(bytes: &[u8]) -> Result<Geometry, String> {
    let model = Model::object(bytes)?;
    let skinned = model.skinned()?;
    if skinned.indices.len() / 3 > TRIANGLES {
        return Err(format!("{} triangles, over {TRIANGLES}", skinned.indices.len() / 3));
    }
    let (low, high) = bounds(&skinned.positions(&skinned.palette(&model.worlds(&model.rest()), Mat4::IDENTITY)));
    let extent = (high - low).max_element();
    if !extent.is_finite() {
        return Err("the object has no finite extent".into());
    }
    let scale = if extent > SIZE { SIZE / extent } else { 1.0 };
    Ok(Geometry { fit: Mat4::from_scale(V3::splat(scale)) * Mat4::from_translation(-(low + high) / 2.0), radius: (high - low).length() * scale / 2.0, model, skinned })
}

/// An object's program, run on a thread of its own so nothing it does costs the render thread; the render thread only takes what it last published.
struct Program {
    published: Arc<Mutex<Option<Result<Geometry, String>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Program {
    fn start(bytes: Vec<u8>) -> Result<Program, String> {
        let published = Arc::new(Mutex::new(None));
        let slot = published.clone();
        let thread = std::thread::Builder::new().name("voice-object".into()).spawn(move || *slot.lock().unwrap() = Some(load(&bytes))).map_err(|error| format!("object thread: {error}"))?;
        Ok(Program { published, thread: Some(thread) })
    }

    fn take(&mut self) -> Option<Result<Geometry, String>> {
        self.published.lock().unwrap().take()
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Position-only handling: the tip inside the object turns it about its vertical axis by the tip's bearing, a fast tip throws it away, and untouched it leaves after `LIFETIME`.
pub struct Handling {
    pub yaw: f32,
    deadline: Instant,
    last: Option<(Vec3, Instant)>,
}

impl Handling {
    pub fn new(now: Instant) -> Handling {
        Handling { yaw: 0.0, deadline: now + LIFETIME, last: None }
    }

    /// `tip` is relative to the object's centre; a reason when the object should go.
    pub fn touch(&mut self, now: Instant, radius: f32, tip: Option<Vec3>) -> Option<&'static str> {
        if now >= self.deadline {
            return Some("timed out");
        }
        let previous = self.last.take();
        let tip = tip.filter(|&tip| length(tip) <= radius.max(MIN_REACH))?;
        self.deadline = now + LIFETIME;
        if let Some((before, at)) = previous {
            let seconds = (now - at).as_secs_f32();
            if seconds > 0.0 && length(sub(tip, before)) / seconds > FLICK {
                return Some("flicked away");
            }
            let horizontal = |v: Vec3| v[0].hypot(v[2]);
            if horizontal(before) > AXIS && horizontal(tip) > AXIS {
                let turn = tip[0].atan2(tip[2]) - before[0].atan2(before[2]);
                self.yaw += (turn + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            }
        }
        self.last = Some((tip, now));
        None
    }
}

/// Where the viewer and the right controller's tip are this frame, in the standing universe.
pub struct View {
    pub center: Vec3,
    pub head: Pose,
    pub eyes: [Pose; 2],
    pub tip: Option<Vec3>,
}

struct Drawn {
    appearance: Appearance,
    skinned: Skinned,
    worlds: Vec<Mat4>,
    fit: Mat4,
    radius: f32,
}

/// One shown object; dropping it closes its overlay, frees its GPU resources and joins its program's thread.
pub struct Object<S: Surface> {
    drawn: Option<Drawn>,
    renderer: Renderer,
    program: Program,
    surface: S,
    handling: Handling,
    over: u32,
}

impl<S: Surface> Object<S> {
    pub fn spawn(surface: S, gpu: Rc<Gpu>, bytes: Vec<u8>, now: Instant) -> Result<Object<S>, String> {
        Ok(Object { drawn: None, renderer: Renderer::new(gpu, [EYE, EYE], 1.0)?, program: Program::start(bytes)?, surface, handling: Handling::new(now), over: 0 })
    }

    pub fn hide(&mut self) {
        self.surface.hide();
    }

    /// Draws one frame; `Err` names why the object should go.
    pub fn step(&mut self, now: Instant, view: &View) -> Result<(), String> {
        if self.drawn.is_none() {
            match self.program.take() {
                Some(Ok(geometry)) => {
                    let appearance = self.renderer.appearance(&geometry.model, &geometry.skinned)?;
                    let worlds = geometry.model.worlds(&geometry.model.rest());
                    self.drawn = Some(Drawn { appearance, skinned: geometry.skinned, worlds, fit: geometry.fit, radius: geometry.radius });
                }
                Some(Err(error)) => return Err(error),
                None => {}
            }
        }
        let radius = self.drawn.as_ref().map_or(0.0, |drawn| drawn.radius);
        if let Some(reason) = self.handling.touch(now, radius, view.tip.map(|tip| sub(tip, view.center))) {
            return Err(reason.into());
        }
        let Some(drawn) = &mut self.drawn else { return Ok(()) };
        let quad = facing(view.center, view.head.t);
        let local = quad.inverse();
        let placed = local.mat4() * Mat4::from_translation(view.center.into()) * Mat4::from_rotation_y(self.handling.yaw) * drawn.fit;
        drawn.appearance.update(&drawn.skinned.palette(&drawn.worlds, placed), &drawn.skinned.vertices, &[]);
        let eyes = view.eyes.map(|eye| eye_projection(local.apply(view.head.then(&eye).t).into(), QUAD / 2.0, QUAD / 2.0));
        let started = Instant::now();
        let mut texture = self.renderer.render(&drawn.appearance, eyes)?;
        self.over = if started.elapsed() > BUDGET { self.over + 1 } else { 0 };
        if self.over >= OVER_BUDGET {
            return Err(format!("over its {} ms frame budget for {OVER_BUDGET} frames", BUDGET.as_millis()));
        }
        self.surface.place_in_world(&quad);
        self.surface.texture(&mut texture)?;
        self.surface.show();
        Ok(())
    }

    #[cfg(test)]
    fn read(&self) -> Result<Vec<u8>, String> {
        self.renderer.read()
    }
}

/// The one object shown at a time: every way an object leaves goes through `clear`.
pub struct Slot<S: Surface> {
    object: Option<Object<S>>,
}

impl<S: Surface> Default for Slot<S> {
    fn default() -> Slot<S> {
        Slot { object: None }
    }
}

impl<S: Surface> Slot<S> {
    pub fn clear(&mut self, why: &str) {
        if self.object.take().is_some() {
            eprintln!("object gone: {why}");
        }
    }

    /// Clears the shown object before spawning, so the new one can take its overlay key.
    pub fn show(&mut self, spawn: impl FnOnce() -> Result<Object<S>, String>) {
        self.clear("replaced");
        match spawn() {
            Ok(object) => {
                eprintln!("object spawned");
                self.object = Some(object);
            }
            Err(error) => eprintln!("object not shown: {error}"),
        }
    }

    pub fn hide(&mut self) {
        if let Some(object) = &mut self.object {
            object.hide();
        }
    }

    pub fn step(&mut self, now: Instant, view: &View) {
        if let Some(Err(why)) = self.object.as_mut().map(|object| object.step(now, view)) {
            self.clear(&why);
        }
    }

    #[cfg(test)]
    fn shown(&self) -> bool {
        self.object.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::tests::Builder;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SURFACES: AtomicUsize = AtomicUsize::new(0);

    struct Counted;

    impl Counted {
        fn new() -> Counted {
            SURFACES.fetch_add(1, Ordering::SeqCst);
            Counted
        }
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            SURFACES.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl Surface for Counted {
        fn place_in_world(&self, _: &Pose) {}
        fn texture(&self, _: &mut openvr_sys::VRVulkanTextureData_t) -> Result<(), String> {
            Ok(())
        }
        fn show(&mut self) {}
        fn hide(&mut self) {}
    }

    /// A cube `side` metres across with a 64-texel PNG texture, as a generic glTF exporter writes one.
    fn cube(side: f32) -> Vec<u8> {
        let mut builder = Builder::new(serde_json::json!({}));
        let h = side / 2.0;
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let mut normal = [0.0; 3];
                normal[axis] = sign;
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let base = (positions.len() / 3) as u16;
                for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let mut corner = [0.0; 3];
                    corner[axis] = sign * h;
                    corner[u] = a * h;
                    corner[v] = b * h * sign;
                    positions.extend(corner);
                    normals.extend(normal);
                    uvs.extend([(a + 1.0) / 2.0, (b + 1.0) / 2.0]);
                }
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        let position = builder.accessor("VEC3", &positions);
        let normal = builder.accessor("VEC3", &normals);
        let uv = builder.accessor("VEC2", &uvs);
        let index = builder.indices(&indices);
        let texels: Vec<u8> = (0..64 * 64).flat_map(|i| if (i % 64 / 8 + i / 64 / 8) % 2 == 0 { [230, 120, 40, 255] } else { [40, 90, 200, 255] }).collect();
        let mut png = Vec::new();
        image::RgbaImage::from_raw(64, 64, texels).unwrap().write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let view = builder.view(&png);
        builder.set("images", serde_json::json!([{ "bufferView": view, "mimeType": "image/png" }]));
        builder.set("textures", serde_json::json!([{ "source": 0 }]));
        builder.set("materials", serde_json::json!([{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }]));
        builder.set("nodes", serde_json::json!([{ "mesh": 0, "rotation": [0.3, 0.3, 0.0, 0.906] }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv }, "indices": index, "material": 0 }] }]));
        builder.glb()
    }

    fn view(tip: Option<Vec3>) -> View {
        let eye = |x| Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [x, 0.0, 0.0]);
        View { center: [0.0, 1.2, -0.5], head: Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.3, 0.0]), eyes: [eye(-0.032), eye(0.032)], tip }
    }

    /// Steps until the program has published and the object is drawn once.
    fn drawn(object: &mut Object<Counted>, now: Instant) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while object.drawn.is_none() {
            assert!(Instant::now() < deadline, "the program never published");
            object.step(now, &view(None)).unwrap();
            std::thread::yield_now();
        }
        object.step(now, &view(None)).unwrap();
    }

    #[test]
    fn a_plain_gltf_object_is_drawn_in_both_eyes_shaded_and_fit_to_its_size() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        let mut object = Object::spawn(Counted::new(), gpu, cube(3.0), now).unwrap();
        drawn(&mut object, now);
        let fit = object.drawn.as_ref().unwrap().radius;
        assert!(fit >= SIZE / 2.0 && fit <= SIZE * 3f32.sqrt() / 2.0, "a turned 3 m cube shows with its largest side at {SIZE} m: radius {fit}");
        let pixels = object.read().unwrap();
        let width = 2 * EYE as usize;
        for half in 0..2 {
            let mut covered = std::collections::HashSet::new();
            for y in 0..EYE as usize {
                for x in 0..EYE as usize {
                    let at = (y * width + half * EYE as usize + x) * 4;
                    if pixels[at + 3] > 0 {
                        covered.insert([pixels[at] / 32, pixels[at + 1] / 32, pixels[at + 2] / 32]);
                    }
                }
            }
            assert!(covered.len() >= 4, "eye {half}: textured, lit and shaded faces give several colours, got {covered:?}");
        }
        let small = Object::spawn(Counted::new(), Rc::new(Gpu::new(None).unwrap()), cube(0.05), now);
        let mut small = small.unwrap();
        drawn(&mut small, now);
        let radius = small.drawn.as_ref().unwrap().radius;
        assert!(radius >= 0.05 / 2.0 && radius <= 0.05 * 1.5, "an object smaller than {SIZE} m keeps its size: radius {radius}");
    }

    #[test]
    fn a_file_that_is_not_a_bounded_gltf_scene_is_refused_off_the_render_thread() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        for (bytes, expected) in [(b"not a model".to_vec(), "glTF"), (Builder::new(serde_json::json!({})).glb(), "no mesh")] {
            let mut slot = Slot::default();
            slot.show(|| Object::spawn(Counted::new(), gpu.clone(), bytes, now));
            let deadline = Instant::now() + Duration::from_secs(10);
            let error = loop {
                assert!(Instant::now() < deadline);
                if let Err(error) = slot.object.as_mut().unwrap().step(now, &view(None)) {
                    break error;
                }
            };
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn stirring_the_tip_inside_turns_the_object_and_a_flick_throws_it_away() {
        let start = Instant::now();
        let mut handling = Handling::new(start);
        let frame = Duration::from_millis(11);
        let mut now = start;
        for step in 0..=45 {
            let angle = step as f32 * std::f32::consts::FRAC_PI_2 / 45.0;
            assert_eq!(handling.touch(now, 0.1, Some([0.08 * angle.sin(), 0.0, 0.08 * angle.cos()])), None);
            now += frame;
        }
        assert!((handling.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "a quarter turn of the tip turns it a quarter: {}", handling.yaw);
        assert_eq!(handling.touch(now, 0.1, Some([0.5, 0.0, 0.0])), None, "outside its reach nothing happens");
        now += frame;
        assert_eq!(handling.touch(now, 0.1, Some([0.0, 0.0, 0.0])), None);
        now += frame;
        assert_eq!(handling.touch(now, 0.1, Some([0.03, 0.0, 0.0])), Some("flicked away"), "3 cm in 11 ms is past {FLICK} m/s");
    }

    #[test]
    fn an_untouched_object_times_out_and_a_touch_restarts_its_lifetime() {
        let start = Instant::now();
        let mut handling = Handling::new(start);
        assert_eq!(handling.touch(start + LIFETIME / 2, 0.1, Some([0.0, 0.05, 0.0])), None);
        assert_eq!(handling.touch(start + LIFETIME, 0.1, None), None, "the touch restarted it");
        assert_eq!(handling.touch(start + LIFETIME / 2 + LIFETIME, 0.1, None), Some("timed out"));
    }

    fn threads() -> usize {
        std::fs::read_dir("/proc/self/task").unwrap().count()
    }

    fn resident() -> usize {
        let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
        statm.split_whitespace().nth(1).unwrap().parse::<usize>().unwrap() * 4096
    }

    const CHILD: &str = "VOICE_VR_OBJECT_CLEANUP_CHILD";

    /// Runs alone in a child process so other tests' threads and memory do not blur its counts.
    #[test]
    fn every_way_an_object_leaves_returns_overlays_threads_and_memory_to_baseline() {
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["object::tests::every_way_an_object_leaves_returns_overlays_threads_and_memory_to_baseline", "--exact", "--nocapture", "--test-threads=1"])
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success(), "the cleanup run failed: {status}");
            return;
        }
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let bytes = cube(0.2);
        let mut slot: Slot<Counted> = Slot::default();
        let spawn = |slot: &mut Slot<Counted>, now: Instant| {
            slot.show(|| Object::spawn(Counted::new(), gpu.clone(), bytes.clone(), now));
            drawn(slot.object.as_mut().unwrap(), now);
        };
        let start = Instant::now();
        spawn(&mut slot, start);
        slot.clear("warm-up");
        let (surfaces, allocations, threads_before, resident_before) = (SURFACES.load(Ordering::SeqCst), gpu.allocations(), threads(), resident());
        assert_eq!(surfaces, 0);
        let mut peak = 0;
        for round in 0..120 {
            let now = start + Duration::from_secs(round);
            spawn(&mut slot, now);
            assert_eq!(SURFACES.load(Ordering::SeqCst), 1, "one overlay while shown");
            assert!(gpu.allocations() > allocations && threads() <= threads_before + 1);
            match round % 4 {
                0 => {
                    slot.step(now, &view(Some([0.0, 1.2, -0.5])));
                    slot.step(now + Duration::from_millis(11), &view(Some([0.04, 1.2, -0.5])));
                }
                1 => slot.step(now + LIFETIME, &view(None)),
                2 => spawn(&mut slot, now),
                _ => slot.clear("session ended"),
            }
            if round % 4 == 2 {
                assert_eq!(SURFACES.load(Ordering::SeqCst), 1, "a replaced object leaves exactly one overlay");
                slot.clear("asleep");
            }
            assert!(!slot.shown(), "round {round}: the object is gone");
            peak = peak.max(resident());
        }
        let growth = resident().saturating_sub(resident_before);
        eprintln!("cleanup: overlays {} gpu allocations {} -> {} threads {} -> {} resident {} -> {} MiB (peak {} MiB)", SURFACES.load(Ordering::SeqCst), allocations, gpu.allocations(), threads_before, threads(), resident_before >> 20, resident() >> 20, peak >> 20);
        assert_eq!(SURFACES.load(Ordering::SeqCst), 0, "every overlay closed");
        assert_eq!(gpu.allocations(), allocations, "every GPU allocation freed");
        assert_eq!(threads(), threads_before, "every program thread joined");
        assert!(growth < 24 << 20, "resident memory grew {} MiB over 120 objects", growth >> 20);
    }
}
