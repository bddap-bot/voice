use std::collections::HashMap;

use glam::{Quat, Vec3};
use serde::Deserialize;

use crate::vrm::{Humanoid, Version};

pub const IDLES: [&str; 3] = ["idle", "idle-2", "idle-3"];
const IDLE_FADE: f32 = 0.6;
const BLINK: f32 = 0.18;

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
                let height = source.hips_height.filter(|&height| height > 0.0).ok_or("motion has no source hips height")?;
                let Some(rest) = rest_hips else { continue };
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

struct Action {
    clip: String,
    time: f32,
    weight: f32,
    fade: Option<(f32, f32, f32, f32)>,
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
    next_idle: f32,
    next_blink: f32,
    blink_start: Option<f32>,
    now: f32,
    random: Random,
}

impl Animator {
    pub fn new(clips: HashMap<String, Clip>, random: Random) -> Animator {
        let mut animator = Animator { clips, actions: Vec::new(), idle: None, next_idle: f32::INFINITY, next_blink: 1.2, blink_start: None, now: 0.0, random };
        animator.play_idle();
        animator
    }

    fn play_idle(&mut self) {
        let available: Vec<&str> = IDLES.into_iter().filter(|name| self.clips.contains_key(*name)).collect();
        let choices: Vec<&str> = available.iter().copied().filter(|name| Some(*name) != self.idle.as_deref()).collect();
        let Some(name) = choices.get((self.random.next() * choices.len() as f32) as usize).or(available.first()).map(|name| name.to_string()) else { return };
        self.next_idle = if available.len() > 1 { self.now + 7.0 + self.random.next() * 7.0 } else { f32::INFINITY };
        self.play(&name);
        self.idle = Some(name);
    }

    fn play(&mut self, name: &str) {
        let fading = self.actions.iter().any(|action| action.weight > 0.0);
        for action in &mut self.actions {
            action.fade = Some((self.now, IDLE_FADE, action.weight, 0.0));
        }
        self.actions.retain(|action| action.clip != name);
        self.actions.push(Action { clip: name.to_owned(), time: 0.0, weight: if fading { 0.0 } else { 1.0 }, fade: fading.then_some((self.now, IDLE_FADE, 0.0, 1.0)) });
    }

    pub fn update(&mut self, delta: f32) {
        self.now += delta;
        for action in &mut self.actions {
            let duration = self.clips[&action.clip].duration;
            action.time = (action.time + delta) % duration;
            if let Some((start, length, from, to)) = action.fade {
                let progress = ((self.now - start) / length).clamp(0.0, 1.0);
                action.weight = from + (to - from) * progress;
                if progress >= 1.0 {
                    action.fade = None;
                }
            }
        }
        self.actions.retain(|action| action.weight > 0.0 || action.fade.is_some());
        if self.now >= self.next_idle {
            self.play_idle();
        }
        if self.blink_start.is_none() && self.now >= self.next_blink {
            self.blink_start = Some(self.now);
        }
        if self.blink_start.is_some_and(|start| self.now - start >= BLINK) {
            self.blink_start = None;
            self.next_blink = self.now + 2.2 + self.random.next() * 4.2;
        }
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
        Animator::new(clips, Random(7))
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
}
