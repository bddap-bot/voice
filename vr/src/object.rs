use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use std::time::{Duration, Instant};

use glam::{Mat4, Quat, Vec3 as V3};

use crate::device;
use crate::hub::Hold;
use crate::openvr::Overlay;
use crate::placement::{facing, length, sub, Pose, Vec3};
use crate::program::{self, Transform};
use crate::render::{eye_projection, eye_texels, Appearance, Renderer};
use crate::vrm::{bounds, Model, Skinned};
use crate::vulkan::Gpu;

/// The overlay quad an object is drawn on; wider than the object so its near side is not cut off.
pub const QUAD: f32 = 0.4;
/// The largest side an object is shown at; a smaller one keeps its own size, glTF units being metres.
pub const SIZE: f32 = 0.25;
pub const TRIANGLES: usize = 200_000;
pub const VERTICES: usize = 600_000;
pub const LIFETIME: Duration = Duration::from_secs(180);
/// The render thread's time for one object, mostly the wait for its GPU work, past which it is stopped.
#[derive(Clone, Copy)]
pub struct Budget {
    /// Averaged over about `WINDOW` frames.
    pub mean: Duration,
    pub frame: Duration,
}

pub const BUDGET: Budget = Budget { mean: Duration::from_millis(4), frame: Duration::from_millis(40) };
/// No frame is ever over it: for pictures from a device slower than the headset, and for tests of anything but the budget, which pass at any speed.
pub const UNBUDGETED: Budget = Budget { mean: Duration::MAX, frame: Duration::MAX };
/// Frames before either limit applies: uploads and first draws run long on any device.
const WINDOW: f32 = 45.0;
/// Tip speed relative to the object that throws it away.
pub const FLICK: f32 = 1.8;
const MIN_REACH: f32 = 0.06;
/// The tip's depth, as a fraction of the object's radius, where its parts start to spread and where they are fully apart.
const OPENING: f32 = 0.5;
const OPEN: f32 = 0.2;
/// How quickly the spread follows the tip, per second.
const EASE: f32 = 8.0;
/// Below this horizontal distance from the axis the tip's bearing is noise, so it does not turn the object.
const AXIS: f32 = 0.02;

/// Where an object is drawn: a SteamVR overlay in the host, a counted fake in tests.
pub trait Surface {
    fn place_in_world(&self, pose: &Pose);
    fn texture(&self, texture: &mut openvr_sys::VRVulkanTextureData_t) -> Result<(), String>;
    fn part(&mut self, _: f32) {}
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
    fn part(&mut self, part: f32) {
        Overlay::part(self, part)
    }
    fn show(&mut self) {
        Overlay::show(self)
    }
    fn hide(&mut self) {
        Overlay::hide(self)
    }
}

/// The interval between program ticks, about one display frame.
const TICK: Duration = Duration::from_millis(11);

/// What the object's thread loads for the render thread to draw.
struct Geometry {
    model: Model,
    skinned: Skinned,
    fit: Mat4,
    radius: f32,
    duration: f32,
    parts: Parts,
    anchors: Vec<Option<V3>>,
}

/// The centre of each node's own and descendants' vertices, in that node's frame, for pointing at it.
fn anchors(model: &Model, skinned: &Skinned, points: &[V3], rest: &[Mat4]) -> Vec<Option<V3>> {
    let mut extents = vec![(V3::splat(f32::INFINITY), V3::splat(f32::NEG_INFINITY)); model.nodes()];
    for (vertex, point) in skinned.vertices.iter().zip(points) {
        for node in std::iter::successors(Some(skinned.joint_node(vertex.joints[0])), |&node| model.parent(node)) {
            extents[node] = (extents[node].0.min(*point), extents[node].1.max(*point));
        }
    }
    extents.iter().zip(rest).map(|((low, high), world)| (low.x <= high.x).then(|| world.inverse().transform_point3((*low + *high) / 2.0))).collect()
}

/// The pieces an object that spreads pulls apart into: its top-level nodes, or a lone root's children.
struct Parts {
    of: Vec<Option<usize>>,
    /// Each part's centre less the object's, in the object's own coordinates.
    offsets: Vec<V3>,
}

impl Parts {
    fn find(model: &Model, skinned: &Skinned, points: &[V3], centre: V3) -> Parts {
        if !model.spreads {
            return Parts { of: vec![None; model.nodes()], offsets: Vec::new() };
        }
        let roots = (0..model.nodes()).filter(|&node| model.parent(node).is_none()).count();
        let depth = (roots == 1) as usize;
        let chain = |node: usize| std::iter::successors(Some(node), |&at| model.parent(at)).collect::<Vec<_>>();
        let mut keys = Vec::new();
        let of: Vec<Option<usize>> = (0..model.nodes())
            .map(|node| {
                let chain = chain(node);
                let key = *chain.get(chain.len().checked_sub(depth + 1)?)?;
                Some(keys.iter().position(|&known| known == key).unwrap_or_else(|| {
                    keys.push(key);
                    keys.len() - 1
                }))
            })
            .collect();
        let mut extents = vec![(V3::splat(f32::INFINITY), V3::splat(f32::NEG_INFINITY)); keys.len()];
        for (vertex, point) in skinned.vertices.iter().zip(points) {
            if let Some(part) = of[skinned.joint_node(vertex.joints[0])] {
                extents[part] = (extents[part].0.min(*point), extents[part].1.max(*point));
            }
        }
        let offsets = extents.iter().map(|(low, high)| if low.x <= high.x { (*low + *high) / 2.0 - centre } else { V3::ZERO }).collect();
        Parts { of, offsets }
    }

    fn spread(&self, worlds: &[Mat4], amount: f32) -> Vec<Mat4> {
        worlds.iter().zip(&self.of).map(|(world, part)| part.map_or(*world, |part| Mat4::from_translation(self.offsets[part] * amount) * *world)).collect()
    }
}

fn load(bytes: &[u8], stop: &AtomicBool) -> Result<Geometry, String> {
    let go = || if stop.load(Ordering::SeqCst) { Err("stopped while loading".to_string()) } else { Ok(()) };
    let model = Model::object(bytes)?;
    go()?;
    let (vertices, triangles) = model.drawn_size();
    if vertices > VERTICES || triangles > TRIANGLES {
        return Err(format!("{vertices} vertices and {triangles} triangles drawn, over {VERTICES} or {TRIANGLES}"));
    }
    let skinned = model.skinned()?;
    go()?;
    let rest = model.worlds(&model.rest());
    let points = skinned.positions(&skinned.palette(&rest, Mat4::IDENTITY), &Default::default());
    go()?;
    let (low, high) = bounds(&points);
    let extent = (high - low).max_element();
    if !extent.is_finite() {
        return Err("the object has no finite extent".into());
    }
    let scale = if extent > SIZE { SIZE / extent } else { 1.0 };
    let duration = model.program.as_ref().map_or(0.0, |program| program.duration);
    let parts = Parts::find(&model, &skinned, &points, (low + high) / 2.0);
    go()?;
    let anchors = anchors(&model, &skinned, &points, &rest);
    Ok(Geometry { fit: Mat4::from_scale(V3::splat(scale)) * Mat4::from_translation(-(low + high) / 2.0), radius: (high - low).length() * scale / 2.0, model, skinned, duration, parts, anchors })
}

/// What passes between the render thread and the object's thread; each side only ever takes the other's latest.
#[derive(Default)]
struct Exchange {
    geometry: Option<Geometry>,
    transforms: Option<Vec<Transform>>,
    failed: Option<String>,
    time: f32,
    tip: Option<Vec3>,
    data: Option<Vec<u8>>,
    /// Live data reached the program since the render thread last looked.
    fed: bool,
}

#[derive(Clone, Copy)]
struct Work {
    load: fn(&[u8], &AtomicBool) -> Result<Geometry, String>,
    tick: fn(&mut program::Program, f32, Option<Vec3>) -> Result<Option<Vec<Transform>>, String>,
}

const WORK: Work = Work { load, tick: program::Program::tick };

fn run(bytes: Vec<u8>, work: Work, exchange: &Mutex<Exchange>, stop: &AtomicBool) {
    let mut geometry = match (work.load)(&bytes, stop) {
        Ok(geometry) => geometry,
        Err(error) => return exchange.lock().unwrap().failed = Some(error),
    };
    let declared = geometry.model.program.take().map(|declared| (declared, geometry.model.nodes()));
    exchange.lock().unwrap().geometry = Some(geometry);
    let Some((declared, nodes)) = declared else { return };
    if stop.load(Ordering::SeqCst) {
        return;
    }
    let mut program = match program::Program::new(&declared.wasm, nodes) {
        Ok(program) => program,
        Err(error) => return exchange.lock().unwrap().failed = Some(error),
    };
    let mut device = declared.device_feed.then(|| device::Device::new(std::path::Path::new("/"), Instant::now()));
    while !stop.load(Ordering::SeqCst) {
        let (time, tip, data) = {
            let mut exchange = exchange.lock().unwrap();
            (exchange.time, exchange.tip, exchange.data.take())
        };
        let data = data.or_else(|| device.as_mut()?.sample(Instant::now()));
        if let Some(data) = data {
            if let Err(error) = program.feed(&data) {
                return exchange.lock().unwrap().failed = Some(error);
            }
            exchange.lock().unwrap().fed = true;
        }
        match (work.tick)(&mut program, time, tip) {
            Ok(Some(transforms)) => exchange.lock().unwrap().transforms = Some(transforms),
            Ok(None) => {}
            Err(error) => return exchange.lock().unwrap().failed = Some(error),
        }
        std::thread::park_timeout(TICK);
    }
}

/// An object's thread: it loads the file, then ticks its program, so nothing the object does costs the render thread.
/// Dropping it never joins: the thread owns all it uses and ends at its next check of `stop`, so a load in progress cannot stall a frame.
struct Runner {
    exchange: Arc<Mutex<Exchange>>,
    stop: Arc<AtomicBool>,
    thread: Thread,
}

impl Runner {
    fn start(bytes: Vec<u8>, work: Work) -> Result<Runner, String> {
        let exchange = Arc::new(Mutex::new(Exchange::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared, stopped) = (exchange.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("voice-object".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(bytes, work, &shared, &stopped))).is_err() {
                    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).failed = Some("the object's thread panicked".into());
                    shared.clear_poison();
                }
            })
            .map_err(|error| format!("object thread: {error}"))?;
        Ok(Runner { exchange, stop, thread: thread.thread().clone() })
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.unpark();
    }
}

/// Position-only handling: the tip inside the object turns it about its vertical axis by the tip's bearing, a fast tip throws it away, and neither touched nor fed live data it leaves after `LIFETIME`.
/// Pushing the tip deep into an object that spreads spreads its parts until the tip withdraws, and sliding the tip up or down inside it scrubs its timeline, which holds still while touched.
pub struct Handling {
    pub yaw: f32,
    /// How far apart its parts are, 0 to 1.
    pub spread: f32,
    /// Vertical travel of the tip while touching, in metres.
    pub slide: f32,
    /// Seconds the timeline has run untouched.
    pub clock: f32,
    /// Whether the hub holds the timeline still, so only the tip moves it.
    held: bool,
    /// Where the tip is inside the object, relative to its centre.
    pub touching: Option<Vec3>,
    deadline: Instant,
    /// The tip relative to the centre and in the room, when last inside.
    last: Option<(Vec3, Vec3, Instant)>,
    stepped: Instant,
}

impl Handling {
    pub fn new(now: Instant) -> Handling {
        Handling { yaw: 0.0, spread: 0.0, slide: 0.0, clock: 0.0, held: false, touching: None, deadline: now + LIFETIME, last: None, stepped: now }
    }

    pub fn renew(&mut self, now: Instant) {
        self.deadline = now + LIFETIME;
    }

    /// `reach` is the object's radius, at least `MIN_REACH`; `tip` is the tip relative to the object's centre and in the room. A reason when the object should go.
    pub fn touch(&mut self, now: Instant, reach: f32, tip: Option<(Vec3, Vec3)>) -> Option<&'static str> {
        if now >= self.deadline {
            return Some("timed out");
        }
        let delta = now.saturating_duration_since(self.stepped).as_secs_f32();
        self.stepped = now;
        let previous = self.last.take();
        let inside = tip.filter(|&(tip, _)| length(tip) <= reach);
        self.touching = inside.map(|(tip, _)| tip);
        let depth = self.touching.map_or(1.0, |tip| length(tip) / reach);
        let target = ((OPENING - depth) / (OPENING - OPEN)).clamp(0.0, 1.0);
        self.spread += (target - self.spread) * (delta * EASE).min(1.0);
        let Some((tip, room)) = inside else {
            if !self.held {
                self.clock += delta;
            }
            return None;
        };
        self.deadline = now + LIFETIME;
        if let Some((before, room_before, at)) = previous {
            let seconds = (now - at).as_secs_f32();
            if seconds > 0.0 && length(sub(room, room_before)) / seconds > FLICK {
                return Some("flicked away");
            }
            self.slide += tip[1] - before[1];
            let horizontal = |v: Vec3| v[0].hypot(v[2]);
            if horizontal(before) > AXIS && horizontal(tip) > AXIS {
                let turn = tip[0].atan2(tip[2]) - before[0].atan2(before[2]);
                self.yaw += (turn + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            }
        }
        self.last = Some((tip, room, now));
        None
    }
}

/// The space between neighbouring places.
const GAP: f32 = 0.05;

/// Where the centre of the object at `place` sits in the frame of a puppet `size` times hand size: right of the figure as it faces you, about chest high, place 0 nearest.
pub fn beside(size: f32, place: usize) -> Vec3 {
    [SIZE / 2.0 + 0.1 * size + place as f32 * (SIZE + GAP), -crate::FLOOR * size, 0.0]
}

/// Where the puppet stands and the viewer and the right controller's tip are this frame, in the standing universe.
pub struct View {
    pub stand: Pose,
    pub size: f32,
    pub head: Pose,
    pub eyes: [Pose; 2],
    pub tip: Option<Vec3>,
}

impl View {
    /// Fixed to the puppet's frame alone, so walking around a set-down puppet shows the object's other sides.
    pub fn center(&self, place: usize) -> Vec3 {
        self.stand.apply(beside(self.size, place))
    }
}

struct Drawn {
    appearance: Appearance,
    model: Model,
    skinned: Skinned,
    worlds: Vec<Mat4>,
    fit: Mat4,
    radius: f32,
    duration: f32,
    parts: Parts,
    anchors: Vec<Option<V3>>,
    /// The object's own coordinates to the standing universe, as last drawn.
    placed: Mat4,
}

fn transformed(model: &Model, transforms: &[Transform]) -> Vec<Mat4> {
    let mut pose = model.rest();
    for (node, transform) in transforms.iter().enumerate() {
        let rotation = glam::Vec4::from_slice(&transform[3..7]).try_normalize().map_or(Quat::IDENTITY, Quat::from_vec4);
        pose.place(node, V3::from_slice(&transform[..3]), rotation, V3::from_slice(&transform[7..]));
    }
    model.worlds(&pose)
}

/// One shown object; dropping it closes its overlay, frees its GPU resources and stops its program's thread.
pub struct Object<S: Surface> {
    /// First, so the overlay closes before the textures it shows are freed.
    surface: S,
    /// Which place beside the puppet it stands at.
    place: usize,
    drawn: Option<Drawn>,
    renderer: Renderer,
    runner: Runner,
    handling: Handling,
    /// Live data waiting for the next frame, so it reaches the program with the time that frame sets, a hold's included.
    data: Option<Vec<u8>>,
    budget: Budget,
    /// Seconds the render thread spends on it per frame, a running mean.
    cost: f32,
    /// Its slowest frame past the warm-up, in seconds.
    worst: f32,
    frames: u32,
}

impl<S: Surface> Object<S> {
    pub fn spawn(surface: S, gpu: Rc<Gpu>, bytes: Vec<u8>, budget: Budget, now: Instant) -> Result<Object<S>, String> {
        Object::loading(surface, gpu, bytes, WORK, budget, now)
    }

    fn loading(surface: S, gpu: Rc<Gpu>, bytes: Vec<u8>, work: Work, budget: Budget, now: Instant) -> Result<Object<S>, String> {
        Ok(Object { place: 0, drawn: None, renderer: Renderer::new(gpu, [eye_texels(QUAD, crate::render::NEAREST); 2], 1.0)?, runner: Runner::start(bytes, work)?, surface, handling: Handling::new(now), data: None, budget, cost: 0.0, worst: 0.0, frames: 0 })
    }

    pub fn hide(&mut self) {
        self.surface.hide();
    }

    /// Live data for the program; only the newest waiting update is kept.
    pub fn feed(&mut self, data: Vec<u8>) {
        self.data = Some(data);
    }

    pub fn hold(&mut self, hold: Hold) {
        let handling = &mut self.handling;
        handling.held = hold != Hold::Run;
        if let Hold::At(seconds) = hold {
            handling.clock = seconds;
            handling.slide = 0.0;
        }
    }

    fn has(&self, name: &str) -> bool {
        self.drawn.as_ref().is_some_and(|drawn| drawn.model.named(name).is_some())
    }

    /// Where the node named `name` is now, in the standing universe.
    pub fn locate(&self, name: &str) -> Option<Vec3> {
        let drawn = self.drawn.as_ref()?;
        let node = drawn.model.named(name)?;
        let worlds = drawn.parts.spread(&drawn.worlds, self.handling.spread);
        Some((drawn.placed * worlds[node]).transform_point3(drawn.anchors[node]?).into())
    }

    /// Draws one frame; `Err` names why the object should go.
    pub fn step(&mut self, now: Instant, view: &View) -> Result<(), String> {
        let (geometry, transforms, fed) = {
            let mut exchange = self.runner.exchange.lock().unwrap();
            if let Some(failed) = exchange.failed.take() {
                return Err(failed);
            }
            (exchange.geometry.take(), exchange.transforms.take(), std::mem::take(&mut exchange.fed))
        };
        if fed {
            self.handling.renew(now);
        }
        if let Some(geometry) = geometry {
            let appearance = self.renderer.appearance(&geometry.model, &geometry.skinned)?;
            let worlds = geometry.model.worlds(&geometry.model.rest());
            self.drawn = Some(Drawn { appearance, model: geometry.model, skinned: geometry.skinned, worlds, fit: geometry.fit, radius: geometry.radius, duration: geometry.duration, parts: geometry.parts, anchors: geometry.anchors, placed: Mat4::IDENTITY });
        }
        let reach = self.drawn.as_ref().map_or(0.0, |drawn| drawn.radius).max(MIN_REACH);
        let center = view.center(self.place);
        if let Some(reason) = self.handling.touch(now, reach, view.tip.map(|tip| (sub(tip, center), tip))) {
            return Err(reason.into());
        }
        let Some(drawn) = &mut self.drawn else { return Ok(()) };
        if let Some(transforms) = transforms {
            drawn.worlds = transformed(&drawn.model, &transforms);
        }
        let turned = Mat4::from_rotation_y(self.handling.yaw) * drawn.fit;
        {
            let mut exchange = self.runner.exchange.lock().unwrap();
            let handling = &self.handling;
            exchange.time = handling.clock + handling.slide / (2.0 * reach) * drawn.duration;
            exchange.tip = handling.touching.map(|offset| turned.inverse().transform_point3(offset.into()).into());
            if let Some(data) = self.data.take() {
                exchange.data = Some(data);
                self.runner.thread.unpark();
            }
        }
        let quad = facing(center, view.head.t);
        let local = quad.inverse();
        drawn.placed = Mat4::from_translation(center.into()) * turned;
        drawn.appearance.update(&drawn.skinned.palette(&drawn.parts.spread(&drawn.worlds, self.handling.spread), local.mat4() * drawn.placed), &drawn.skinned.vertices, &[]);
        let seen = view.eyes.map(|eye| local.apply(view.head.then(&eye).t));
        let texels = self.renderer.texels(QUAD, seen[0][2].min(seen[1][2]));
        self.surface.part(texels as f32 / self.renderer.eye[0] as f32);
        let started = Instant::now();
        let texture = self.renderer.render(&drawn.appearance, seen.map(|eye| eye_projection(eye.into(), QUAD / 2.0, QUAD / 2.0)), texels)?;
        let spent = started.elapsed();
        self.frames += 1;
        let measured = self.frames as f32 - WINDOW;
        if measured <= 0.0 {
            return self.show(texture, &quad);
        }
        self.cost += (spent.as_secs_f32() - self.cost) / measured.min(WINDOW);
        self.worst = self.worst.max(spent.as_secs_f32());
        if spent > self.budget.frame {
            return Err(format!("one frame took {} ms, over {} ms", spent.as_millis(), self.budget.frame.as_millis()));
        }
        if measured >= WINDOW && self.cost > self.budget.mean.as_secs_f32() {
            return Err(format!("{:.1} ms a frame, over its {} ms budget", self.cost * 1e3, self.budget.mean.as_millis()));
        }
        self.show(texture, &quad)
    }

    fn show(&mut self, mut texture: openvr_sys::VRVulkanTextureData_t, quad: &Pose) -> Result<(), String> {
        self.surface.place_in_world(quad);
        self.surface.texture(&mut texture)?;
        self.surface.show();
        Ok(())
    }

    /// Its render cost as the budget sees it, for the log.
    pub fn cost(&self) -> String {
        match self.frames as f32 - WINDOW {
            measured if measured > 0.0 => format!("running mean {:.2} ms a frame, worst {:.2} ms over {measured} frames, budget {} ms mean and {} ms a frame", self.cost * 1e3, self.worst * 1e3, self.budget.mean.as_millis(), self.budget.frame.as_millis()),
            _ => format!("{} frames drawn, too few to measure", self.frames),
        }
    }

    /// The last frame's texels, both eyes side by side.
    pub fn read(&self) -> Result<Vec<u8>, String> {
        self.renderer.read()
    }

    /// The bounding sphere's radius as shown, once loaded.
    pub fn radius(&self) -> Option<f32> {
        self.drawn.as_ref().map(|drawn| drawn.radius)
    }
}

/// The most objects shown at once. Each object's render waits on its own GPU work on the render thread: on the headset the puppet's frame measured 3.5 ms and a shown object 2.85 ms a frame, so three keep the overlay's update at or above 72 Hz, half the display's 144 Hz, and a fourth would not.
pub const MOST: usize = 3;

/// The objects shown, each at its own place beside the puppet until it leaves; every way one leaves goes through `remove` or `clear`.
pub struct Shelf<S: Surface> {
    /// Oldest first.
    objects: Vec<Object<S>>,
}

impl<S: Surface> Default for Shelf<S> {
    fn default() -> Shelf<S> {
        Shelf { objects: Vec::new() }
    }
}

impl<S: Surface> Shelf<S> {
    fn remove(&mut self, index: usize, why: &str) {
        let object = self.objects.remove(index);
        tracing::info!(place = object.place + 1, "object gone: {why}; {}", object.cost());
    }

    pub fn clear(&mut self, why: &str) {
        while !self.objects.is_empty() {
            self.remove(0, why);
        }
    }

    /// Spawns at the nearest free place, given to `spawn` so its overlay key is that place's; with every place taken the oldest goes first.
    pub fn show(&mut self, spawn: impl FnOnce(usize) -> Result<Object<S>, String>) {
        if self.objects.len() >= MOST {
            self.remove(0, "replaced");
        }
        let place = (0..).find(|place| self.objects.iter().all(|object| object.place != *place)).unwrap();
        match spawn(place) {
            Ok(mut object) => {
                tracing::info!(place = place + 1, "object spawned");
                object.place = place;
                self.objects.push(object);
            }
            Err(error) => tracing::warn!("object not shown: {error}"),
        }
    }

    pub fn hide(&mut self) {
        for object in &mut self.objects {
            object.hide();
        }
    }

    /// The object a pointed name means and the node's name in it: `<place>:<node>` names the object at that place, counted from the puppet outward from 1; a bare name means the newest object that has it.
    fn named<'a>(&self, name: &'a str) -> Option<(usize, &'a str)> {
        let placed = name.split_once(':').and_then(|(place, node)| {
            let place = place.parse::<usize>().ok()?;
            Some((self.objects.iter().position(|object| object.place + 1 == place)?, node))
        });
        placed.or_else(|| Some((self.objects.iter().rposition(|object| object.has(name))?, name)))
    }

    pub fn locate(&self, name: &str) -> Option<Vec3> {
        let (index, node) = self.named(name)?;
        self.objects[index].locate(node)
    }

    /// What a reply's live data or hold reaches: the object its pointing names, else the newest.
    fn addressed(&mut self, point: Option<&str>) -> Option<&mut Object<S>> {
        let index = point.and_then(|name| self.named(name)).map_or(self.objects.len().checked_sub(1)?, |(index, _)| index);
        self.objects.get_mut(index)
    }

    pub fn feed(&mut self, data: Vec<u8>, point: Option<&str>) {
        match self.addressed(point) {
            Some(object) => object.feed(data),
            None => tracing::info!("live data with no object shown"),
        }
    }

    pub fn hold(&mut self, hold: Hold, point: Option<&str>) {
        match self.addressed(point) {
            Some(object) => object.hold(hold),
            None => tracing::info!("a hold with no object shown"),
        }
    }

    pub fn step(&mut self, now: Instant, view: &View) {
        let mut index = 0;
        while index < self.objects.len() {
            match self.objects[index].step(now, view) {
                Ok(()) => index += 1,
                Err(why) => self.remove(index, &why),
            }
        }
    }

    #[cfg(test)]
    fn shown(&self) -> bool {
        !self.objects.is_empty()
    }
}

/// A surface that shows nothing, for drawing an object offscreen.
pub struct Offscreen;

impl Surface for Offscreen {
    fn place_in_world(&self, _: &Pose) {}
    fn texture(&self, _: &mut openvr_sys::VRVulkanTextureData_t) -> Result<(), String> {
        Ok(())
    }
    fn show(&mut self) {}
    fn hide(&mut self) {}
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::vrm::tests::Builder;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SURFACES: AtomicUsize = AtomicUsize::new(0);

    /// Long past any healthy wait on any machine: only a deadlock or a lost signal reaches it.
    const HANG: Duration = Duration::from_secs(60);

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
    pub fn cube(side: f32) -> Vec<u8> {
        scene(side, None)
    }

    /// The cube as node 0, run by `program` when given; node 1 is an empty the program may move.
    fn scene(side: f32, program: Option<(&str, f32)>) -> Vec<u8> {
        built(side, serde_json::json!([{ "mesh": 0, "rotation": [0.3, 0.3, 0.0, 0.906] }, {}]), program, false)
    }

    fn built(side: f32, nodes: serde_json::Value, program: Option<(&str, f32)>, spreads: bool) -> Vec<u8> {
        built_with(side, nodes, program, spreads, serde_json::json!({}))
    }

    /// `declared` adds to the program's extension object.
    fn built_with(side: f32, nodes: serde_json::Value, program: Option<(&str, f32)>, spreads: bool, declared: serde_json::Value) -> Vec<u8> {
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
        builder.set("materials", serde_json::json!([{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }, { "pbrMetallicRoughness": { "baseColorFactor": [0.2, 0.4, 0.8, 1.0] } }]));
        builder.set("nodes", nodes);
        let mut extensions = serde_json::json!({});
        if let Some((text, duration)) = program {
            let view = builder.view(&crate::program::tests::wasm(text));
            let mut extension = serde_json::json!({ "bufferView": view, "duration": duration });
            extension.as_object_mut().unwrap().extend(declared.as_object().unwrap().clone());
            extensions[crate::vrm::PROGRAM] = extension;
        }
        if spreads {
            extensions[crate::vrm::SPREAD] = serde_json::json!({});
        }
        builder.set("extensions", extensions);
        let mesh = |material: usize| serde_json::json!({ "primitives": [{ "attributes": { "POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv }, "indices": index, "material": material }] });
        builder.set("meshes", serde_json::json!([mesh(0), mesh(1)]));
        builder.glb()
    }

    fn view(tip: Option<Vec3>) -> View {
        let eye = |x| Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [x, 0.0, 0.0]);
        let stand = Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], sub([0.0, 1.2, -0.5], beside(1.0, 0)));
        View { stand, size: 1.0, head: Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.3, 0.0]), eyes: [eye(-0.032), eye(0.032)], tip }
    }

    /// Steps until the program has published and the object is drawn once.
    fn drawn(object: &mut Object<Counted>, now: Instant) {
        let deadline = Instant::now() + HANG;
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
        let mut object = Object::spawn(Counted::new(), gpu, cube(3.0), UNBUDGETED, now).unwrap();
        drawn(&mut object, now);
        let fit = object.drawn.as_ref().unwrap().radius;
        assert!(fit >= SIZE / 2.0 && fit <= SIZE * 3f32.sqrt() / 2.0, "a turned 3 m cube shows with its largest side at {SIZE} m: radius {fit}");
        let pixels = object.read().unwrap();
        let eye = ((pixels.len() / 8) as f64).sqrt() as usize;
        for half in 0..2 {
            let mut covered = std::collections::HashSet::new();
            for y in 0..eye {
                for x in 0..eye {
                    let at = (y * 2 * eye + half * eye + x) * 4;
                    if pixels[at + 3] > 0 {
                        covered.insert([pixels[at] / 32, pixels[at + 1] / 32, pixels[at + 2] / 32]);
                    }
                }
            }
            assert!(covered.len() >= 4, "eye {half}: textured, lit and shaded faces give several colours, got {covered:?}");
        }
        let small = Object::spawn(Counted::new(), Rc::new(Gpu::new(None).unwrap()), cube(0.05), UNBUDGETED, now);
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
            let mut shelf = Shelf::default();
            shelf.show(|_| Object::spawn(Counted::new(), gpu.clone(), bytes, UNBUDGETED, now));
            let deadline = Instant::now() + HANG;
            let error = loop {
                assert!(Instant::now() < deadline);
                if let Err(error) = shelf.objects.last_mut().unwrap().step(now, &view(None)) {
                    break error;
                }
            };
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn a_program_moves_the_object_from_its_own_thread_by_time_and_touch() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let born = Instant::now();
        let mut object = Object::spawn(Counted::new(), gpu, scene(0.2, Some((crate::program::tests::FOLLOW, 4.0))), UNBUDGETED, born).unwrap();
        drawn(&mut object, born);
        object.step(born + Duration::from_secs(5), &view(None)).unwrap();
        let deadline = Instant::now() + HANG;
        let center = view(None).center(0);
        let mut moved = Vec::new();
        while moved.len() < 2 {
            assert!(Instant::now() < deadline, "the program never published");
            object.step(born + Duration::from_secs(5), &view(Some([center[0], center[1] + 0.05, center[2]]))).unwrap();
            let drawn = object.drawn.as_ref().unwrap();
            moved = [0, 1].map(|node| drawn.worlds[node].w_axis.truncate()).into_iter().filter(|at| at.length() > 1e-6).collect();
            std::thread::sleep(TICK);
        }
        assert!((moved[0].x - 5.0).abs() < 1e-5, "time runs on past the 4 s timeline and holds while touched: node 0 at {}", moved[0]);
        assert!(moved[1].y > 0.0 && moved[1].x.abs() < 1e-4, "the touch above the centre reaches node 1 in the object's coordinates: {}", moved[1]);
    }

    #[test]
    fn sliding_the_tip_up_scrubs_the_timeline_and_pushing_it_deep_spreads_the_parts() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let born = Instant::now();
        let nodes = serde_json::json!([{ "mesh": 0, "translation": [-0.06, 0.0, 0.0] }, { "mesh": 0, "translation": [0.06, 0.0, 0.0] }]);
        let mut object = Object::spawn(Counted::new(), gpu, built(0.05, nodes, None, true), UNBUDGETED, born).unwrap();
        drawn(&mut object, born);
        let radius = object.drawn.as_ref().unwrap().radius;
        let center = view(None).center(0);
        let at = |dy: f32| view(Some([center[0] + 0.3 * radius, center[1] + dy, center[2]]));
        let frame = Duration::from_millis(11);
        let mut now = born;
        for step in 0..=20 {
            object.step(now, &at(step as f32 * radius / 40.0)).unwrap();
            now += frame;
        }
        assert!((object.handling.slide - radius / 2.0).abs() < 1e-4, "slid {} for half a radius of {radius}", object.handling.slide);
        assert_eq!(object.handling.clock, 0.0, "the timeline holds while touched");
        object.step(now, &view(None)).unwrap();
        now += frame;
        for _ in 0..60 {
            object.step(now, &view(Some(center))).unwrap();
            now += frame;
        }
        let parts = |object: &Object<Counted>| {
            let drawn = object.drawn.as_ref().unwrap();
            drawn.parts.spread(&drawn.worlds, object.handling.spread).iter().map(|world| world.w_axis.x).collect::<Vec<_>>()
        };
        let apart = parts(&object);
        assert!(object.handling.spread > 0.99 && (apart[1] - apart[0] - 0.24).abs() < 1e-3, "the two cubes 12 cm apart spread to 24 cm: {apart:?}");
        for _ in 0..60 {
            object.step(now, &view(None)).unwrap();
            now += frame;
        }
        let closed = parts(&object);
        assert!((closed[1] - closed[0] - 0.12).abs() < 1e-3, "withdrawn, they close: {closed:?}");
    }

    #[test]
    fn a_hold_stills_the_timeline_at_its_time_the_tip_still_scrubs_it_and_run_lets_it_go_on() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let born = Instant::now();
        let mut shelf = Shelf::default();
        shelf.show(|_| Object::spawn(Counted::new(), gpu, scene(0.2, Some((crate::program::tests::FOLLOW, 4.0))), UNBUDGETED, born));
        drawn(shelf.objects.last_mut().unwrap(), born);
        let frame = Duration::from_millis(11);
        let mut now = born;
        for _ in 0..30 {
            shelf.step(now, &view(None));
            now += frame;
        }
        shelf.hold(Hold::At(1.5), None);
        let clock = |shelf: &Shelf<Counted>| shelf.objects.last().unwrap().handling.clock;
        for _ in 0..90 {
            shelf.step(now, &view(None));
            now += frame;
        }
        assert_eq!(clock(&shelf), 1.5, "a second untouched leaves a held timeline where the hub put it");
        let deadline = Instant::now() + HANG;
        loop {
            assert!(Instant::now() < deadline, "the program never saw the held time");
            shelf.step(now, &view(None));
            if shelf.objects.last().unwrap().drawn.as_ref().is_some_and(|drawn| (drawn.worlds[0].w_axis.x - 1.5).abs() < 1e-5) {
                break;
            }
            std::thread::sleep(TICK);
        }
        let center = view(None).center(0);
        let radius = shelf.objects.last().unwrap().drawn.as_ref().unwrap().radius;
        for step in 0..=10 {
            shelf.step(now, &view(Some([center[0] + 0.3 * radius, center[1] + step as f32 * radius / 20.0, center[2]])));
            now += frame;
        }
        shelf.step(now, &view(None));
        now += frame;
        assert!(shelf.objects.last().unwrap().handling.slide > 0.0, "the tip still scrubs a held timeline");
        shelf.hold(Hold::Run, None);
        for _ in 0..10 {
            shelf.step(now, &view(None));
            now += frame;
        }
        assert!((clock(&shelf) - 1.5 - 10.0 * frame.as_secs_f32()).abs() < 1e-4, "run lets it go on from where it was held: {}", clock(&shelf));
    }

    #[test]
    fn a_file_that_does_not_spread_keeps_its_parts_in_place_with_the_tip_deep() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let born = Instant::now();
        let nodes = serde_json::json!([{ "mesh": 0, "translation": [-0.06, 0.0, 0.0] }, { "mesh": 0, "translation": [0.06, 0.0, 0.0] }]);
        let mut object = Object::spawn(Counted::new(), gpu, built(0.05, nodes, None, false), UNBUDGETED, born).unwrap();
        drawn(&mut object, born);
        let mut now = born;
        for _ in 0..60 {
            object.step(now, &view(Some(view(None).center(0)))).unwrap();
            now += Duration::from_millis(11);
        }
        let drawn = object.drawn.as_ref().unwrap();
        let apart: Vec<f32> = drawn.parts.spread(&drawn.worlds, object.handling.spread).iter().map(|world| world.w_axis.x).collect();
        assert!((apart[1] - apart[0] - 0.12).abs() < 1e-5, "the two cubes stay 12 cm apart: {apart:?}");
    }

    #[test]
    fn a_named_part_is_located_where_it_is_drawn_spread_and_turned() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let born = Instant::now();
        let nodes = serde_json::json!([{ "mesh": 0, "name": "left", "translation": [-0.06, 0.0, 0.0] }, { "mesh": 0, "name": "right", "translation": [0.06, 0.0, 0.0] }]);
        let mut object = Object::spawn(Counted::new(), gpu, built(0.05, nodes, None, true), UNBUDGETED, born).unwrap();
        drawn(&mut object, born);
        let center = V3::from(view(None).center(0));
        let right = V3::from(object.locate("right").unwrap());
        assert!(right.abs_diff_eq(center + V3::new(0.06, 0.0, 0.0), 1e-5), "the right cube's centre: {right}");
        assert_eq!(object.locate("missing"), None);
        object.handling.spread = 1.0;
        object.handling.yaw = std::f32::consts::FRAC_PI_2;
        object.step(born, &view(None)).unwrap();
        let moved = V3::from(object.locate("right").unwrap());
        assert!(moved.abs_diff_eq(center + V3::new(0.0, 0.0, -0.12), 1e-5), "spread to 12 cm and turned a quarter: {moved}");
    }

    #[test]
    fn several_objects_stand_at_their_own_places_point_by_place_and_one_flicked_away_leaves_the_rest_in_place() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        let named = |name: &str| built(0.05, serde_json::json!([{ "mesh": 0, "name": name }]), None, false);
        let mut shelf: Shelf<Counted> = Shelf::default();
        for name in ["gear", "gear", "wheel"] {
            shelf.show(|_| Object::spawn(Counted::new(), gpu.clone(), named(name), UNBUDGETED, now));
            drawn(shelf.objects.last_mut().unwrap(), now);
        }
        let at = |place: usize| V3::from(view(None).center(place));
        for (place, object) in shelf.objects.iter().enumerate() {
            assert_eq!(object.place, place);
        }
        assert!(at(1).distance(at(0)) >= SIZE && at(2).distance(at(1)) >= SIZE, "neighbouring places hold the largest objects apart");
        assert!(V3::from(shelf.locate("gear").unwrap()).abs_diff_eq(at(1), 1e-5), "a bare name means the newest object that has it");
        assert!(V3::from(shelf.locate("1:gear").unwrap()).abs_diff_eq(at(0), 1e-5), "a place names the object there");
        assert!(V3::from(shelf.locate("3:wheel").unwrap()).abs_diff_eq(at(2), 1e-5));
        assert_eq!(shelf.locate("3:gear"), None);
        assert_eq!(shelf.locate("4:wheel"), None, "an empty place names no object");
        shelf.hold(Hold::At(2.0), Some("1:gear"));
        assert!(shelf.objects[0].handling.held && !shelf.objects[2].handling.held, "a hold reaches the object its pointing names");
        shelf.hold(Hold::At(2.0), None);
        assert!(shelf.objects[2].handling.held, "and without pointing, the newest");
        let tip = |offset: f32| view(Some((at(1) + V3::new(offset, 0.0, 0.0)).into()));
        shelf.step(now, &tip(0.0));
        shelf.step(now + Duration::from_millis(11), &tip(0.03));
        assert_eq!(shelf.objects.iter().map(|object| object.place).collect::<Vec<_>>(), [0, 2], "a flick throws away only the object it hits");
        assert!(V3::from(shelf.locate("wheel").unwrap()).abs_diff_eq(at(2), 1e-5), "the rest stay where they were");
        shelf.show(|place| {
            assert_eq!(place, 1, "a new object takes the nearest free place");
            Object::spawn(Counted::new(), gpu.clone(), named("wheel"), UNBUDGETED, now)
        });
        drawn(shelf.objects.last_mut().unwrap(), now);
        assert!(V3::from(shelf.locate("wheel").unwrap()).abs_diff_eq(at(1), 1e-5));
        shelf.clear("cleared by the hub");
        assert!(!shelf.shown());
    }

    #[test]
    fn walking_around_a_set_down_puppet_leaves_its_object_where_it_was_put() {
        use crate::placement::{on_controller, Anchor};
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let born = Instant::now();
        let nodes = serde_json::json!([{ "mesh": 0, "name": "left", "translation": [-0.06, 0.0, 0.0] }, { "mesh": 0, "name": "right", "translation": [0.06, 0.0, 0.0] }]);
        let mut object = Object::spawn(Counted::new(), gpu, built(0.05, nodes, None, true), UNBUDGETED, born).unwrap();
        drawn(&mut object, born);
        let hand = |angle: f32, at: Vec3| Pose::from_axes([angle.cos(), 0.0, -angle.sin()], [0.0, 1.0, 0.0], [angle.sin(), 0.0, angle.cos()], at);
        let held = on_controller(&hand(0.7, [0.3, 1.0, -0.6]));
        let floor = hand(-1.2, [0.5, 0.0, -1.1]);
        for (set_down, size) in [(Anchor::Stuck(held), 1.0), (Anchor::Grounded(crate::placement::Floor::new(floor)), 5.0)] {
            let mut anchor = set_down;
            let pose = anchor.stand(born, &hand(0.0, [0.0; 3]), [0.0; 3]);
            let place = pose.apply(beside(size, 0));
            let mut placed = Vec::new();
            for step in 0..8 {
                let around = step as f32 * std::f32::consts::TAU / 8.0;
                let head = facing([place[0] + 0.8 * around.sin(), place[1] + 0.4, place[2] + 0.8 * around.cos()], place);
                let stand = anchor.stand(born, &hand(around, head.t), head.t);
                let eye = |x| Pose::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [x, 0.0, 0.0]);
                object.step(born, &View { stand, size, head, eyes: [eye(-0.032), eye(0.032)], tip: None }).unwrap();
                placed.push((object.drawn.as_ref().unwrap().placed, V3::from(object.locate("right").unwrap())));
            }
            let (first, right) = placed[0];
            for (step, (matrix, part)) in placed.iter().enumerate() {
                assert!(matrix.abs_diff_eq(first, 1e-5) && part.abs_diff_eq(right, 1e-5), "{size}x: seen from the {step}th eighth of the way around, the object stays put: {matrix} {part} against {first} {right}");
            }
            assert!(right.abs_diff_eq(V3::from(place) + V3::new(0.06, 0.0, 0.0), 1e-5), "{size}x: beside the puppet where it was set down: {right}");
        }
    }

    #[test]
    fn live_data_fed_to_a_shown_object_moves_it_from_the_program_thread() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let born = Instant::now();
        let mut shelf = Shelf::default();
        shelf.show(|_| Object::spawn(Counted::new(), gpu, scene(0.2, Some((crate::program::tests::GAUGE, 0.0))), UNBUDGETED, born));
        shelf.feed(b"\x05".to_vec(), None);
        let deadline = Instant::now() + HANG;
        loop {
            assert!(Instant::now() < deadline, "the data never reached the drawing");
            shelf.step(born, &view(None));
            if shelf.objects.last().unwrap().drawn.as_ref().is_some_and(|drawn| drawn.worlds[0].w_axis.x == 5.0) {
                break;
            }
            std::thread::sleep(TICK);
        }
    }

    #[test]
    fn an_object_that_asks_for_the_device_feed_reads_this_machine_and_stays_while_fed() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let born = Instant::now();
        let bytes = built_with(0.2, serde_json::json!([{ "mesh": 0 }]), Some((crate::program::tests::GAUGE, 0.0)), false, serde_json::json!({ "feed": "device" }));
        let mut object = Object::spawn(Counted::new(), gpu, bytes, UNBUDGETED, born).unwrap();
        let cores = std::fs::read_to_string("/proc/stat").unwrap().lines().filter(|line| line.starts_with("cpu") && line.as_bytes()[3].is_ascii_digit()).count();
        let late = born + LIFETIME - Duration::from_secs(1);
        let deadline = Instant::now() + HANG;
        loop {
            assert!(Instant::now() < deadline, "the device's readings never reached the drawing");
            object.step(late, &view(None)).unwrap();
            if object.drawn.as_ref().is_some_and(|drawn| drawn.worlds[0].w_axis.x == (cores % 256) as f32) {
                break;
            }
            std::thread::sleep(TICK);
        }
        object.step(late + LIFETIME / 2, &view(None)).expect("fed live data, it outlives its untouched lifetime");
    }

    #[test]
    fn many_nodes_of_interleaved_materials_draw_in_one_call_per_material() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        let nodes: Vec<serde_json::Value> = (0..500).map(|at| serde_json::json!({ "mesh": at % 2, "translation": [at as f32 * 0.001, 0.0, 0.0] })).collect();
        let mut object = Object::spawn(Counted::new(), gpu, built(0.001, serde_json::Value::Array(nodes), None, false), UNBUDGETED, now).unwrap();
        drawn(&mut object, now);
        let draws = &object.drawn.as_ref().unwrap().skinned.draws;
        assert_eq!(draws.iter().map(|draw| (draw.material, draw.count)).collect::<Vec<_>>(), [(0, 250 * 36), (1, 250 * 36)]);
    }

    #[test]
    fn an_object_removed_on_one_slow_frame_logs_that_frame_s_cost() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        let mut object = Object::spawn(Counted::new(), gpu, cube(0.1), Budget { mean: Duration::from_secs(5), frame: Duration::ZERO }, now).unwrap();
        drawn(&mut object, now);
        let error = (0..2 * WINDOW as usize).find_map(|_| object.step(now, &view(None)).err()).expect("stopped on its first measured frame");
        assert!(error.contains("one frame"), "{error}");
        let logged = object.cost();
        assert!(logged.starts_with("running mean") && !logged.contains("mean 0.00 ms") && !logged.contains("worst 0.00 ms"), "{logged}");
    }

    #[test]
    fn an_object_over_its_budget_is_stopped_after_its_first_frames() {
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        let tight = Budget { mean: Duration::from_nanos(1), frame: Duration::MAX };
        let mut object = Object::spawn(Counted::new(), gpu, cube(0.1), tight, now).unwrap();
        drawn(&mut object, now);
        let error = (0..2 * WINDOW as usize).find_map(|_| object.step(now, &view(None)).err()).expect("stopped within its window");
        assert!(error.contains("budget"), "{error}");
        assert_eq!(object.frames as f32, 2.0 * WINDOW, "measured only after its first {WINDOW} frames, then over a window of them");
    }

    struct Held {
        inside: usize,
        open: bool,
        frames: usize,
    }

    static HELD: (Mutex<Held>, std::sync::Condvar) = (Mutex::new(Held { inside: 0, open: false, frames: 0 }), std::sync::Condvar::new());

    #[test]
    fn a_program_that_never_returns_is_stopped_without_stalling_the_render_thread() {
        let held = |program: &mut program::Program, time, tip| {
            let (state, changed) = &HELD;
            let mut held = state.lock().unwrap();
            held.inside += 1;
            changed.notify_all();
            loop {
                let frames = held.frames;
                let waited;
                (held, waited) = changed.wait_timeout_while(held, HANG, |held| !held.open && held.frames == frames).unwrap();
                if held.open || waited.timed_out() {
                    break;
                }
            }
            held.inside -= 1;
            drop(held);
            program.tick(time, tip)
        };
        let spin = r#"(module (memory (export "memory") 1) (func (export "tick") (param f32 f32 f32 f32 i32) (result i32) (loop $l (br $l)) (i32.const 0)))"#;
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        let mut object = Object::loading(Counted::new(), gpu, scene(0.2, Some((spin, 0.0))), Work { tick: held, ..WORK }, UNBUDGETED, now).unwrap();
        let (state, changed) = &HELD;
        let step = |object: &mut Object<Counted>| {
            let stepped = object.step(now, &view(None));
            state.lock().unwrap().frames += 1;
            changed.notify_all();
            stepped
        };
        let deadline = Instant::now() + HANG;
        while object.drawn.is_none() || state.lock().unwrap().inside == 0 {
            assert!(Instant::now() < deadline, "the program never published and ticked");
            step(&mut object).unwrap();
        }
        let frames = object.frames;
        for _ in 0..2 * WINDOW as u32 {
            step(&mut object).unwrap();
        }
        assert_eq!(object.frames, frames + 2 * WINDOW as u32, "every step drew a frame");
        assert_eq!(state.lock().unwrap().inside, 1, "the program's thread stayed inside one tick while the render thread drew every one of those frames");
        state.lock().unwrap().open = true;
        changed.notify_all();
        let deadline = Instant::now() + HANG;
        let error = loop {
            assert!(Instant::now() < deadline, "the program was never stopped");
            if let Err(error) = step(&mut object) {
                break error;
            }
        };
        assert!(error.contains("budget"), "{error}");
    }

    #[test]
    fn a_file_whose_size_is_claimed_rather_than_carried_is_refused_before_it_is_allocated() {
        let refused = |nodes: serde_json::Value, edit: &dyn Fn(&mut Builder)| {
            let mut builder = Builder::new(serde_json::json!({}));
            let position = builder.accessor("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
            builder.set("nodes", nodes);
            edit(&mut builder);
            load(&builder.glb(), &AtomicBool::new(false)).err().expect("refused")
        };
        let one = serde_json::json!([{ "mesh": 0 }]);
        let claimed = refused(one.clone(), &|builder| {
            builder.push_accessor(serde_json::json!({ "bufferView": 0, "componentType": 5126, "count": 4_000_000_000_000u64, "type": "VEC3" }));
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": 1 } }] }]));
        });
        assert!(claimed.contains("outside its bufferView"), "{claimed}");
        let unstored = refused(one.clone(), &|builder| {
            builder.push_accessor(serde_json::json!({ "componentType": 5126, "count": 4_000_000_000_000u64, "type": "VEC3" }));
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": 1 } }] }]));
        });
        assert!(unstored.contains("too many values"), "{unstored}");
        let repeated = refused(serde_json::json!([{ "mesh": 0 }]), &|builder| {
            builder.push_accessor(serde_json::json!({ "componentType": 5126, "count": (1 << 24) / 3 / 2, "type": "VEC3" }));
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": 1 } }, { "attributes": { "POSITION": 1 } }, { "attributes": { "POSITION": 1 } }] }]));
        });
        assert!(repeated.contains("too many values"), "one unstored accessor read three times counts three times: {repeated}");
        let shared = refused(serde_json::Value::Array((0..crate::vrm::OBJECT_NODES).map(|_| serde_json::json!({ "mesh": 0 })).collect()), &|builder| {
            let corners: Vec<f32> = (0..200 * 3).map(|value| value as f32).collect();
            let position = builder.accessor("VEC3", &corners);
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": position } }] }]));
        });
        assert!(shared.contains("vertices"), "one mesh drawn by every node: {shared}");
        let chain: Vec<serde_json::Value> = (0..100).map(|node| if node < 99 { serde_json::json!({ "mesh": 0, "children": [node + 1] }) } else { serde_json::json!({ "mesh": 0 }) }).collect();
        assert!(refused(serde_json::Value::Array(chain), &|_| {}).contains("deep"));
        let huge = refused(one, &|builder| {
            let mut png = Vec::new();
            image::RgbaImage::new(1, 1).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
            png[16..24].copy_from_slice(&[0, 0, 0x75, 0x30, 0, 0, 0x75, 0x30]);
            let view = builder.view(&png);
            builder.set("images", serde_json::json!([{ "bufferView": view, "mimeType": "image/png" }]));
            builder.set("textures", serde_json::json!([{ "source": 0 }]));
            builder.set("materials", serde_json::json!([{ "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }, { "pbrMetallicRoughness": { "baseColorFactor": [0.2, 0.4, 0.8, 1.0] } }]));
            builder.set("meshes", serde_json::json!([{ "primitives": [{ "attributes": { "POSITION": 0 }, "material": 0 }] }]));
        });
        assert!(huge.contains("image 0"), "a 30000-texel header is refused from the header: {huge}");
    }

    #[test]
    fn stirring_the_tip_inside_turns_the_object_and_a_flick_throws_it_away() {
        let start = Instant::now();
        let mut handling = Handling::new(start);
        let still = |tip: Vec3| Some((tip, tip));
        let frame = Duration::from_millis(11);
        let mut now = start;
        for step in 0..=45 {
            let angle = step as f32 * std::f32::consts::FRAC_PI_2 / 45.0;
            assert_eq!(handling.touch(now, 0.1, still([0.08 * angle.sin(), 0.0, 0.08 * angle.cos()])), None);
            now += frame;
        }
        assert!((handling.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "a quarter turn of the tip turns it a quarter: {}", handling.yaw);
        assert_eq!(handling.touch(now, 0.1, still([0.5, 0.0, 0.0])), None, "outside its reach nothing happens");
        now += frame;
        assert_eq!(handling.touch(now, 0.1, still([0.0, 0.0, 0.0])), None);
        now += frame;
        assert_eq!(handling.touch(now, 0.1, Some(([0.03, 0.0, 0.0], [0.0; 3]))), None, "the object carried 3 cm past a still tip is not a flick");
        now += frame;
        assert_eq!(handling.touch(now, 0.1, Some(([0.0, 0.0, 0.0], [0.03, 0.0, 0.0]))), Some("flicked away"), "the tip moving 3 cm in 11 ms is past {FLICK} m/s");
    }

    #[test]
    fn an_untouched_object_times_out_and_a_touch_restarts_its_lifetime() {
        let start = Instant::now();
        let mut handling = Handling::new(start);
        let still = |tip: Vec3| Some((tip, tip));
        assert_eq!(handling.touch(start + LIFETIME / 2, 0.1, still([0.0, 0.05, 0.0])), None);
        assert_eq!(handling.touch(start + LIFETIME, 0.1, None), None, "the touch restarted it");
        assert_eq!(handling.touch(start + LIFETIME / 2 + LIFETIME, 0.1, None), Some("timed out"));
    }

    /// Holds every gated load until the test opens it, so a frame that waited on a loader would see it end.
    static GATE: (Mutex<bool>, std::sync::Condvar) = (Mutex::new(false), std::sync::Condvar::new());

    struct Watched(Arc<AtomicBool>);

    impl Drop for Watched {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl Surface for Watched {
        fn place_in_world(&self, _: &Pose) {}
        fn texture(&self, _: &mut openvr_sys::VRVulkanTextureData_t) -> Result<(), String> {
            Ok(())
        }
        fn show(&mut self) {}
        fn hide(&mut self) {}
    }

    #[test]
    fn an_object_replaced_mid_load_leaves_within_a_frame_and_its_thread_ends_after() {
        let gated = |bytes: &[u8], stop: &AtomicBool| {
            let (open, opened) = &GATE;
            let _ = opened.wait_timeout_while(open.lock().unwrap(), HANG, |open| !*open).unwrap();
            load(bytes, stop)
        };
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        let mut shelf = Shelf::default();
        let shown = |shelf: &mut Shelf<Watched>| {
            let gone = Arc::new(AtomicBool::new(false));
            shelf.show(|_| Object::loading(Watched(gone.clone()), gpu.clone(), cube(0.2), Work { load: gated, ..WORK }, UNBUDGETED, now));
            (gone, Arc::downgrade(&shelf.objects.last().unwrap().runner.exchange))
        };
        let (first, first_exchange) = shown(&mut shelf);
        shelf.step(now, &view(None));
        let beside: Vec<_> = (1..MOST).map(|_| shown(&mut shelf)).collect();
        assert!(!first.load(Ordering::SeqCst), "a new object joins the shown ones while there is a free place");
        let (second, second_exchange) = shown(&mut shelf);
        assert!(first.load(Ordering::SeqCst), "with every place taken, the oldest object left in the frame that replaced it");
        assert!(!second.load(Ordering::SeqCst));
        shelf.step(now, &view(None));
        shelf.clear("session ended");
        assert!(second.load(Ordering::SeqCst) && beside.iter().all(|(gone, _)| gone.load(Ordering::SeqCst)), "the cleared objects left in the frame that cleared them");
        assert!(first_exchange.strong_count() > 0 && second_exchange.strong_count() > 0, "both loads are still held, so neither leaving waited for its thread");
        *GATE.0.lock().unwrap() = true;
        GATE.1.notify_all();
        let deadline = Instant::now() + HANG;
        while first_exchange.strong_count() + second_exchange.strong_count() > 0 {
            assert!(Instant::now() < deadline, "a loading thread never ended");
            std::thread::sleep(TICK);
        }
    }

    #[test]
    fn a_panic_while_loading_fails_the_object_at_once() {
        let broken = |_: &[u8], _: &AtomicBool| panic!("a broken file");
        let gpu = Rc::new(Gpu::new(None).unwrap());
        let now = Instant::now();
        let mut object = Object::loading(Counted::new(), gpu, cube(0.2), Work { load: broken, ..WORK }, UNBUDGETED, now).unwrap();
        let deadline = Instant::now() + HANG;
        let error = loop {
            assert!(Instant::now() < deadline, "the panic never reached the render thread");
            if let Err(error) = object.step(now, &view(None)) {
                break error;
            }
            std::thread::sleep(TICK);
        };
        assert!(error.contains("panicked"), "{error}");
    }

    fn threads() -> usize {
        std::fs::read_dir("/proc/self/task").unwrap().count()
    }

    /// Program threads end on their own after their object leaves; waits for the count to fall to `limit`.
    fn settled(limit: usize) -> usize {
        let deadline = Instant::now() + HANG;
        while threads() > limit && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        threads()
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
        let bytes = [cube(0.2), scene(0.2, Some((crate::program::tests::FOLLOW, 0.0)))];
        let mut shelf: Shelf<Counted> = Shelf::default();
        let start = Instant::now();
        let spawn = |shelf: &mut Shelf<Counted>, now: Instant| {
            shelf.show(|_| Object::spawn(Counted::new(), gpu.clone(), bytes[now.duration_since(start).as_secs() as usize % 5 % 2].clone(), UNBUDGETED, now));
            drawn(shelf.objects.last_mut().unwrap(), now);
        };
        spawn(&mut shelf, start);
        let warming = Arc::downgrade(&shelf.objects.last().unwrap().runner.exchange);
        shelf.clear("warm-up");
        while warming.strong_count() > 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let (surfaces, allocations, threads_before, resident_before) = (SURFACES.load(Ordering::SeqCst), gpu.allocations(), threads(), resident());
        assert_eq!(surfaces, 0);
        let mut peak = 0;
        for round in 0..120 {
            let now = start + Duration::from_secs(round);
            spawn(&mut shelf, now);
            assert_eq!(SURFACES.load(Ordering::SeqCst), 1, "one overlay while shown");
            assert!(gpu.allocations() > allocations && settled(threads_before + 1) <= threads_before + 1);
            match round % 4 {
                0 => {
                    shelf.step(now, &view(Some([0.0, 1.2, -0.5])));
                    shelf.step(now + Duration::from_millis(11), &view(Some([0.04, 1.2, -0.5])));
                }
                1 => shelf.step(now + LIFETIME, &view(None)),
                2 => {
                    for _ in 0..MOST {
                        spawn(&mut shelf, now);
                    }
                }
                _ => shelf.clear("session ended"),
            }
            if round % 4 == 2 {
                assert_eq!(SURFACES.load(Ordering::SeqCst), MOST, "with every place taken a new object replaces the oldest, one overlay a place");
                shelf.clear("asleep");
            }
            assert!(!shelf.shown(), "round {round}: the object is gone");
            peak = peak.max(resident());
        }
        let growth = resident().saturating_sub(resident_before);
        eprintln!("cleanup: overlays {} gpu allocations {} -> {} threads {} -> {} resident {} -> {} MiB (peak {} MiB)", SURFACES.load(Ordering::SeqCst), allocations, gpu.allocations(), threads_before, threads(), resident_before >> 20, resident() >> 20, peak >> 20);
        assert_eq!(SURFACES.load(Ordering::SeqCst), 0, "every overlay closed");
        assert_eq!(gpu.allocations(), allocations, "every GPU allocation freed");
        assert_eq!(settled(threads_before), threads_before, "every program thread ended");
        assert!(growth < 24 << 20, "resident memory grew {} MiB over 120 objects", growth >> 20);
    }
}
