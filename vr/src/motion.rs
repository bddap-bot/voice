use std::collections::HashMap;

use glam::{Quat, Vec3};
use serde::Deserialize;

use crate::vrm::{Humanoid, Version};

/// The family of the standing idles: the overlay rotates through every clip of it.
pub const IDLE: &str = "idle";
/// docs/moods.json: the face each mood draws and how the figure shows its own expressions.
const ACTING: &str = include_str!("../../docs/moods.json");
/// The page's easing of a face weight toward its mood, per frame at 60 fps.
const FACE_EASE: f32 = 0.12;
const IDLE_FADE: f32 = 0.6;
/// The page's crossfade into a gesture.
const GESTURE_FADE: f32 = 0.18;
const BLINK: f32 = 0.18;

#[derive(Deserialize)]
struct Acting {
    moods: HashMap<String, HashMap<String, f32>>,
    own: Own,
}

#[derive(Deserialize)]
struct Own {
    chance: f32,
    seconds: f32,
}

fn acting() -> &'static Acting {
    static PARSED: std::sync::OnceLock<Acting> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(ACTING).expect("docs/moods.json"))
}

/// The page's moods, each a clip family too when the catalog has one.
pub fn moods() -> impl Iterator<Item = &'static str> {
    acting().moods.keys().map(String::as_str)
}

/// A clip's family: its action without a trailing `-<n>`, so `wave-2` is another `wave`.
pub fn family(action: &str) -> &str {
    match action.rsplit_once('-') {
        Some((family, number)) if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) => family,
        _ => action,
    }
}

#[derive(Deserialize)]
struct Track {
    name: String,
    times: Vec<f32>,
    values: Vec<f32>,
}

#[derive(Deserialize)]
struct Source {
    duration: f32,
    #[serde(rename = "hipsHeight")]
    hips_height: Option<f32>,
    tracks: Vec<Track>,
}

struct Keys<T> {
    times: Vec<f32>,
    values: Vec<T>,
}

impl<T: Copy> Keys<T> {
    fn sample(&self, time: f32, mix: impl Fn(T, T, f32) -> T) -> T {
        let after = self.times.partition_point(|&at| at <= time);
        if after == 0 {
            return self.values[0];
        }
        if after == self.times.len() {
            return self.values[after - 1];
        }
        let (start, end) = (self.times[after - 1], self.times[after]);
        mix(self.values[after - 1], self.values[after], (time - start) / (end - start))
    }
}

pub struct Clip {
    duration: f32,
    rotations: Vec<(String, Keys<Quat>)>,
    hips: Option<Keys<Vec3>>,
}

impl Clip {
    pub fn parse(bytes: &[u8], version: Version, rest_hips: Option<Vec3>) -> Result<Clip, String> {
        let source: Source = serde_json::from_slice(bytes).map_err(|error| format!("invalid motion: {error}"))?;
        if !source.duration.is_finite() || source.duration <= 0.0 {
            return Err("invalid motion duration".into());
        }
        let mirrored = version == Version::Zero;
        let mut rotations = Vec::new();
        let mut hips = None;
        for track in source.tracks {
            let (bone, property) = track.name.split_once('.').ok_or("invalid motion track")?;
            let width = if property == "quaternion" { 4 } else if track.name == "hips.position" { 3 } else { return Err("invalid motion track".into()) };
            if track.times.is_empty() || track.values.len() != track.times.len() * width || track.times.windows(2).any(|pair| pair[1] < pair[0]) {
                return Err(format!("motion track {} disagrees with its times", track.name));
            }
            if width == 4 {
                let values = track.values.chunks_exact(4).map(|v| if mirrored { Quat::from_xyzw(-v[0], v[1], -v[2], v[3]) } else { Quat::from_xyzw(v[0], v[1], v[2], v[3]) }).collect();
                rotations.push((bone.to_owned(), Keys { times: track.times, values }));
            } else {
                let Some(rest) = rest_hips else { continue };
                let height = source.hips_height.filter(|&height| height > 0.0).ok_or("motion has no source hips height")?;
                let scale = rest.y.abs() / height;
                let values = track.values.chunks_exact(3).map(|v| Vec3::new(v[0], v[1], v[2]) * scale * if mirrored { Vec3::new(-1.0, 1.0, -1.0) } else { Vec3::ONE }).collect();
                hips = Some(Keys { times: track.times, values });
            }
        }
        if rotations.is_empty() {
            return Err("animation has no rotation tracks".into());
        }
        Ok(Clip { duration: source.duration, rotations, hips })
    }

    pub fn duration(&self) -> f32 {
        self.duration
    }

    pub fn anchor(&mut self, standing: &Humanoid) {
        for (bone, rotation) in &standing.rotations {
            match self.rotations.iter_mut().find(|(name, _)| name == bone) {
                Some((_, keys)) => {
                    let anchor = *rotation * keys.values[0].inverse();
                    keys.values.iter_mut().for_each(|value| *value = (anchor * *value).normalize());
                }
                None => self.rotations.push((bone.clone(), Keys { times: vec![0.0, self.duration], values: vec![*rotation; 2] })),
            }
        }
    }
}

struct Fade {
    start: f32,
    length: f32,
    from: f32,
    to: f32,
}

struct Action {
    clip: String,
    time: f32,
    weight: f32,
    fade: Option<Fade>,
    once: bool,
}

pub struct Random(u64);

impl Random {
    pub fn seeded() -> Random {
        Random(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |elapsed| elapsed.as_nanos() as u64) | 1)
    }

    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

pub struct Animator {
    clips: HashMap<String, Clip>,
    actions: Vec<Action>,
    idle: Option<String>,
    gesture: Option<String>,
    next_idle: f32,
    next_blink: f32,
    blink_start: Option<f32>,
    now: f32,
    random: Random,
    /// The figure's own expressions, those its file names beyond the VRM presets.
    own: Vec<String>,
    mood: Option<String>,
    shown: Option<(String, f32)>,
    faces: HashMap<String, f32>,
}

impl Animator {
    pub fn new(clips: HashMap<String, Clip>, own: Vec<String>, random: Random) -> Animator {
        let mut animator = Animator { clips, actions: Vec::new(), idle: None, gesture: None, next_idle: f32::INFINITY, next_blink: 1.2, blink_start: None, now: 0.0, random, own, mood: None, shown: None, faces: HashMap::new() };
        animator.play_idle();
        animator
    }

    fn variants(&self, of: &str) -> Vec<String> {
        let mut variants: Vec<String> = self.clips.keys().filter(|action| family(action) == of).cloned().collect();
        variants.sort();
        variants
    }

    fn pick(&mut self, from: &[String]) -> Option<String> {
        from.get(((self.random.next() * from.len() as f32) as usize).min(from.len().saturating_sub(1))).cloned()
    }

    fn play_idle(&mut self) {
        let available = self.variants(IDLE);
        let choices: Vec<String> = available.iter().filter(|name| Some(name.as_str()) != self.idle.as_deref()).cloned().collect();
        let Some(name) = self.pick(&choices).or(available.first().cloned()) else { return };
        self.next_idle = if available.len() > 1 { self.now + 7.0 + self.random.next() * 7.0 } else { f32::INFINITY };
        self.play(&name, IDLE_FADE, false);
        self.idle = Some(name);
        self.gesture = None;
        if self.random.next() < acting().own.chance {
            let own = self.own.clone();
            self.shown = self.pick(&own).map(|name| (name, self.now + acting().own.seconds));
        }
    }

    /// Shows a mood as the page's `mood` does: its face, and once its body clip when it changes and no gesture plays.
    pub fn mood(&mut self, name: &str) {
        if self.mood.as_deref() == Some(name) {
            return;
        }
        self.mood = Some(name.to_owned());
        if self.gesture.is_none() {
            self.gesture(name);
        }
    }

    /// The face weights to draw this frame: the mood's, or one of the figure's own expressions while it shows.
    pub fn faces(&self) -> impl Iterator<Item = (&str, f32)> {
        self.faces.iter().map(|(name, &weight)| (name.as_str(), weight))
    }

    fn play(&mut self, name: &str, length: f32, once: bool) {
        let fading = self.actions.iter().any(|action| action.weight > 0.0);
        for action in &mut self.actions {
            action.fade = Some(Fade { start: self.now, length, from: action.weight, to: 0.0 });
        }
        self.actions.retain(|action| action.clip != name);
        self.actions.push(Action { clip: name.to_owned(), time: 0.0, weight: if fading { 0.0 } else { 1.0 }, fade: fading.then_some(Fade { start: self.now, length, from: 0.0, to: 1.0 }), once });
    }

    /// Plays a gesture clip once, as the page's `gesture`, then returns to an idle; false without its clip.
    pub fn gesture(&mut self, name: &str) -> bool {
        let variants = self.variants(name);
        let Some(variant) = self.pick(&variants) else { return false };
        self.play(&variant, GESTURE_FADE, true);
        self.gesture = Some(name.to_owned());
        true
    }

    pub fn update(&mut self, delta: f32) -> Option<&str> {
        self.now += delta;
        for action in &mut self.actions {
            let duration = self.clips[&action.clip].duration;
            action.time = if action.once { (action.time + delta).min(duration) } else { (action.time + delta) % duration };
            if let Some(Fade { start, length, from, to }) = action.fade {
                let progress = ((self.now - start) / length).clamp(0.0, 1.0);
                action.weight = from + (to - from) * progress;
                if progress >= 1.0 {
                    action.fade = None;
                }
            }
        }
        self.actions.retain(|action| action.weight > 0.0 || action.fade.is_some());
        let finished = self.gesture.is_some() && self.actions.last().is_some_and(|action| action.once && action.time >= self.clips[&action.clip].duration);
        let switched = finished || (self.gesture.is_none() && self.now >= self.next_idle);
        if switched {
            self.play_idle();
        }
        let shown = self.shown.as_ref().filter(|(_, until)| self.now < *until).map(|(name, _)| name.clone());
        let targets: HashMap<String, f32> = match shown {
            Some(name) => HashMap::from([(name, 1.0)]),
            None => self.mood.as_ref().and_then(|mood| acting().moods.get(mood)).cloned().unwrap_or_default(),
        };
        let ease = 1.0 - (1.0 - FACE_EASE).powf(delta * 60.0);
        for name in targets.keys() {
            self.faces.entry(name.clone()).or_insert(0.0);
        }
        for (name, weight) in &mut self.faces {
            *weight += (targets.get(name).copied().unwrap_or(0.0) - *weight) * ease;
        }
        self.faces.retain(|name, weight| targets.contains_key(name) || *weight > 1e-3);
        if self.blink_start.is_none() && self.now >= self.next_blink {
            self.blink_start = Some(self.now);
        }
        if self.blink_start.is_some_and(|start| self.now - start >= BLINK) {
            self.blink_start = None;
            self.next_blink = self.now + 2.2 + self.random.next() * 4.2;
        }
        switched.then(|| self.idle.as_deref()).flatten()
    }

    pub fn blink(&self) -> f32 {
        self.blink_start.map_or(0.0, |start| (((self.now - start) / BLINK).min(1.0) * std::f32::consts::PI).sin())
    }

    pub fn humanoid(&self, standing: &Humanoid, rest_hips: Option<Vec3>) -> Humanoid {
        let mut rotations: HashMap<String, (Quat, f32)> = HashMap::new();
        let mut hips: Option<(Vec3, f32)> = None;
        for action in self.actions.iter().filter(|action| action.weight > 0.0) {
            let clip = &self.clips[&action.clip];
            for (bone, keys) in &clip.rotations {
                let value = keys.sample(action.time, |a, b, t| a.slerp(b, t));
                rotations
                    .entry(bone.clone())
                    .and_modify(|(sum, total)| {
                        *total += action.weight;
                        *sum = sum.slerp(value, action.weight / *total);
                    })
                    .or_insert((value, action.weight));
            }
            if let Some(keys) = &clip.hips {
                let value = keys.sample(action.time, Vec3::lerp);
                hips = Some(match hips {
                    Some((sum, total)) => (sum.lerp(value, action.weight / (total + action.weight)), total + action.weight),
                    None => (value, action.weight),
                });
            }
        }
        let mut humanoid = standing.clone();
        for (bone, (value, total)) in rotations {
            let original = standing.rotations.get(&bone).copied().unwrap_or(Quat::IDENTITY);
            humanoid.rotations.insert(bone, if total < 1.0 { value.slerp(original, 1.0 - total) } else { value });
        }
        humanoid.hips = match (hips, rest_hips) {
            (Some((value, total)), Some(rest)) if total < 1.0 => Some(value.lerp(rest, 1.0 - total)),
            (Some((value, _)), _) => Some(value),
            (None, _) => None,
        };
        humanoid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion(turns: &[f32], hips: Option<&[f32]>) -> Vec<u8> {
        let times: Vec<f32> = (0..turns.len()).map(|key| key as f32).collect();
        let mut tracks = vec![serde_json::json!({ "name": "spine.quaternion", "times": times, "values": turns.iter().flat_map(|&angle| Quat::from_rotation_z(angle).to_array()).collect::<Vec<f32>>() })];
        if let Some(hips) = hips {
            tracks.push(serde_json::json!({ "name": "hips.position", "times": times, "values": hips.iter().flat_map(|&y| [0.0, y, 0.0]).collect::<Vec<f32>>() }));
        }
        serde_json::to_vec(&serde_json::json!({ "name": "test", "duration": turns.len() as f32, "hipsHeight": 100.0, "tracks": tracks })).unwrap()
    }

    fn angle(rotation: Quat) -> f32 {
        let (axis, angle) = rotation.to_axis_angle();
        angle * axis.z.signum()
    }

    #[test]
    fn a_track_interpolates_between_keys_and_holds_past_its_ends() {
        let clip = Clip::parse(&motion(&[0.0, 1.0, 0.5], None), Version::One, None).unwrap();
        let keys = &clip.rotations[0].1;
        for (time, expected) in [(-1.0, 0.0), (0.5, 0.5), (1.5, 0.75), (2.5, 0.5)] {
            assert!((angle(keys.sample(time, |a, b, t| a.slerp(b, t))) - expected).abs() < 1e-4, "at {time}");
        }
    }

    #[test]
    fn a_vrm_0_figure_takes_the_motion_mirrored_and_hips_scaled_to_its_own_height() {
        let clip = Clip::parse(&motion(&[0.4, 0.4], Some(&[50.0, 50.0])), Version::Zero, Some(Vec3::new(0.0, 0.9, 0.0))).unwrap();
        let rotation = clip.rotations[0].1.values[0];
        assert!(rotation.abs_diff_eq(Quat::from_xyzw(-0.0, 0.0, -(0.2f32).sin(), (0.2f32).cos()), 1e-6));
        assert!(clip.hips.unwrap().values[0].abs_diff_eq(Vec3::new(0.0, 0.45, 0.0), 1e-6));
    }

    #[test]
    fn an_invalid_motion_is_refused() {
        assert!(Clip::parse(b"{}", Version::One, None).is_err());
        let short = serde_json::json!({ "duration": 1.0, "tracks": [{ "name": "spine.quaternion", "times": [0.0], "values": [0.0, 0.0] }] });
        assert!(Clip::parse(&serde_json::to_vec(&short).unwrap(), Version::One, None).err().unwrap().contains("disagrees"));
        let scale = serde_json::json!({ "duration": 1.0, "tracks": [{ "name": "spine.scale", "times": [0.0], "values": [1.0, 1.0, 1.0] }] });
        assert!(Clip::parse(&serde_json::to_vec(&scale).unwrap(), Version::One, None).is_err());
    }

    #[test]
    fn anchoring_starts_a_standing_idle_from_the_standing_pose() {
        let mut clip = Clip::parse(&motion(&[0.3, 0.5], None), Version::One, None).unwrap();
        let standing = Humanoid { rotations: HashMap::from([("spine".to_owned(), Quat::from_rotation_z(1.0)), ("neck".to_owned(), Quat::from_rotation_x(0.2))]), hips: None };
        clip.anchor(&standing);
        let spine = &clip.rotations.iter().find(|(bone, _)| bone == "spine").unwrap().1;
        assert!((angle(spine.values[0]) - 1.0).abs() < 1e-5 && (angle(spine.values[1]) - 1.2).abs() < 1e-5, "the clip moves relative to its first frame");
        let neck = &clip.rotations.iter().find(|(bone, _)| bone == "neck").unwrap().1;
        assert!(neck.values.iter().all(|value| value.abs_diff_eq(Quat::from_rotation_x(0.2), 1e-6)), "a standing bone the clip lacks holds still");
    }

    fn animator(names: &[&str]) -> Animator {
        let clips = names.iter().enumerate().map(|(at, name)| (name.to_string(), Clip::parse(&motion(&[at as f32 * 0.5; 20], None), Version::One, None).unwrap())).collect();
        Animator::new(clips, Vec::new(), Random(7))
    }

    #[test]
    fn an_idle_change_crossfades_over_the_fade_time() {
        let mut animator = animator(&["idle", "idle-2"]);
        let standing = Humanoid::default();
        let first = animator.idle.clone().unwrap();
        let first_angle = angle(animator.humanoid(&standing, None).rotations["spine"]);
        let switch = animator.next_idle;
        assert!((7.0..=14.0).contains(&switch), "the next idle comes 7 to 14 s later: {switch}");
        while animator.now < switch {
            animator.update(0.05);
        }
        let second = animator.idle.clone().unwrap();
        assert_ne!(first, second, "the next idle differs from the last");
        animator.update(IDLE_FADE / 2.0);
        let middle = angle(animator.humanoid(&standing, None).rotations["spine"]);
        let second_angle = if first_angle == 0.0 { 0.5 } else { 0.0 };
        assert!((middle - (first_angle + second_angle) / 2.0).abs() < 0.05, "half way through the fade the pose is half way: {middle}");
        animator.update(IDLE_FADE);
        assert!((angle(animator.humanoid(&standing, None).rotations["spine"]) - second_angle).abs() < 1e-4);
        assert_eq!(animator.actions.len(), 1, "a faded-out idle is dropped");
    }

    #[test]
    fn a_bone_only_the_outgoing_idle_moves_returns_to_standing_during_the_fade() {
        let neck = |angle: f32| {
            let values = Quat::from_rotation_x(angle).to_array().repeat(2);
            serde_json::json!({ "name": "neck.quaternion", "times": [0.0, 1.0], "values": values })
        };
        let hips = serde_json::json!({ "name": "hips.position", "times": [0.0, 1.0], "values": [0.0, 120.0, 0.0, 0.0, 120.0, 0.0] });
        let first = serde_json::json!({ "name": "a", "duration": 1.0, "hipsHeight": 100.0, "tracks": [neck(0.6), hips] });
        let second = serde_json::json!({ "name": "b", "duration": 1.0, "tracks": [{ "name": "spine.quaternion", "times": [0.0], "values": [0.0, 0.0, 0.0, 1.0] }] });
        let rest = Some(Vec3::new(0.0, 1.0, 0.0));
        let clips = [("idle", first), ("idle-2", second)].map(|(name, json)| (name.to_owned(), Clip::parse(&serde_json::to_vec(&json).unwrap(), Version::One, rest).unwrap()));
        let mut animator = Animator::new(HashMap::new(), Vec::new(), Random(3));
        animator.clips = clips.into_iter().collect();
        animator.play("idle", IDLE_FADE, false);
        animator.play("idle-2", IDLE_FADE, false);
        animator.update(IDLE_FADE / 2.0);
        let standing = Humanoid { rotations: HashMap::from([("neck".to_owned(), Quat::from_rotation_x(0.2))]), hips: None };
        let mixed = animator.humanoid(&standing, rest);
        assert!((mixed.rotations["neck"].to_axis_angle().1 - 0.4).abs() < 1e-4, "half way, the neck is half way back to standing");
        assert!(mixed.hips.unwrap().abs_diff_eq(Vec3::new(0.0, 1.1, 0.0), 1e-5), "and the hips half way back to rest");
    }

    #[test]
    fn a_lone_idle_loops_without_switching() {
        let mut animator = animator(&["idle", "wave"]);
        assert_eq!(animator.idle.as_deref(), Some("idle"));
        assert_eq!(animator.next_idle, f32::INFINITY);
        for _ in 0..1000 {
            animator.update(0.05);
        }
        assert_eq!(animator.actions.len(), 1);
        assert!(animator.actions[0].time < 20.0, "the clip time wraps");
    }

    #[test]
    fn a_gesture_plays_once_over_the_idle_then_an_idle_returns() {
        let mut animator = animator(&["idle", "idle-2", "wave"]);
        assert!(!animator.gesture("bow"), "a gesture without its clip is refused");
        animator.next_idle = 1.0;
        assert!(animator.gesture("wave"));
        let standing = Humanoid::default();
        animator.update(GESTURE_FADE);
        assert!((angle(animator.humanoid(&standing, None).rotations["spine"]) - 1.0).abs() < 1e-4, "the gesture takes over within its fade");
        while animator.now < 19.0 {
            assert_eq!(animator.update(0.05), None, "no idle change while it plays");
        }
        let returned = animator.update(1.5);
        assert!(returned.is_some_and(|idle| idle.starts_with("idle")), "the clip ends and an idle follows: {returned:?}");
        animator.update(IDLE_FADE);
        assert_ne!(angle(animator.humanoid(&standing, None).rotations["spine"]), 1.0);
        assert!(animator.actions.iter().all(|action| action.clip != "wave"));
    }

    #[test]
    fn blinks_close_and_open_within_their_time_and_recur() {
        let mut animator = animator(&["idle"]);
        let mut closed = Vec::new();
        for step in 0..2000 {
            animator.update(0.01);
            if animator.blink() > 0.99 {
                closed.push(step as f32 * 0.01);
            }
        }
        assert!(closed[0] > 1.2 && closed[0] < 1.2 + BLINK, "the first blink comes after 1.2 s: {}", closed[0]);
        let gaps: Vec<f32> = closed.windows(2).map(|pair| pair[1] - pair[0]).filter(|gap| *gap > 0.5).collect();
        assert!(!gaps.is_empty() && gaps.iter().all(|gap| (2.2..=6.6).contains(gap)), "blinks recur 2.2 to 6.4 s apart: {gaps:?}");
    }

    #[test]
    fn a_numbered_clip_is_another_of_its_family() {
        assert_eq!(family("wave-2"), "wave");
        assert_eq!(family("idle-13"), "idle");
        assert_eq!(family("thumbs-up"), "thumbs-up");
        assert_eq!(family("look-around"), "look-around");
        assert_eq!(family("sit-idle-2"), "sit-idle");
    }

    #[test]
    fn every_idle_and_gesture_variant_takes_its_turn() {
        let mut animator = animator(&["idle", "idle-2", "idle-14", "wave", "wave-3"]);
        let mut idles = std::collections::HashSet::new();
        let mut waves = std::collections::HashSet::new();
        for _ in 0..60 {
            animator.play_idle();
            idles.insert(animator.idle.clone().unwrap());
            assert!(animator.gesture("wave"));
            waves.insert(animator.actions.last().unwrap().clip.clone());
        }
        assert_eq!(idles.len(), 3, "{idles:?}");
        assert_eq!(waves, ["wave", "wave-3"].map(String::from).into());
        assert!(!animator.gesture("nod"), "a gesture without a clip of its family is refused");
    }

    #[test]
    fn a_mood_eases_its_face_in_and_plays_its_body_clip_once() {
        let mut animator = animator(&["idle", "sad"]);
        animator.mood("sad");
        assert_eq!(animator.gesture.as_deref(), Some("sad"));
        for _ in 0..120 {
            animator.update(1.0 / 60.0);
        }
        let faces: HashMap<&str, f32> = animator.faces().collect();
        assert!((faces["sad"] - acting().moods["sad"]["sad"]).abs() < 0.01, "{faces:?}");
        let playing = animator.actions.len();
        animator.mood("sad");
        assert_eq!(animator.actions.len(), playing, "the same mood again replays nothing");
        animator.mood("neutral");
        for _ in 0..240 {
            animator.update(1.0 / 60.0);
        }
        assert_eq!(animator.faces().count(), 0, "neutral clears the face");
    }

    #[test]
    fn an_idle_change_sometimes_shows_one_of_the_figures_own_expressions_over_the_mood() {
        let clips = ["idle", "idle-2"].iter().map(|name| (name.to_string(), Clip::parse(&motion(&[0.0; 4], None), Version::One, None).unwrap())).collect();
        let mut animator = Animator::new(clips, vec!["smug".into()], Random(11));
        animator.mood("pleased");
        let mut shown = 0;
        for _ in 0..200 {
            animator.shown = None;
            animator.play_idle();
            for _ in 0..60 {
                animator.update(1.0 / 60.0);
            }
            let faces: HashMap<&str, f32> = animator.faces().collect();
            if faces.get("smug").is_some_and(|weight| *weight > 0.9) {
                shown += 1;
                assert!(faces.get("happy").map_or(true, |weight| *weight < 0.1), "the own expression replaces the mood's face: {faces:?}");
            }
        }
        let expected = acting().own.chance * 200.0;
        assert!((shown as f32 - expected).abs() < 30.0, "shown {shown} of 200 idle changes, expected about {expected}");
    }
}
