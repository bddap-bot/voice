use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::conversation::Trace;

/// The page's wait for an ignored turn before the page itself sends it to the hub.
const UNANSWERED: Duration = Duration::from_secs(3);
/// The page's wait for the model to start speaking a hub reply before the next one goes out.
const TURN_START: Duration = Duration::from_secs(20);
/// The page's quiet after a spoken hub reply before the next reply or standing instructions.
pub const TURN_QUIET: Duration = Duration::from_millis(1500);
const FAILED: &str = "The hub request failed.";
const ASLEEP: &str = "Live is asleep";
const NO_DISPLAY: &str = "the VR overlay shows no display; it stays on the page";

/// What the hub state asks of the session.
#[derive(Debug, PartialEq)]
pub enum Out {
    Frame(&'static str, Value),
    Tell(Value),
    ListenForQuiet,
}

#[derive(Deserialize, Debug)]
struct Reply {
    id: String,
    stamp: String,
    #[serde(default)]
    first: bool,
    commentary: Vec<String>,
    instructions: Vec<String>,
    #[serde(default)]
    display: Option<Value>,
}

struct Turn {
    started: bool,
    deadline: Instant,
}

/// The page's delegation and hub-reply handling for one session: `delegate`, `hub`, `hub-error` and the order replies and standing instructions reach Live.
#[derive(Default)]
pub struct Hub {
    out: Vec<Out>,
    waiting: HashSet<String>,
    /// Delegations Live itself created this session; a reply to one carries its id.
    owned: HashSet<String>,
    replies: VecDeque<Reply>,
    turn: Option<Turn>,
    unanswered: Option<Instant>,
    pending_standing: Option<Vec<String>>,
    standing: Option<Vec<String>>,
    reply_unspoken: bool,
    settling: bool,
    over_context: bool,
    sleeping: bool,
}

fn has_word(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

fn stamp() -> String {
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

fn tell(kind: &str, event_id: String, delegation_id: Option<&str>, content: &str) -> Out {
    Out::Tell(json!({ "type": kind, "event_id": event_id, "delegation_id": delegation_id, "content": content }))
}

fn part_id(prefix: &str, stamp: &str, index: usize) -> String {
    if index == 0 {
        format!("{prefix}_{stamp}")
    } else {
        format!("{prefix}_{stamp}_{index}")
    }
}

impl Hub {
    pub fn drain(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    /// The model wants something the hub must answer.
    pub fn delegated(&mut self, id: Option<&str>, trace: &mut Trace, now: Instant) {
        let Some(id) = id else { return };
        self.owned.insert(id.to_owned());
        self.hand_off(id, trace, now);
    }

    fn hand_off(&mut self, id: &str, trace: &mut Trace, now: Instant) {
        self.unanswered = None;
        let delegation = trace.delegated(now);
        self.waiting.insert(id.to_owned());
        eprintln!("delegating {id}: {:?}", delegation.text);
        if delegation.text.is_empty() {
            return self.failed(id, "no transcript before the delegation");
        }
        self.out.push(Out::Frame("delegate", json!({ "id": id, "text": delegation.text, "context": delegation.context, "duration_ms": delegation.duration_ms })));
    }

    pub fn failed(&mut self, id: &str, message: &str) {
        if !self.waiting.remove(id) {
            return;
        }
        eprintln!("hub request {id} failed: {message}");
        let owned = self.owned.contains(id).then_some(id);
        self.out.push(tell("session.commentary.append", format!("hub_error_{id}"), owned, FAILED));
    }

    pub fn heard(&mut self, delta: &str, now: Instant) {
        if self.turn.as_ref().is_some_and(|turn| turn.started) {
            self.interrupt("barge-in");
        }
        if has_word(delta) {
            self.unanswered = (!self.waiting.is_empty() && !self.sleeping).then_some(now + UNANSWERED);
        }
    }

    pub fn spoke(&mut self) {
        self.unanswered = None;
        if let Some(turn) = &mut self.turn {
            if !turn.started {
                turn.started = true;
                self.out.push(Out::ListenForQuiet);
            }
        }
        self.reply_unspoken = false;
        if self.pending_standing.is_some() && !self.settling {
            self.settling = true;
            self.out.push(Out::ListenForQuiet);
        }
    }

    /// The sign-off was heard: replies from here on are not spoken and standing instructions wait for good.
    pub fn sleep(&mut self) {
        self.sleeping = true;
    }

    /// A `hub` frame's body: one JSON line, then any display image.
    pub fn reply(&mut self, body: &[u8], trace: &mut Trace, now: Instant) {
        let line = body.split(|&byte| byte == b'\n').next().unwrap_or_default();
        let reply: Reply = match serde_json::from_slice(line) {
            Ok(reply) => reply,
            Err(error) => return eprintln!("hub reply unreadable: {error}"),
        };
        eprintln!("hub reply {} ({}): {:?}{}", reply.id, reply.stamp, reply.commentary, if reply.instructions.is_empty() { "" } else { " with instructions" });
        let unspoken = if !reply.commentary.is_empty() && self.sleeping {
            Some(ASLEEP)
        } else {
            reply.display.is_some().then_some(NO_DISPLAY)
        };
        let mut ack = json!({ "id": reply.id, "stamp": reply.stamp });
        if let Some(unspoken) = unspoken {
            ack["unspoken"] = unspoken.into();
        }
        let instructions = reply.instructions.clone();
        if reply.first && self.turn.is_some() && instructions.is_empty() && self.pending_standing.is_none() {
            self.present(reply, trace);
        } else {
            self.replies.push_back(reply);
            self.play(trace, now);
        }
        if !instructions.is_empty() {
            self.stand_by(instructions);
        }
        self.out.push(Out::Frame("hub-ack", ack));
    }

    /// Appends a reply's commentary; true when Live was told something to speak.
    fn present(&mut self, reply: Reply, trace: &mut Trace) -> bool {
        if reply.commentary.is_empty() && reply.instructions.is_empty() {
            return false;
        }
        self.waiting.remove(&reply.id);
        trace.hub_replied();
        if reply.commentary.is_empty() || self.sleeping {
            return false;
        }
        let owned = self.owned.contains(&reply.id).then_some(reply.id.as_str());
        self.reply_unspoken = true;
        for (index, content) in reply.commentary.iter().enumerate() {
            self.out.push(tell("session.commentary.append", part_id("hub", &reply.stamp, index), owned, content));
        }
        true
    }

    /// The page's `playHubReplies`: one reply at a time, the next once the model has spoken this one and gone quiet.
    fn play(&mut self, trace: &mut Trace, now: Instant) {
        if self.turn.is_some() {
            return;
        }
        while let Some(reply) = self.replies.pop_front() {
            self.turn = Some(Turn { started: false, deadline: now + TURN_START });
            if self.present(reply, trace) {
                return;
            }
        }
        self.turn = None;
        self.apply_standing();
    }

    fn stand_by(&mut self, instructions: Vec<String>) {
        if self.sleeping {
            return;
        }
        self.pending_standing = Some(instructions);
        self.apply_standing();
    }

    fn apply_standing(&mut self) {
        if self.pending_standing.is_none() || self.sleeping || self.reply_unspoken || self.settling || self.turn.is_some() || !self.replies.is_empty() {
            return;
        }
        let instructions = self.pending_standing.take().unwrap();
        let stamp = stamp();
        for (index, content) in instructions.iter().enumerate() {
            self.out.push(tell("session.instructions.append", part_id("standing", &stamp, index), None, content));
        }
        eprintln!("standing instructions applied");
        self.standing = Some(instructions);
    }

    /// Past nine tenths of the context window the model may lose the standing instructions, so they go in again.
    pub fn usage(&mut self, ratio: f64) {
        let over = ratio > 0.9;
        if over && !self.over_context && self.pending_standing.is_none() {
            if let Some(standing) = self.standing.clone() {
                self.stand_by(standing);
            }
        }
        self.over_context = over;
    }

    fn interrupt(&mut self, reason: &str) {
        for reply in self.replies.drain(..) {
            eprintln!("hub reply {} ({}) dropped: {reason}", reply.id, reply.stamp);
        }
        self.turn = None;
        self.apply_standing();
    }

    /// Advances the waits: `quiet` is whether playback has been quiet for `TURN_QUIET` since the last `ListenForQuiet`.
    pub fn step(&mut self, trace: &mut Trace, now: Instant, quiet: bool) {
        if quiet && self.settling {
            self.settling = false;
            self.apply_standing();
        }
        if let Some(turn) = &self.turn {
            if if turn.started { quiet } else { now >= turn.deadline } {
                self.turn = None;
                self.play(trace, now);
            }
        }
        if self.unanswered.is_some_and(|at| now >= at) {
            self.hand_off(&format!("turn_{}", stamp()), trace, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        hub: Hub,
        trace: Trace,
        now: Instant,
        out: Vec<Out>,
    }

    impl Fixture {
        fn new() -> Fixture {
            Fixture { hub: Hub::default(), trace: Trace::default(), now: Instant::now(), out: Vec::new() }
        }
        fn collect(&mut self) {
            self.out.extend(self.hub.drain());
        }
        fn hear(&mut self, delta: &str) {
            self.trace.heard(delta, self.now);
            self.hub.heard(delta, self.now);
            self.collect();
        }
        fn speak(&mut self, delta: &str) {
            self.trace.spoke(delta);
            self.hub.spoke();
            self.collect();
        }
        fn delegate(&mut self, id: &str) {
            self.hub.delegated(Some(id), &mut self.trace, self.now);
            self.collect();
        }
        fn reply(&mut self, value: Value) {
            self.hub.reply(format!("{value}\n").as_bytes(), &mut self.trace, self.now);
            self.collect();
        }
        fn wait(&mut self, duration: Duration, quiet: bool) {
            self.now += duration;
            self.hub.step(&mut self.trace, self.now, quiet);
            self.collect();
        }
        fn frames(&self, verb: &str) -> Vec<Value> {
            self.out.iter().filter_map(|out| match out {
                Out::Frame(name, body) if *name == verb => Some(body.clone()),
                _ => None,
            }).collect()
        }
        /// (channel, delegation_id, content) of every append.
        fn told(&self) -> Vec<(String, Value, String)> {
            self.out.iter().filter_map(|out| match out {
                Out::Tell(event) => Some((event["type"].as_str().unwrap().split('.').nth(1).unwrap().to_owned(), event["delegation_id"].clone(), event["content"].as_str().unwrap().to_owned())),
                _ => None,
            }).collect()
        }
        fn event_ids(&self) -> Vec<String> {
            self.out.iter().filter_map(|out| match out {
                Out::Tell(event) => Some(event["event_id"].as_str().unwrap().to_owned()),
                _ => None,
            }).collect()
        }
    }

    fn reply(id: &str, stamp: &str, commentary: &[&str]) -> Value {
        json!({ "id": id, "stamp": stamp, "first": false, "commentary": commentary, "instructions": [], "timing_ms": 5 })
    }

    fn commentary(delegation: Value, content: &str) -> (String, Value, String) {
        ("commentary".into(), delegation, content.into())
    }

    fn instructions(content: &str) -> (String, Value, String) {
        ("instructions".into(), Value::Null, content.into())
    }

    #[test]
    fn a_delegation_sends_the_words_and_context_and_its_reply_is_commentary_with_its_id() {
        let mut f = Fixture::new();
        f.hear("Hello there.");
        f.delegate("item_first");
        f.speak("Hi.");
        f.hear("What's in the build queue right now?");
        f.delegate("item_exact");
        let frames = f.frames("delegate");
        assert_eq!(frames.iter().map(|frame| (frame["id"].clone(), frame["text"].clone(), frame["context"].clone())).collect::<Vec<_>>(), [
            (json!("item_first"), json!("Hello there."), json!([])),
            (json!("item_exact"), json!("What's in the build queue right now?"), json!([{ "speaker": "user", "text": "Hello there." }, { "speaker": "live", "text": "Hi." }])),
        ]);
        assert!(frames.iter().all(|frame| frame["duration_ms"].is_u64()));
        assert!(f.told().is_empty());
        f.reply(reply("item_exact", "mix", &["Two jobs need attention.", "Both are on the display."]));
        f.speak("Two jobs need attention.");
        f.wait(Duration::ZERO, true);
        f.reply(reply("share_1", "share_1", &["The link is the release notes."]));
        assert_eq!(f.event_ids(), ["hub_mix", "hub_mix_1", "hub_share_1"]);
        assert_eq!(f.told(), [commentary(json!("item_exact"), "Two jobs need attention."), commentary(json!("item_exact"), "Both are on the display."), commentary(Value::Null, "The link is the release notes.")]);
        assert_eq!(f.frames("hub-ack"), [json!({ "id": "item_exact", "stamp": "mix" }), json!({ "id": "share_1", "stamp": "share_1" })]);
    }

    #[test]
    fn a_display_only_push_says_nothing_and_a_display_is_acknowledged_unspoken() {
        let mut f = Fixture::new();
        f.hear("How many jobs are queued?");
        f.delegate("slow");
        f.reply(json!({ "id": "slow", "stamp": "push_1", "commentary": [], "instructions": [], "display": { "markdown": "chart" } }));
        assert!(f.told().is_empty());
        assert!(f.hub.waiting.contains("slow"));
        f.reply(json!({ "id": "slow", "stamp": "reply_1", "commentary": ["Four jobs are queued."], "instructions": [], "display": { "markdown": "chart" } }));
        assert_eq!(f.told(), [commentary(json!("slow"), "Four jobs are queued.")]);
        assert!(f.hub.waiting.is_empty());
        let acks = f.frames("hub-ack");
        assert_eq!(acks.len(), 2);
        assert!(acks.iter().all(|ack| ack["unspoken"] == NO_DISPLAY));
    }

    #[test]
    fn a_failed_request_is_told_once_and_a_delegation_with_no_words_fails() {
        let mut f = Fixture::new();
        f.hear("Is the printer busy?");
        f.delegate("lost");
        f.hub.failed("lost", "delegation queue is full");
        f.hub.failed("lost", "again");
        f.delegate("empty");
        f.collect();
        assert_eq!(f.event_ids(), ["hub_error_lost", "hub_error_empty"]);
        assert_eq!(f.told(), [commentary(json!("lost"), FAILED), commentary(json!("empty"), FAILED)]);
        assert_eq!(f.frames("delegate").len(), 1);
        f.hub.delegated(None, &mut f.trace, f.now);
        assert!(f.hub.drain().is_empty());
    }

    #[test]
    fn a_first_reply_goes_out_mid_turn_follow_ups_wait_for_speech_and_quiet_and_barge_in_drops_the_rest() {
        let mut f = Fixture::new();
        f.hear("Check the test beacon.");
        f.delegate("first");
        f.hear(" While that runs, let me tell you");
        f.reply(json!({ "id": "first", "stamp": "o1", "first": true, "commentary": ["First part.", "Second part."], "instructions": [] }));
        f.reply(json!({ "id": "first", "stamp": "o2", "commentary": ["Slide two."], "instructions": [], "display": { "markdown": "Slide two picture" } }));
        f.reply(json!({ "id": "first", "stamp": "o3", "commentary": ["Slide three."], "instructions": [] }));
        f.reply(json!({ "id": "other", "stamp": "o4", "first": true, "commentary": ["Other answer."], "instructions": [] }));
        assert_eq!(f.told(), [commentary(json!("first"), "First part."), commentary(json!("first"), "Second part."), commentary(Value::Null, "Other answer.")]);
        f.wait(Duration::from_secs(1), true);
        assert_eq!(f.told().len(), 3, "quiet before the model speaks does not end the turn");
        f.hear(" about the garden.");
        f.speak("First part.");
        assert_eq!(f.out.iter().filter(|out| **out == Out::ListenForQuiet).count(), 1);
        f.wait(Duration::from_millis(100), false);
        assert_eq!(f.told().len(), 3);
        f.wait(Duration::from_millis(100), true);
        assert_eq!(f.told()[3..], [commentary(json!("first"), "Slide two.")]);
        f.speak("Slide two.");
        f.hear("Stop there.");
        f.wait(Duration::from_secs(30), true);
        assert_eq!(f.told().len(), 4, "slide three was dropped by the barge-in");
        assert_eq!(f.frames("hub-ack").iter().map(|ack| ack["stamp"].as_str().unwrap().to_owned()).collect::<Vec<_>>(), ["o1", "o2", "o3", "o4"]);
    }

    #[test]
    fn a_reply_the_model_never_speaks_releases_the_queue_after_the_turn_timeout() {
        let mut f = Fixture::new();
        f.reply(reply("a", "a", &["Reply A."]));
        f.reply(reply("b", "b", &["Reply B."]));
        f.wait(TURN_START - Duration::from_millis(1), false);
        assert_eq!(f.told().len(), 1);
        f.wait(Duration::from_millis(1), false);
        assert_eq!(f.told().len(), 2);
    }

    #[test]
    fn standing_instructions_follow_the_spoken_reply_beside_them_an_instructions_only_reply_is_silent_and_a_rollover_reappends() {
        let mut f = Fixture::new();
        f.hear("Check the test beacon.");
        f.delegate("styled");
        f.reply(json!({ "id": "styled", "stamp": "s1", "first": true, "commentary": ["The beacon is violet."], "instructions": ["Hum while waiting.", "Keep everything else unchanged."] }));
        assert_eq!(f.told(), [commentary(json!("styled"), "The beacon is violet.")]);
        f.speak("The beacon is violet.");
        f.wait(Duration::from_millis(10), false);
        assert_eq!(f.told().len(), 1);
        f.wait(Duration::from_millis(10), true);
        assert_eq!(f.told()[1..], [instructions("Hum while waiting."), instructions("Keep everything else unchanged.")]);
        f.hear("Check the printer.");
        f.delegate("silent");
        f.reply(json!({ "id": "silent", "stamp": "s2", "first": true, "commentary": [], "instructions": ["Answer in one short sentence."] }));
        assert_eq!(f.told()[3..], [instructions("Answer in one short sentence.")]);
        assert!(f.hub.waiting.is_empty());
        for ratio in [0.5, 0.95, 0.96] {
            f.hub.usage(ratio);
        }
        f.collect();
        assert_eq!(f.told()[4..], [instructions("Answer in one short sentence.")]);
        assert_eq!(f.frames("hub-ack"), [json!({ "id": "styled", "stamp": "s1" }), json!({ "id": "silent", "stamp": "s2" })]);
    }

    #[test]
    fn pending_instructions_wait_for_every_earlier_reply_and_hold_back_a_first_reply_that_would_jump_the_queue() {
        let mut f = Fixture::new();
        f.reply(json!({ "id": "early", "stamp": "early", "commentary": [], "instructions": ["Old direction."] }));
        f.reply(json!({ "id": "a", "stamp": "a", "commentary": ["Reply A."], "instructions": ["New direction."] }));
        f.reply(json!({ "id": "b", "stamp": "b", "first": true, "commentary": ["Reply B."], "instructions": [] }));
        f.hub.usage(0.95);
        f.collect();
        let early = [instructions("Old direction."), commentary(Value::Null, "Reply A.")];
        assert_eq!(f.told(), early);
        f.speak("Reply A.");
        f.wait(Duration::ZERO, true);
        assert_eq!(f.told()[2..], [commentary(Value::Null, "Reply B.")]);
        f.speak("Reply B.");
        f.wait(Duration::ZERO, true);
        assert_eq!(f.told()[3..], [instructions("New direction.")]);
    }

    #[test]
    fn instructions_wait_past_the_turn_timeout_until_the_model_speaks_the_reply() {
        let mut f = Fixture::new();
        f.reply(json!({ "id": "late", "stamp": "late", "commentary": ["Reply."], "instructions": ["Direction."] }));
        f.wait(TURN_START + Duration::from_secs(1), true);
        assert_eq!(f.told(), [commentary(Value::Null, "Reply.")]);
        f.speak("Reply.");
        f.wait(Duration::ZERO, true);
        assert_eq!(f.told()[1..], [instructions("Direction.")]);
    }

    #[test]
    fn after_the_sign_off_a_reply_is_acknowledged_unspoken_and_nothing_is_told() {
        let mut f = Fixture::new();
        f.hub.sleep();
        f.reply(json!({ "id": "late", "stamp": "late_stamp", "first": true, "commentary": ["Too late."], "instructions": ["Ignored."] }));
        assert!(f.told().is_empty());
        assert_eq!(f.frames("hub-ack"), [json!({ "id": "late", "stamp": "late_stamp", "unspoken": ASLEEP })]);
    }

    #[test]
    fn a_turn_the_model_ignores_while_a_delegation_is_pending_goes_to_the_hub() {
        let mut f = Fixture::new();
        f.hear("Hello there.");
        f.wait(Duration::from_millis(3100), false);
        assert_eq!(f.frames("delegate").len(), 0, "nothing pending, nothing forwarded");
        f.delegate("item_first");
        f.hear(" How are you?");
        f.wait(Duration::from_secs(1), false);
        f.speak("Fine.");
        f.wait(Duration::from_millis(2500), false);
        assert_eq!(f.frames("delegate").len(), 1);
        f.hear(" Also, is the printer");
        f.wait(Duration::from_secs(2), false);
        f.hear(" busy right now?");
        f.wait(Duration::from_millis(2900), false);
        assert_eq!(f.frames("delegate").len(), 1);
        f.wait(Duration::from_millis(200), false);
        let frames = f.frames("delegate");
        assert_eq!(frames.len(), 2);
        let printer = frames[1]["id"].as_str().unwrap().to_owned();
        assert!(printer.starts_with("turn_"));
        assert_eq!(frames[1]["text"], "How are you?\nAlso, is the printer busy right now?");
        f.hear(" And the backup?");
        f.wait(Duration::from_secs(1), false);
        f.delegate("item_backup");
        f.hear("?");
        f.wait(Duration::from_millis(3100), false);
        assert_eq!(f.frames("delegate").len(), 3, "punctuation alone forwards nothing");
        f.reply(reply(&printer, "printer", &["The printer is idle."]));
        assert_eq!(f.told(), [commentary(Value::Null, "The printer is idle.")]);
    }

    /// The page's recorded delegated-reply sessions, replayed: each delegation reaches the hub as the words heard before it, and each first reply reaches Live as it arrives, mid-utterance, under the delegation's id.
    #[test]
    fn the_recorded_delegated_reply_sessions_replay_against_the_native_client() {
        let capture: Value = serde_json::from_str(include_str!("../../test/fixtures/delegated-reply-capture.json")).unwrap();
        let replies = capture["replies"].as_array().unwrap();
        let rows = capture["capture"]["rt"].as_array().unwrap();
        for (session, expected) in replies.iter().enumerate() {
            let start = Instant::now();
            let mut f = Fixture::new();
            let mut delegations = Vec::new();
            for row in rows.iter().filter(|row| row["ch"] == session) {
                f.now = start + Duration::from_millis(row["at"].as_u64().unwrap());
                f.wait(Duration::ZERO, false);
                let event = &row["event"];
                match (row["dir"].as_str().unwrap(), event["type"].as_str().unwrap_or("")) {
                    ("in", "session.input_transcript.delta") => f.hear(event["delta"].as_str().unwrap()),
                    ("in", "session.output_transcript.delta") => f.speak(event["delta"].as_str().unwrap()),
                    ("in", "session.delegation.created") => {
                        let id = format!("recorded_{session}_{}", delegations.len());
                        f.delegate(&id);
                        delegations.push(id);
                    }
                    ("out", "session.commentary.append") if event["event_id"].as_str().unwrap().starts_with("hub_") => {
                        let stamp = event["event_id"].as_str().unwrap().strip_prefix("hub_").unwrap();
                        let before = f.told().len();
                        f.reply(json!({ "id": delegations[0], "stamp": stamp, "first": true, "commentary": [event["content"]], "instructions": [], "timing_ms": 1 }));
                        assert_eq!(f.told()[before..], [commentary(json!(delegations[0]), expected.as_str().unwrap())], "session {session}");
                        assert_eq!(f.event_ids().last().unwrap(), event["event_id"].as_str().unwrap());
                    }
                    _ => {}
                }
            }
            assert_eq!(delegations.len(), 1, "session {session}");
            let delegate = f.frames("delegate");
            assert_eq!(delegate.len(), 1, "session {session}");
            assert_eq!(delegate[0]["id"], delegations[0].as_str());
            let heard: String = rows.iter().filter(|row| row["ch"] == session && row["event"]["type"] == "session.input_transcript.delta" && row["at"].as_u64() < rows.iter().find(|row| row["ch"] == session && row["event"]["type"] == "session.delegation.created").unwrap()["at"].as_u64()).map(|row| row["event"]["delta"].as_str().unwrap()).collect();
            assert_eq!(delegate[0]["text"], heard.trim(), "session {session}");
            assert_eq!(f.frames("hub-ack").len(), 1, "session {session}");
            assert!(f.hub.waiting.is_empty(), "session {session}");
        }
    }
}
