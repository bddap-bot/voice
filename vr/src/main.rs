mod openvr;
mod page;
mod placement;
mod vulkan;
mod vrm;
mod render;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;

use openvr::{Runtime, Signal};
use page::{Frame, Page};
use placement::{above_hand, desk_spot, Anchor, Hand, Interaction, Pose};
use render::{eye_projection, Renderer};
use vrm::Model;

const EYE: u32 = 768;
const QUAD: f32 = 0.4;
const HEIGHT: f32 = 0.3;
const MARGIN: f32 = 0.03;
const FLOOR: f32 = -QUAD / 2.0 + MARGIN;
const PAGE: &str = "https://bddap-bot.github.io/voice/";
const HEAD_AHEAD: f32 = 0.04;
const REQUEST_LOST: Duration = Duration::from_millis(100);
const IN_FLIGHT: usize = 3;

fn directory(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(fallback)).join("voice-vr")
}

fn load(path: &PathBuf) -> Option<Anchor> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn save(path: &PathBuf, anchor: &Anchor) {
    let temporary = path.with_extension("tmp");
    if let Err(error) = std::fs::write(&temporary, serde_json::to_vec(anchor).unwrap()).and_then(|()| std::fs::rename(&temporary, path)) {
        eprintln!("placement not saved: {error}");
    }
}

#[derive(Default)]
struct Meter {
    window: Option<Instant>,
    waits: Vec<Duration>,
    uploads: Vec<Duration>,
}

fn summary(samples: &mut [Duration]) -> String {
    samples.sort();
    let mean = samples.iter().sum::<Duration>().as_secs_f64() * 1e3 / samples.len() as f64;
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)].as_secs_f64() * 1e3;
    format!("mean {mean:.1} p95 {p95:.1} ms")
}

impl Meter {
    fn frame(&mut self, wait: Duration, upload: Duration) {
        let now = Instant::now();
        let window = *self.window.get_or_insert(now);
        self.waits.push(wait);
        self.uploads.push(upload);
        let elapsed = now - window;
        if elapsed >= Duration::from_secs(5) {
            let rate = self.waits.len() as f64 / elapsed.as_secs_f64();
            eprintln!("overlay {rate:.1} fps; pose→frame {}; upload+submit {}", summary(&mut self.waits), summary(&mut self.uploads));
            *self = Meter { window: Some(now), ..Meter::default() };
        }
    }
}

fn native(runtime: &Runtime, puppet: &std::path::Path) -> Result<(), String> {
    let model = Model::parse(&std::fs::read(puppet).map_err(|error| format!("{}: {error}", puppet.display()))?).map_err(|error| format!("{}: {error}", puppet.display()))?;
    let fit = model.pose(&model.rest()).fit(HEIGHT);
    let mut posed = model.pose(&model.standing());
    posed.place(&fit, model.version, FLOOR);
    let mut renderer = Renderer::new(vulkan::Gpu::new(Some(runtime))?, [EYE, EYE], &model)?;
    renderer.set_mesh(&posed)?;
    let mut overlay = runtime.create_overlay("voice.puppet", "Puppet", QUAD)?;
    let eye_offsets = runtime.eye_offsets();
    loop {
        if let Some(Signal::Quit) = runtime.poll() {
            return Ok(());
        }
        let poses = runtime.poses(0.0);
        let left = runtime.hands(&poses).into_iter().find(|hand| hand.hand == Hand::Left).zip(runtime.hand_index(Hand::Left));
        let (Some(head), Some((hand, device))) = (runtime.head(&poses), left) else {
            overlay.hide();
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let quad = above_hand(&hand.pose, &head, -FLOOR);
        overlay.place_on(device, &hand.pose.inverse().then(&quad));
        let local = quad.inverse();
        let eyes = eye_offsets.map(|eye: Pose| eye_projection(local.apply(head.then(&eye).t).into(), QUAD / 2.0, QUAD / 2.0));
        overlay.submit(&mut renderer.render(eyes)?)?;
        runtime.wait_frame();
    }
}

fn run() -> Result<(), String> {
    let config = directory("XDG_CONFIG_HOME", ".config");
    let state = directory("XDG_STATE_HOME", ".local/state");
    std::fs::create_dir_all(&state).map_err(|error| format!("{}: {error}", state.display()))?;
    let runtime = Runtime::init()?;
    if let Some(puppet) = std::env::var_os("VOICE_VR_PUPPET") {
        return native(&runtime, std::path::Path::new(&puppet));
    }
    let token_file = config.join("token");
    let token = std::fs::read_to_string(&token_file).map_err(|error| format!("{}: {error}", token_file.display()))?.trim().to_owned();
    let page_address = std::env::var("VOICE_VR_PAGE").unwrap_or_else(|_| PAGE.to_owned());
    let browser = std::env::var("VOICE_VR_BROWSER").unwrap_or_else(|_| "chromium".to_owned());
    let placement_file = state.join("placement.json");

    let mut uploader = vulkan::Uploader::new(vulkan::Gpu::new(Some(&runtime))?, EYE * 2, EYE)?;
    let mut overlay = runtime.create_overlay("voice.puppet", "Puppet", QUAD)?;
    let hello = json!({ "type": "hello", "token": token, "eye": [EYE, EYE], "quad": [QUAD, QUAD], "height": HEIGHT, "margin": MARGIN });
    let mut page = Page::open(&browser, &page_address, &state.join("browser"), hello).map_err(|error| format!("{browser}: {error}"))?;
    let eye_offsets = runtime.eye_offsets();
    let mut interaction = Interaction::new([0.0, -QUAD / 2.0 + MARGIN + HEIGHT * 0.55, 0.0]);
    let mut anchor = load(&placement_file);
    let started = Instant::now();
    let period = runtime.display_period()?;
    let mut meter = Meter::default();
    let mut requested: VecDeque<Instant> = VecDeque::new();
    let mut next_request = Instant::now();

    loop {
        if let Some(Signal::Quit) = runtime.poll() {
            return Ok(());
        }
        if let Some(status) = page.browser_exited() {
            return Err(format!("browser exited: {status}"));
        }
        let poses = runtime.poses(0.0);
        let Some(head) = runtime.head(&poses) else {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let frame = page.latest_frame().map(|(frame, count)| (frame, Instant::now(), requested.drain(..count.min(requested.len())).last()));
        let hands = runtime.hands(&poses);
        let current = *anchor.get_or_insert_with(|| Anchor::World(desk_spot(&head)));
        let hand_pose = |which| hands.iter().find(|hand| hand.hand == which).map(|hand| hand.pose);
        let placed = match (interaction.carrying(), current) {
            (Some((hand, offset)), _) => hand_pose(hand).map(|pose| pose.then(&offset)),
            (None, Anchor::World(pose)) => Some(pose),
            (None, Anchor::Wrist { hand, offset }) => hand_pose(hand).map(|pose| pose.then(&offset)),
        };
        match (interaction.carrying(), current) {
            (Some(_), _) | (None, Anchor::World(_)) => {
                if let Some(pose) = placed {
                    overlay.place_world(&pose);
                }
            }
            (None, Anchor::Wrist { hand, offset }) => {
                if let Some(index) = runtime.hand_index(hand) {
                    overlay.place_on(index, &offset);
                }
            }
        }
        if let Some(placed) = placed {
            match interaction.step(started.elapsed().as_secs_f32(), &current, &placed, &hands, &head) {
                Some(placement::Event::Tap) => page.send(json!({ "type": "tap" })),
                Some(placement::Event::Moved(moved)) => {
                    anchor = Some(moved);
                    save(&placement_file, &moved);
                }
                None => {}
            }
            if requested.front().is_some_and(|at| at.elapsed() >= REQUEST_LOST) {
                requested.clear();
            }
            if requested.len() < IN_FLIGHT && Instant::now() >= next_request {
                let predicted = runtime.head(&runtime.poses(HEAD_AHEAD)).unwrap_or(head);
                let local = placed.inverse();
                let eyes = eye_offsets.map(|eye: Pose| local.apply(predicted.then(&eye).t));
                page.pose(eyes, local.apply(predicted.t));
                let now = Instant::now();
                next_request = (next_request + period).max(now);
                requested.push_back(now);
            }
        }
        if let Some((frame, arrived, asked)) = frame {
            let mut texture = uploader.upload(&Frame::parse(frame, [EYE, EYE])?)?;
            overlay.submit(&mut texture)?;
            if let Some(asked) = asked {
                meter.frame(arrived - asked, arrived.elapsed());
            }
        }
        let due = next_request.saturating_duration_since(Instant::now());
        page.wait(if requested.len() < IN_FLIGHT && !due.is_zero() { due } else { period });
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("voice-vr: {error}");
        std::process::exit(1);
    }
}
