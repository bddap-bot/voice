//! VRChat's contacts as the importer writes them into `VRCHAT_expressions`' `contacts`: tagged spheres and capsules on
//! the figure's nodes, the receivers among them writing their parameter from the senders touching them, as Cairnfold's
//! `game/src/contacts.gd` does. The wearer's controllers are the other body's senders; the figure's own senders, the
//! built-in ones the importer writes from a VRChat descriptor among them, are its own.

use std::collections::{HashMap, HashSet};

use glam::{Mat4, Vec3};
use serde::Deserialize;

use super::expressions::{off, Controller, Expressions, ValueType};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Constant,
    OnEnter,
    Proximity,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receiver {
    #[serde(rename = "type")]
    pub kind: Kind,
    pub parameter: String,
    #[serde(rename = "self")]
    pub own: bool,
    pub others: bool,
    pub local: bool,
    pub min_velocity: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Shape {
    pub offset: [f32; 3],
    #[serde(default)]
    pub tail: Option<[f32; 3]>,
    pub radius: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Contact {
    pub node: usize,
    pub shape: Shape,
    pub tags: Vec<String>,
    #[serde(default)]
    pub when: Vec<(usize, f32)>,
    #[serde(default)]
    pub receiver: Option<Receiver>,
}

/// A sender in the room's frame: a capsule from `from` to `to` (a sphere where they meet), whether it is the figure's own,
/// and a key naming it from step to step.
#[derive(Clone, Debug)]
pub struct Sender {
    pub from: Vec3,
    pub to: Vec3,
    pub radius: f32,
    pub tags: Vec<String>,
    pub own: bool,
    pub key: usize,
}

impl Sender {
    fn tagged(&self, tags: &[String]) -> bool {
        self.tags.iter().any(|tag| tags.iter().any(|wanted| wanted == tag))
    }
}

/// The tags of the wearer's controllers: each a hand, and the right one's fingertip an index finger.
pub const HAND: [&[&str]; 2] = [&["Hand", "HandL"], &["Hand", "HandR"]];
pub const FINGER: &[&str] = &["Finger", "FingerR", "FingerIndex", "FingerIndexR"];

/// A sender of the wearer's: a controller's capsule or the fingertip, tagged `tags`.
pub fn worn(key: usize, from: Vec3, to: Vec3, radius: f32, tags: &[&str]) -> Sender {
    Sender { from, to, radius, tags: tags.iter().map(|tag| tag.to_string()).collect(), own: false, key }
}

/// A contact's shape placed by its node's `world` (the room's frame): its ends and radius.
fn placed(contact: &Contact, world: Mat4) -> (Vec3, Vec3, f32) {
    let from = world.transform_point3(Vec3::from(contact.shape.offset));
    let to = contact.shape.tail.map_or(from, |tail| world.transform_point3(Vec3::from(tail)));
    (from, to, contact.shape.radius * world.x_axis.truncate().length())
}

/// The closest points of segments `a0`–`a1` and `b0`–`b1`.
fn closest(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> (Vec3, Vec3) {
    let (d1, d2, r) = (a1 - a0, b1 - b0, a0 - b0);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    if a <= f32::EPSILON && e <= f32::EPSILON {
        return (a0, b0);
    }
    let (s, t) = if a <= f32::EPSILON {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e <= f32::EPSILON {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let denominator = a * e - b * b;
            let mut s = if denominator > f32::EPSILON { ((b * f - c * e) / denominator).clamp(0.0, 1.0) } else { 0.0 };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    (a0 + d1 * s, b0 + d2 * t)
}

/// What the receivers touched a step ago: each receiver's senders, and each sender's place, by its key.
#[derive(Default)]
pub struct Touch {
    touching: Vec<HashSet<usize>>,
    last: HashMap<usize, Vec3>,
}

impl Touch {
    /// The figure's own senders that `values` switch on, VRChat's built-in ones on its bones among them, placed by `worlds`
    /// in the room's frame, each keyed by its place in the file.
    pub fn own(expressions: &Expressions, values: &[f32], worlds: &[Mat4]) -> Vec<Sender> {
        expressions.contacts.iter().enumerate().filter(|(_, contact)| contact.receiver.is_none() && !off(&contact.when, values)).map(|(key, contact)| {
            let (from, to, radius) = placed(contact, worlds[contact.node]);
            Sender { from, to, radius, tags: contact.tags.clone(), own: true, key }
        }).collect()
    }

    /// Sets each receiver's parameter on `controller` from the `senders` touching it, the figure's nodes placed by `worlds`
    /// in the senders' frame; `delta` the time since the last.
    pub fn sense(&mut self, expressions: &Expressions, values: &[f32], worlds: &[Mat4], senders: &[Sender], controller: &mut Controller, delta: f32) {
        let receivers: Vec<(&Contact, &Receiver)> = expressions.contacts.iter().filter_map(|contact| contact.receiver.as_ref().map(|receiver| (contact, receiver))).collect();
        self.touching.resize_with(receivers.len(), HashSet::new);
        for (at, (contact, receiver)) in receivers.into_iter().enumerate() {
            let mut now = HashSet::new();
            let mut value: f32 = 0.0;
            if !off(&contact.when, values) {
                let (from, to, radius) = placed(contact, worlds[contact.node]);
                for sender in senders {
                    if !(if sender.own { receiver.own } else { receiver.others }) || !sender.tagged(&contact.tags) {
                        continue;
                    }
                    let (a, b) = closest(from, to, sender.from, sender.to);
                    let gap = a.distance(b) - sender.radius;
                    if gap > radius {
                        continue;
                    }
                    now.insert(sender.key);
                    value = value.max(match receiver.kind {
                        Kind::Constant => 1.0,
                        Kind::Proximity if radius > 0.0 => 1.0 - gap.max(0.0) / radius,
                        Kind::Proximity => 1.0,
                        Kind::OnEnter => {
                            let fast = self.last.get(&sender.key).is_none_or(|was| delta <= 0.0 || sender.from.distance(*was) / delta >= receiver.min_velocity);
                            (!self.touching[at].contains(&sender.key) && fast) as u8 as f32
                        }
                    });
                }
            }
            self.touching[at] = now;
            let value = match expressions.parameters.iter().find(|parameter| parameter.name == receiver.parameter).map(|parameter| parameter.kind) {
                Some(ValueType::Float) => value,
                Some(ValueType::Int) => value.trunc(),
                Some(ValueType::Bool) => (value != 0.0) as u8 as f32,
                None => continue,
            };
            controller.set(&receiver.parameter, value);
        }
        self.last = senders.iter().map(|sender| (sender.key, sender.from)).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::expressions::Configuration;

    fn menu() -> Expressions {
        let receiver = |kind: &str, parameter: &str, own: bool| serde_json::json!({"node": 0, "shape": {"offset": [0, 0, 0], "radius": 0.2}, "tags": ["Hand"], "receiver": {"type": kind, "parameter": parameter, "self": own, "others": !own, "local": false, "minVelocity": 1.0}});
        Expressions::parse(&serde_json::json!({
            "menu": [], "layers": [], "clips": [],
            "parameters": [{"name": "Near", "type": "float"}, {"name": "On", "type": "bool"}, {"name": "Tap", "type": "bool"}, {"name": "Mine", "type": "bool"}, {"name": "Wet", "type": "float"}],
            "targets": [{"effect": {"parameter": "Wet"}}],
            "contacts": [receiver("proximity", "Near", false), receiver("constant", "On", false), receiver("onEnter", "Tap", false), receiver("constant", "Mine", true),
                {"node": 0, "shape": {"offset": [0, 0, 0.1], "radius": 0.05}, "tags": ["Hand"]}]
        }))
        .unwrap()
    }

    #[test]
    fn receivers_write_their_parameters_from_the_hands_touching_them() {
        let menu = menu();
        let mut controller = Controller::new(&menu, &Configuration::default());
        let mut touch = Touch::default();
        let worlds = [Mat4::from_translation(Vec3::new(0.0, 1.5, 0.0))];
        let head = Vec3::new(0.0, 1.5, 0.0);
        let mut feel = |at: Vec3, tags: &[&str], delta: f32, controller: &mut Controller| touch.sense(&menu, &[0.0], &worlds, &[worn(1000, at, at, 0.05, tags)], controller, delta);
        feel(head + Vec3::Z * 0.35, HAND[0], 1.0 / 60.0, &mut controller);
        assert_eq!((controller.get("Near"), controller.get("On")), (0.0, 0.0));
        feel(head + Vec3::Z * 0.15, HAND[0], 1.0 / 60.0, &mut controller);
        assert!((controller.get("Near") - 0.5).abs() < 1e-5 && controller.get("On") == 1.0 && controller.get("Tap") == 1.0);
        feel(head + Vec3::Z * 0.15, HAND[0], 1.0 / 60.0, &mut controller);
        assert_eq!((controller.get("Tap"), controller.get("On")), (0.0, 1.0), "the tap lasts one step");
        feel(head + Vec3::Z * 0.5, HAND[0], 1.0 / 60.0, &mut controller);
        feel(head + Vec3::Z * 0.24, HAND[0], 1.0, &mut controller);
        assert_eq!(controller.get("Tap"), 0.0, "an entry slower than its minimum velocity does not tap");
        feel(head, &["Foot"], 1.0 / 60.0, &mut controller);
        assert_eq!(controller.get("On"), 0.0, "an untagged sender touches nothing");
        let own = Touch::own(&menu, &[0.0], &worlds);
        assert_eq!(own.len(), 1);
        assert!(own[0].own && own[0].from.distance(head + Vec3::Z * 0.1) < 1e-5);
        touch.sense(&menu, &[0.0], &worlds, &own, &mut controller, 1.0 / 60.0);
        assert_eq!((controller.get("Mine"), controller.get("On")), (1.0, 0.0), "its own sender reaches only the receiver allowing self");
    }

    #[test]
    fn a_clip_writes_its_parameter_past_the_menus_range() {
        let menu = Expressions::parse(&serde_json::json!({
            "menu": [], "parameters": [{"name": "Wet", "type": "float"}],
            "layers": [{"states": [{"motion": {"clip": 0}}]}],
            "clips": [{"length": 1, "curves": [{"target": 0, "keys": [[0, 0], [1, 2000]]}]}],
            "targets": [{"effect": {"parameter": "Wet"}}]
        }))
        .unwrap();
        let mut controller = Controller::new(&menu, &Configuration::default());
        controller.step(&menu, 0.5);
        controller.set("Wet", 0.0);
        controller.step(&menu, 0.0);
        assert!((controller.get("Wet") - 1000.0).abs() < 1.0, "{}", controller.get("Wet"));
    }

    #[test]
    fn closest_points_hold_for_parallel_and_degenerate_segments() {
        let (a, b) = closest(Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Y + Vec3::X);
        assert!((a.distance(b) - 1.0).abs() < 1e-6);
        let (a, b) = closest(Vec3::ZERO, Vec3::ZERO, Vec3::new(-1.0, 2.0, 0.0), Vec3::new(1.0, 2.0, 0.0));
        assert!(a == Vec3::ZERO && b.distance(Vec3::new(0.0, 2.0, 0.0)) < 1e-6);
        let (a, b) = closest(Vec3::ZERO, Vec3::Z, Vec3::new(0.0, 3.0, 0.5), Vec3::new(0.0, 3.0, 0.5));
        assert!(a.distance(Vec3::Z * 0.5) < 1e-6 && b.distance(Vec3::new(0.0, 3.0, 0.5)) < 1e-6);
    }

    #[test]
    fn a_hand_resting_inside_does_not_tap_again_when_another_sender_comes_and_goes() {
        let menu = menu();
        let mut controller = Controller::new(&menu, &Configuration::default());
        let mut touch = Touch::default();
        let worlds = [Mat4::from_translation(Vec3::new(0.0, 1.5, 0.0))];
        let inside = Vec3::new(0.0, 1.5, 0.15);
        let hand = worn(1000, inside, inside, 0.05, HAND[0]);
        let far = worn(1001, Vec3::splat(9.0), Vec3::splat(9.0), 0.05, HAND[1]);
        touch.sense(&menu, &[0.0], &worlds, &[far.clone(), hand.clone()], &mut controller, 1.0 / 60.0);
        touch.sense(&menu, &[0.0], &worlds, &[hand.clone()], &mut controller, 1.0 / 60.0);
        assert_eq!(controller.get("Tap"), 0.0);
    }
}
