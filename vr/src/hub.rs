use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::conversation::Trace;
use crate::turn::{hold, judge_quiet};

/// The page's wait for an ignored turn before the page itself sends it to the hub.
const UNANSWERED: Duration = Duration::from_secs(3);
/// The page's wait for the model to start speaking a hub reply before the next one goes out.
const TURN_START: Duration = Duration::from_secs(20);
/// The page's quiet after a spoken hub reply before the next reply or standing instructions.
pub const TURN_QUIET: Duration = Duration::from_millis(1500);
const FAILED: &str = "The hub request failed.";
const ASLEEP: &str = "Live is asleep";
/// Audio seconds after which the standing instructions go in again, so a long session does not lose them.
const STANDING_REFRESH_S: f64 = 600.0;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Channel {
    Speakable,
    Commentary,
    Developer,
}

impl Channel {
    fn name(self) -> &'static str {
        match self {
            Channel::Speakable => "speakable",
            Channel::Commentary => "commentary",
            Channel::Developer => "developer",
        }
    }
}

/// Every client event the overlay sends; the realtime server closes the call on one it rejects, so nothing else is constructible.
#[derive(Debug, PartialEq)]
pub enum Tell {
    Session(Channel, String),
    Delegation(String, String),
}

impl Tell {
    pub fn event(&self) -> Value {
        match self {
            Tell::Session(channel, text) => json!({ "type": "session.context.append", "channel": channel.name(), "content": [{ "type": "input_text", "text": text }] }),
            Tell::Delegation(id, text) => json!({ "type": "delegation.context.append", "delegation_item_id": id, "channel": "speakable", "content": [{ "type": "input_text", "text": text }] }),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Frame(&'static str, Value),
    Tell(Tell),
}

#[derive(Deserialize, Debug)]
struct Reply {
    id: String,
    stamp: String,
    #[serde(default)]
    first: bool,
    commentary: Vec<String>,
    instructions: Vec<String>,
    display: Option<Shown>,
    #[serde(skip)]
    shown: Option<Display>,
}

#[derive(Deserialize, Debug)]
struct Shown {
    markdown: Option<String>,
    link: Option<String>,
    image: Option<serde::de::IgnoredAny>,
    object: Option<serde::de::IgnoredAny>,
    point: Option<String>,
    data: Option<String>,
    hold: Option<Value>,
    #[serde(default)]
    clear: bool,
    html: Option<serde::de::IgnoredAny>,
}

/// What a hub reply asks to show, as the page's display card does.
#[derive(Debug, Default, PartialEq)]
pub struct Display {
    pub markdown: Option<String>,
    pub link: Option<String>,
    /// The encoded PNG, JPEG, GIF or WebP.
    pub image: Option<Vec<u8>>,
    /// A binary glTF 2 scene, shown beside those already shown; a reply carries an image or an object, never both, so either is the whole trailer.
    pub object: Option<Vec<u8>>,
    /// A node in a shown object for the puppet to point at while it speaks: `<node>`, or `<place>:<node>` to choose among objects that share a name.
    pub point: Option<String>,
    pub data: Option<String>,
    pub hold: Option<Hold>,
    /// Removes every shown object first.
    pub clear: bool,
    /// An HTML document, which only the page can run.
    pub html: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hold {
    At(f32),
    Run,
}

impl Hold {
    fn read(value: &Value) -> Option<Hold> {
        match value {
            Value::Bool(false) => Some(Hold::Run),
            _ => value.as_f64().map(|seconds| seconds as f32).filter(|seconds| seconds.is_finite() && *seconds >= 0.0).map(Hold::At),
        }
    }
}

impl Display {
    /// Whether it only speaks to the shown object, leaving the panel and the object as they are.
    pub fn addresses_object(&self) -> bool {
        self.markdown.is_none() && self.link.is_none() && self.image.is_none() && self.object.is_none() && !self.html
    }

    /// Whether it carries a card for the panel.
    pub fn card(&self) -> bool {
        self.markdown.is_some() || self.link.is_some() || self.image.is_some() || self.html
    }
}

enum Turn {
    /// The model has until then to start speaking the reply.
    Awaiting(Instant),
    Speaking,
}

/// Standing instructions not yet appended; `Settling` waits for quiet after the model spoke.
#[derive(Default)]
enum Standing {
    #[default]
    None,
    Waiting(Vec<String>),
    Settling(Vec<String>),
}

/// The page's delegation and hub-reply handling for one session: `delegate`, `hub`, `hub-error` and the order replies and standing instructions reach Live.
#[derive(Default)]
pub struct Hub {
    out: Vec<Command>,
    listen: bool,
    waiting: HashSet<String>,
    /// Delegations Live itself created this session; a reply to one carries its id.
    owned: HashSet<String>,
    replies: VecDeque<Reply>,
    turn: Option<Turn>,
    unanswered: Option<Instant>,
    request_end: Option<Instant>,
    pending_standing: Standing,
    standing: Vec<String>,
    reply_unspoken: bool,
    audio_seconds: f64,
    standing_at: f64,
    sleeping: bool,
    /// A presented reply's display, shown once the model starts speaking it.
    cued: Option<Display>,
    display: Option<Display>,
    held: Option<Held>,
    heard_at: Option<Instant>,
    ask: Option<u64>,
}

/// A hand-off waiting for the speech to finish its thought.
struct Held {
    id: String,
    judging: Option<u64>,
    judge_at: Option<Instant>,
    until: Instant,
}

fn has_word(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

fn stamp() -> String {
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

fn speak(delegation: Option<&str>, content: &str) -> Command {
    Command::Tell(match delegation {
        Some(id) => Tell::Delegation(id.to_owned(), content.to_owned()),
        None => Tell::Session(Channel::Speakable, content.to_owned()),
    })
}

impl Hub {
    pub fn drain(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.out)
    }

    /// Whether a wait for quiet started since the last call; the caller restarts playback's silence count, as the page's `quiet()` does.
    pub fn take_listen(&mut self) -> bool {
        std::mem::take(&mut self.listen)
    }

    /// The newest display a reply carried, once.
    pub fn take_display(&mut self) -> Option<Display> {
        self.display.take()
    }

    pub fn sleeping(&self) -> bool {
        self.sleeping
    }

    pub fn delegated(&mut self, id: &str, trace: &mut Trace, now: Instant) {
        self.owned.insert(id.to_owned());
        if trace.ends_request() {
            self.hand_off(id, trace, now);
        } else {
            self.due(id.to_owned(), now);
        }
    }

    /// A hand-off is due: it waits for the speech so far to be judged, and a later one joins a hand-off already held, under Live's newest id.
    fn due(&mut self, id: String, now: Instant) {
        self.unanswered = None;
        match &mut self.held {
            Some(held) => {
                if self.owned.contains(&id) {
                    held.id = id;
                }
            }
            None => {
                self.held = Some(Held { id, judging: None, judge_at: Some(now), until: self.heard_at.unwrap_or(now) + hold() });
                self.judge(now);
            }
        }
    }

    fn judge(&mut self, now: Instant) {
        static ASKS: AtomicU64 = AtomicU64::new(0);
        let Some(held) = &mut self.held else { return };
        if held.judging.is_none() && held.judge_at.is_some_and(|at| now >= at) {
            let number = ASKS.fetch_add(1, Ordering::Relaxed);
            held.judging = Some(number);
            held.judge_at = None;
            self.ask = Some(number);
        }
    }

    /// The number of a judgement to make now of the speech Live heard, once.
    pub fn take_ask(&mut self) -> Option<u64> {
        self.ask.take()
    }

    /// A judgement of the held hand-off's speech: a finished thought goes now, an unfinished one waits for more speech or `hold()` of quiet.
    pub fn judged(&mut self, number: u64, complete: bool, trace: &mut Trace, now: Instant) {
        let Some(held) = self.held.as_mut().filter(|held| held.judging == Some(number)) else { return };
        held.judging = None;
        if complete {
            let id = held.id.clone();
            self.hand_off(&id, trace, now);
        } else {
            tracing::info!("holding {}: the thought sounds unfinished", held.id);
        }
    }

    fn hand_off(&mut self, id: &str, trace: &mut Trace, now: Instant) {
        self.held = None;
        self.unanswered = None;
        self.request_end = None;
        let delegation = trace.delegated(now);
        trace.calls.asked(id, &delegation.text);
        self.waiting.insert(id.to_owned());
        tracing::info!("delegating {id}: {:?}", delegation.text);
        if delegation.text.is_empty() {
            return self.failed(id, "no transcript before the delegation", trace);
        }
        self.out.push(Command::Frame("delegate", json!({ "id": id, "text": delegation.text, "context": delegation.context, "duration_ms": delegation.duration_ms })));
    }

    pub fn failed(&mut self, id: &str, message: &str, trace: &mut Trace) {
        if !self.waiting.remove(id) {
            return;
        }
        trace.calls.failed(id);
        tracing::warn!("hub request {id} failed: {message}");
        let owned = self.owned.contains(id).then_some(id);
        self.out.push(speak(owned, FAILED));
    }

    pub fn heard(&mut self, delta: &str, trace: &Trace, now: Instant) {
        if matches!(self.turn, Some(Turn::Speaking)) {
            self.interrupt("barge-in");
        }
        if has_word(delta) {
            self.unanswered = (!self.waiting.is_empty() && !self.sleeping).then_some(now + UNANSWERED);
            self.heard_at = Some(now);
            if let Some(held) = &mut self.held {
                held.judging = None;
                held.judge_at = Some(now + judge_quiet());
                held.until = now + hold();
            }
        }
        self.request_end = (trace.ends_request() && !self.sleeping).then_some(now + UNANSWERED);
    }

    pub fn spoke(&mut self) {
        self.unanswered = None;
        self.show_cued();
        if matches!(self.turn, Some(Turn::Awaiting(_))) {
            self.turn = Some(Turn::Speaking);
            self.listen = true;
        }
        self.reply_unspoken = false;
        if let Standing::Waiting(instructions) = &mut self.pending_standing {
            let instructions = std::mem::take(instructions);
            self.pending_standing = Standing::Settling(instructions);
            self.listen = true;
        }
    }

    /// The sign-off was heard: replies from here on are not spoken and standing instructions wait for good.
    pub fn sleep(&mut self) {
        self.sleeping = true;
        self.held = None;
    }

    /// The session is over: words heard since the last delegation go to the hub as one `ended` frame, which no voice answers.
    pub fn ended(&mut self, trace: &Trace, now: Instant) {
        let unrelayed = trace.pending(now);
        if has_word(&unrelayed.text) {
            self.out.push(Command::Frame("ended", json!({ "text": unrelayed.text, "context": unrelayed.context, "duration_ms": unrelayed.duration_ms })));
        }
    }

    /// A `hub` frame's body: one JSON line, then any display image.
    pub fn reply(&mut self, body: &[u8], trace: &mut Trace, now: Instant) {
        let (line, image) = match body.iter().position(|&byte| byte == b'\n') {
            Some(end) => (&body[..end], &body[end + 1..]),
            None => (body, &[][..]),
        };
        let mut reply: Reply = match serde_json::from_slice(line) {
            Ok(reply) => reply,
            Err(error) => return tracing::warn!("hub reply unreadable: {error}"),
        };
        tracing::info!("hub reply {} ({}): {:?}{}", reply.id, reply.stamp, reply.commentary, if reply.instructions.is_empty() { "" } else { " with instructions" });
        trace.calls.replied(&reply.id, &reply.commentary);
        if let Some(shown) = reply.display.take() {
            reply.shown = Some(if shown.html.is_some() {
                Display { html: true, ..Display::default() }
            } else {
                let trailer = |carried: bool| (carried && !image.is_empty()).then(|| image.to_vec());
                Display { markdown: shown.markdown, link: shown.link, image: trailer(shown.image.is_some()), object: trailer(shown.object.is_some() && shown.image.is_none()), point: shown.point, data: shown.data, hold: shown.hold.as_ref().and_then(Hold::read), clear: shown.clear, html: false }
            });
        }
        let unspoken = (!reply.commentary.is_empty() && self.sleeping).then_some(ASLEEP);
        let mut ack = json!({ "id": reply.id, "stamp": reply.stamp });
        if let Some(unspoken) = unspoken {
            ack["unspoken"] = unspoken.into();
        }
        let instructions = reply.instructions.clone();
        if reply.first && self.turn.is_some() && instructions.is_empty() && matches!(self.pending_standing, Standing::None) {
            self.present(reply, trace);
        } else {
            self.replies.push_back(reply);
            self.play(trace, now);
        }
        if !instructions.is_empty() {
            self.stand_by(instructions);
        }
        self.out.push(Command::Frame("hub-ack", ack));
    }

    fn show_cued(&mut self) {
        if let Some(display) = self.cued.take() {
            self.display = Some(display);
        }
    }

    /// Appends a reply's commentary and cues its display for when the model speaks it; true when Live was told something to speak.
    fn present(&mut self, mut reply: Reply, trace: &mut Trace) -> bool {
        let spoken = !reply.commentary.is_empty() && !self.sleeping;
        if let Some(display) = reply.shown.take() {
            if spoken {
                self.cued = Some(display);
            } else {
                self.display = Some(display);
            }
        }
        if reply.commentary.is_empty() && reply.instructions.is_empty() {
            return false;
        }
        self.waiting.remove(&reply.id);
        trace.hub_replied();
        if !spoken {
            return false;
        }
        let owned = self.owned.contains(&reply.id).then_some(reply.id.as_str());
        self.reply_unspoken = true;
        for content in &reply.commentary {
            self.out.push(speak(owned, content));
        }
        true
    }

    /// The page's `playHubReplies`: one reply at a time, the next once the model has spoken this one and gone quiet.
    fn play(&mut self, trace: &mut Trace, now: Instant) {
        if self.turn.is_some() {
            return;
        }
        while let Some(reply) = self.replies.pop_front() {
            self.turn = Some(Turn::Awaiting(now + TURN_START));
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
        self.pending_standing = match self.pending_standing {
            Standing::Settling(_) => Standing::Settling(instructions),
            _ => Standing::Waiting(instructions),
        };
        self.apply_standing();
    }

    fn apply_standing(&mut self) {
        if self.sleeping || self.reply_unspoken || self.turn.is_some() || !self.replies.is_empty() {
            return;
        }
        let Standing::Waiting(instructions) = &mut self.pending_standing else { return };
        let instructions = std::mem::take(instructions);
        self.pending_standing = Standing::None;
        for content in &instructions {
            self.out.push(Command::Tell(Tell::Session(Channel::Developer, content.clone())));
        }
        tracing::info!("standing instructions applied");
        self.standing = instructions;
        self.standing_at = self.audio_seconds;
    }

    pub fn usage(&mut self, audio_seconds: f64) {
        self.audio_seconds = audio_seconds;
        if audio_seconds - self.standing_at >= STANDING_REFRESH_S && matches!(self.pending_standing, Standing::None) && !self.standing.is_empty() {
            self.stand_by(self.standing.clone());
        }
    }

    fn interrupt(&mut self, reason: &str) {
        for reply in self.replies.drain(..) {
            tracing::warn!("hub reply {} ({}) dropped: {reason}", reply.id, reply.stamp);
        }
        self.turn = None;
        self.apply_standing();
    }

    /// Advances the waits: `quiet` is whether playback has been quiet for `TURN_QUIET` since the last `take_listen`.
    pub fn step(&mut self, trace: &mut Trace, now: Instant, quiet: bool) {
        if quiet {
            if let Standing::Settling(instructions) = &mut self.pending_standing {
                let instructions = std::mem::take(instructions);
                self.pending_standing = Standing::Waiting(instructions);
                self.apply_standing();
            }
        }
        if let Some(turn) = &self.turn {
            if match turn {
                Turn::Awaiting(deadline) => now >= *deadline,
                Turn::Speaking => quiet,
            } {
                self.show_cued();
                self.turn = None;
                self.play(trace, now);
            }
        }
        if self.request_end.is_some_and(|at| now >= at) {
            let id = self.held.take().map_or_else(|| format!("turn_{}", stamp()), |held| held.id);
            self.hand_off(&id, trace, now);
        }
        if self.unanswered.is_some_and(|at| now >= at) {
            self.due(format!("turn_{}", stamp()), now);
        }
        self.judge(now);
        if let Some(held) = self.held.as_ref().filter(|held| now >= held.until) {
            let id = held.id.clone();
            tracing::info!("handing off {id}: {} s of quiet after an unfinished thought", hold().as_secs());
            self.hand_off(&id, trace, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calls::State;

    struct Fixture {
        hub: Hub,
        trace: Trace,
        now: Instant,
        out: Vec<Command>,
        listens: usize,
        /// The judge's answer to every ask, or `None` to leave asks in `asks` for the test to answer.
        verdict: Option<bool>,
        asks: Vec<u64>,
    }

    impl Fixture {
        fn new() -> Fixture {
            Fixture { hub: Hub::default(), trace: Trace::default(), now: Instant::now(), out: Vec::new(), listens: 0, verdict: Some(true), asks: Vec::new() }
        }
        fn collect(&mut self) {
            if let Some(number) = self.hub.take_ask() {
                match self.verdict {
                    Some(complete) => self.hub.judged(number, complete, &mut self.trace, self.now),
                    None => self.asks.push(number),
                }
            }
            self.out.extend(self.hub.drain());
            self.listens += usize::from(self.hub.take_listen());
        }
        fn judged(&mut self, complete: bool) {
            let number = self.asks.pop().expect("an ask to answer");
            self.hub.judged(number, complete, &mut self.trace, self.now);
            self.collect();
        }
        fn hear(&mut self, delta: &str) {
            self.trace.heard(delta, self.now);
            self.hub.heard(delta, &self.trace, self.now);
            self.collect();
        }
        fn speak(&mut self, delta: &str) {
            self.trace.spoke(delta);
            self.hub.spoke();
            self.collect();
        }
        fn delegate(&mut self, id: &str) {
            self.hub.delegated(id, &mut self.trace, self.now);
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
                Command::Frame(name, body) if *name == verb => Some(body.clone()),
                _ => None,
            }).collect()
        }
        /// (channel, delegation item id, text) of every append.
        fn told(&self) -> Vec<(String, Value, String)> {
            self.out.iter().filter_map(|out| match out {
                Command::Tell(tell) => {
                    let event = tell.event();
                    Some((event["channel"].as_str().unwrap().to_owned(), event.get("delegation_item_id").cloned().unwrap_or(Value::Null), event["content"][0]["text"].as_str().unwrap().to_owned()))
                }
                _ => None,
            }).collect()
        }
    }

    fn reply(id: &str, stamp: &str, commentary: &[&str]) -> Value {
        json!({ "id": id, "stamp": stamp, "first": false, "commentary": commentary, "instructions": [], "timing_ms": 5 })
    }

    fn speakable(delegation: Value, content: &str) -> (String, Value, String) {
        ("speakable".into(), delegation, content.into())
    }

    fn instructions(content: &str) -> (String, Value, String) {
        ("developer".into(), Value::Null, content.into())
    }

    #[test]
    fn client_events_take_only_the_shapes_the_realtime_server_accepts() {
        assert_eq!(Tell::Session(Channel::Developer, "Hum.".into()).event(), json!({ "type": "session.context.append", "channel": "developer", "content": [{ "type": "input_text", "text": "Hum." }] }));
        assert_eq!(Tell::Session(Channel::Commentary, "An image arrived.".into()).event()["channel"], "commentary");
        assert_eq!(Tell::Delegation("item_1".into(), "Green.".into()).event(), json!({ "type": "delegation.context.append", "delegation_item_id": "item_1", "channel": "speakable", "content": [{ "type": "input_text", "text": "Green." }] }));
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
        assert_eq!(f.told(), [speakable(json!("item_exact"), "Two jobs need attention."), speakable(json!("item_exact"), "Both are on the display."), speakable(Value::Null, "The link is the release notes.")]);
        assert_eq!(f.frames("hub-ack"), [json!({ "id": "item_exact", "stamp": "mix" }), json!({ "id": "share_1", "stamp": "share_1" })]);
        let calls: Vec<_> = f.trace.calls.iter().map(|call| (call.request.as_str(), call.state, call.replies.join(" "))).collect();
        assert_eq!(calls, [
            ("", State::Replied, "The link is the release notes.".into()),
            ("What's in the build queue right now?", State::Replied, "Two jobs need attention. Both are on the display.".into()),
            ("Hello there.", State::Waiting, String::new()),
        ]);
    }

    #[test]
    fn a_display_only_push_says_nothing_and_each_display_is_kept_for_the_panel_with_its_image() {
        let mut f = Fixture::new();
        f.hear("How many jobs are queued?");
        f.delegate("slow");
        f.reply(json!({ "id": "slow", "stamp": "push_1", "commentary": [], "instructions": [], "display": { "markdown": "chart" } }));
        assert!(f.told().is_empty());
        assert!(f.hub.waiting.contains("slow"));
        assert_eq!(f.hub.take_display(), Some(Display { markdown: Some("chart".into()), ..Display::default() }));
        assert_eq!(f.hub.take_display(), None);
        let line = json!({ "id": "slow", "stamp": "reply_1", "commentary": ["Four jobs are queued."], "instructions": [], "display": { "link": "https://example.com/q", "image": { "mime": "image/png" } } });
        let mut body = format!("{line}\n").into_bytes();
        body.extend([0x89, b'P', b'N', b'G', b'\n', 0]);
        f.hub.reply(&body, &mut f.trace, f.now);
        f.collect();
        assert_eq!(f.told(), [speakable(json!("slow"), "Four jobs are queued.")]);
        assert!(f.hub.waiting.is_empty());
        assert_eq!(f.hub.take_display(), None, "a spoken reply's display waits for its speech");
        f.speak("Four jobs");
        assert_eq!(f.hub.take_display(), Some(Display { link: Some("https://example.com/q".into()), image: Some(vec![0x89, b'P', b'N', b'G', b'\n', 0]), ..Display::default() }));
        f.wait(TURN_QUIET, true);
        f.reply(json!({ "id": "slow", "stamp": "html_1", "commentary": [], "instructions": [], "display": { "html": "<!doctype html><p>chart</p>", "markdown": "ignored" } }));
        assert_eq!(f.hub.take_display(), Some(Display { html: true, ..Display::default() }));
        assert!(Display { html: true, ..Display::default() }.card(), "an html display still gets a card saying where it is shown");
        assert!(Display { markdown: Some("Draft".into()), ..Display::default() }.card());
        assert!(!Display { point: Some("gear".into()), ..Display::default() }.card());
        assert!(f.frames("hub-ack").iter().all(|ack| ack.get("unspoken").is_none()));
    }

    #[test]
    fn an_object_display_carries_the_trailer_as_its_scene() {
        let mut f = Fixture::new();
        f.hear("Show me a cube.");
        f.delegate("cube");
        let line = json!({ "id": "cube", "stamp": "o1", "commentary": ["Here it is."], "instructions": [], "display": { "markdown": "A cube", "object": { "mime": "model/gltf-binary" } } });
        let mut body = format!("{line}\n").into_bytes();
        body.extend(b"glTF\x02\0\0\0");
        f.hub.reply(&body, &mut f.trace, f.now);
        f.speak("Here it is.");
        assert_eq!(f.hub.take_display(), Some(Display { markdown: Some("A cube".into()), object: Some(b"glTF\x02\0\0\0".to_vec()), ..Display::default() }));
        f.reply(json!({ "id": "cube", "stamp": "o2", "commentary": ["This face."], "instructions": [], "display": { "point": "top" } }));
        f.wait(TURN_QUIET, true);
        f.speak("This face.");
        let pointed = f.hub.take_display().unwrap();
        assert_eq!(pointed.point.as_deref(), Some("top"));
        assert!(pointed.addresses_object(), "a display that only points leaves the panel and the object");
        for (stamp, hold, read) in [("o3", json!(0), Some(Hold::At(0.0))), ("o4", json!(12.5), Some(Hold::At(12.5))), ("o5", json!(false), Some(Hold::Run)), ("o6", json!(-1), None), ("o7", json!(true), None)] {
            f.reply(json!({ "id": "cube", "stamp": stamp, "commentary": ["Like so."], "instructions": [], "display": { "data": "2026-10-12", "hold": hold } }));
            f.wait(TURN_QUIET, true);
            f.speak("Like so.");
            let held = f.hub.take_display().unwrap();
            assert_eq!(held.hold, read, "hold {hold}");
            assert!(held.addresses_object(), "a display that holds the timeline leaves the panel and the object");
        }
        f.reply(json!({ "id": "cube", "stamp": "o8", "commentary": ["Gone."], "instructions": [], "display": { "clear": true } }));
        f.wait(TURN_QUIET, true);
        f.speak("Gone.");
        let cleared = f.hub.take_display().unwrap();
        assert!(cleared.clear && cleared.addresses_object() && !cleared.card(), "a clear leaves the panel");
    }

    #[test]
    fn follow_up_displays_play_like_slides_each_shown_as_its_speech_starts_and_one_dropped_by_barge_in_is_never_shown() {
        let mut f = Fixture::new();
        let object = |stamp: &str, text: &str, scene: &[u8]| {
            let line = json!({ "id": "tour", "stamp": stamp, "commentary": [text], "instructions": [], "display": { "object": { "mime": "model/gltf-binary" } } });
            let mut body = format!("{line}\n").into_bytes();
            body.extend(scene);
            body
        };
        f.hear("Show me the tour.");
        f.delegate("tour");
        for (stamp, text, scene) in [("caffeine", "This is caffeine.", b"caffeine"), ("saddle", "This is a saddle.", b"saddle__"), ("gauge", "This is a gauge.", b"gauge___")] {
            f.hub.reply(&object(stamp, text, scene), &mut f.trace, f.now);
            f.collect();
        }
        let shown = |f: &mut Fixture| f.hub.take_display().and_then(|display| display.object);
        assert_eq!(shown(&mut f), None, "nothing shows before the model speaks");
        f.speak("This is");
        assert_eq!(shown(&mut f), Some(b"caffeine".to_vec()));
        f.speak(" caffeine.");
        f.wait(Duration::from_millis(100), false);
        assert_eq!(shown(&mut f), None, "the next slide waits while the model still speaks this one");
        f.wait(TURN_QUIET, true);
        assert_eq!(f.told()[1..], [speakable(json!("tour"), "This is a saddle.")]);
        assert_eq!(shown(&mut f), None, "a follow-up told to the model shows nothing until its speech starts");
        f.speak("This is a saddle.");
        assert_eq!(shown(&mut f), Some(b"saddle__".to_vec()));
        f.hear("Stop.");
        f.wait(TURN_START, true);
        f.speak("Okay.");
        assert_eq!(shown(&mut f), None, "the follow-up the barge-in dropped never shows");
        assert_eq!(f.told().len(), 2);
    }

    #[test]
    fn a_failed_request_is_told_once_and_a_delegation_with_no_words_fails() {
        let mut f = Fixture::new();
        f.hear("Is the printer busy?");
        f.delegate("lost");
        f.hub.failed("lost", "delegation queue is full", &mut f.trace);
        f.hub.failed("lost", "again", &mut f.trace);
        f.delegate("empty");
        f.collect();
        assert_eq!(f.told(), [speakable(json!("lost"), FAILED), speakable(json!("empty"), FAILED)]);
        assert_eq!(f.frames("delegate").len(), 1);
        let calls: Vec<_> = f.trace.calls.iter().map(|call| (call.request.clone(), call.state)).collect();
        assert_eq!(calls, [(String::new(), State::Failed), ("Is the printer busy?".into(), State::Failed)]);
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
        assert_eq!(f.told(), [speakable(json!("first"), "First part."), speakable(json!("first"), "Second part."), speakable(Value::Null, "Other answer.")]);
        f.wait(Duration::from_secs(1), true);
        assert_eq!(f.told().len(), 3, "quiet before the model speaks does not end the turn");
        f.hear(" about the garden.");
        f.speak("First part.");
        assert_eq!(f.listens, 1);
        f.wait(Duration::from_millis(100), false);
        assert_eq!(f.told().len(), 3);
        f.wait(Duration::from_millis(100), true);
        assert_eq!(f.told()[3..], [speakable(json!("first"), "Slide two.")]);
        f.speak("Slide two.");
        f.hear("Stop there.");
        f.wait(Duration::from_secs(30), true);
        assert_eq!(f.told().len(), 4, "slide three was dropped by the barge-in");
        f.reply(reply("later", "o5", &["Later answer."]));
        assert_eq!(f.told()[4..], [speakable(Value::Null, "Later answer.")]);
        assert_eq!(f.frames("hub-ack").iter().map(|ack| ack["stamp"].as_str().unwrap().to_owned()).collect::<Vec<_>>(), ["o1", "o2", "o3", "o4", "o5"]);
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
        assert_eq!(f.told(), [speakable(json!("styled"), "The beacon is violet.")]);
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
        for seconds in [300.0, 599.0, 600.0, 601.0] {
            f.hub.usage(seconds);
        }
        f.collect();
        assert_eq!(f.told()[4..], [instructions("Answer in one short sentence.")]);
        f.hub.usage(1199.0);
        f.collect();
        assert_eq!(f.told().len(), 5);
        f.hub.usage(1200.0);
        f.collect();
        assert_eq!(f.told()[5..], [instructions("Answer in one short sentence.")]);
        assert_eq!(f.frames("hub-ack"), [json!({ "id": "styled", "stamp": "s1" }), json!({ "id": "silent", "stamp": "s2" })]);
    }

    #[test]
    fn pending_instructions_wait_for_every_earlier_reply_and_hold_back_a_first_reply_that_would_jump_the_queue() {
        let mut f = Fixture::new();
        f.reply(json!({ "id": "early", "stamp": "early", "commentary": [], "instructions": ["Old direction."] }));
        f.reply(json!({ "id": "a", "stamp": "a", "commentary": ["Reply A."], "instructions": ["New direction."] }));
        f.reply(json!({ "id": "b", "stamp": "b", "first": true, "commentary": ["Reply B."], "instructions": [] }));
        f.hub.usage(900.0);
        f.collect();
        let early = [instructions("Old direction."), speakable(Value::Null, "Reply A.")];
        assert_eq!(f.told(), early);
        f.speak("Reply A.");
        f.wait(Duration::ZERO, true);
        assert_eq!(f.told()[2..], [speakable(Value::Null, "Reply B.")]);
        f.speak("Reply B.");
        f.wait(Duration::ZERO, true);
        assert_eq!(f.told()[3..], [instructions("New direction.")]);
    }

    #[test]
    fn instructions_wait_past_the_turn_timeout_until_the_model_speaks_the_reply() {
        let mut f = Fixture::new();
        f.reply(json!({ "id": "late", "stamp": "late", "commentary": ["Reply."], "instructions": ["Direction."] }));
        f.wait(TURN_START + Duration::from_secs(1), true);
        assert_eq!(f.told(), [speakable(Value::Null, "Reply.")]);
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
    fn words_after_the_last_delegation_reach_the_hub_once_when_the_session_ends() {
        let mut f = Fixture::new();
        f.hear("Check the printer.");
        f.delegate("printer");
        f.speak("Checking.");
        f.hear("Also remember the filament order. Goodnight.");
        f.speak(&crate::identity::identity().sign_off);
        f.hub.sleep();
        f.hub.ended(&f.trace, f.now);
        f.collect();
        let ended = f.frames("ended");
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0]["text"], "Also remember the filament order. Goodnight.");
        assert_eq!(ended[0]["context"], json!([{ "speaker": "user", "text": "Check the printer." }, { "speaker": "live", "text": "Checking." }, { "speaker": "live", "text": crate::identity::identity().sign_off }]));
        assert_eq!(f.frames("delegate").len(), 1);
    }

    #[test]
    fn a_session_ending_with_nothing_said_since_the_last_delegation_sends_nothing() {
        let mut f = Fixture::new();
        f.hear("Check the printer.");
        f.delegate("printer");
        f.speak("Checking. ");
        f.hear("?");
        f.speak(&crate::identity::identity().sign_off);
        f.hub.sleep();
        f.hub.ended(&f.trace, f.now);
        f.collect();
        assert!(f.frames("ended").is_empty());
        let mut silent = Fixture::new();
        silent.hub.ended(&silent.trace, silent.now);
        silent.collect();
        assert!(silent.frames("ended").is_empty());
    }

    #[test]
    fn after_the_sign_off_an_ignored_turn_is_not_forwarded() {
        let mut f = Fixture::new();
        f.hear("Check the printer.");
        f.delegate("printer");
        f.hub.sleep();
        f.hear(" Goodbye.");
        f.wait(Duration::from_secs(4), false);
        assert_eq!(f.frames("delegate").len(), 1);
    }

    #[test]
    fn a_request_ended_with_the_hand_off_phrase_goes_whole_from_the_model_within_three_seconds_or_else_from_the_hub() {
        let mut f = Fixture::new();
        f.hear("Is the printer");
        f.delegate("item_printer");
        f.hear(" busy right now? Fly, Corvus.");
        f.wait(Duration::from_secs(1), false);
        f.delegate("item_complete");
        f.wait(Duration::from_millis(3100), false);
        assert_eq!(f.frames("delegate").len(), 2);
        f.hear("And the backup");
        f.delegate("item_backup");
        f.hear(" from last night. Fly, Corvus");
        f.wait(Duration::from_secs(1), false);
        f.speak("Okay.");
        f.wait(Duration::from_millis(1900), false);
        assert_eq!(f.frames("delegate").len(), 3);
        f.wait(Duration::from_millis(200), false);
        let frames = f.frames("delegate");
        assert!(frames[3]["id"].as_str().unwrap().starts_with("turn_"));
        assert_eq!(frames.iter().map(|frame| frame["text"].as_str().unwrap()).collect::<Vec<_>>(), ["Is the printer", "Is the printer\nbusy right now?", "And the backup", "And the backup\nfrom last night."]);
    }

    #[test]
    fn a_finished_thought_goes_once_judged() {
        let mut f = Fixture { verdict: None, ..Fixture::new() };
        f.hear("How big is it?");
        f.delegate("item_size");
        assert!(f.frames("delegate").is_empty(), "nothing goes before the judgement");
        f.judged(true);
        assert_eq!(f.frames("delegate"), [json!({ "id": "item_size", "text": "How big is it?", "context": [], "duration_ms": 0 })]);
    }

    #[test]
    fn an_unfinished_thought_waits_and_goes_whole_under_lives_newest_id() {
        let mut f = Fixture { verdict: None, ..Fixture::new() };
        f.hear("Okay. So");
        f.delegate("item_so");
        f.judged(false);
        f.wait(Duration::from_secs(4), false);
        f.hear(" I have an idea.");
        f.delegate("item_idea");
        f.wait(judge_quiet() - Duration::from_millis(1), false);
        assert!(f.asks.is_empty(), "renewed speech is judged after a quiet");
        f.wait(Duration::from_millis(1), false);
        f.judged(true);
        let frames = f.frames("delegate");
        assert_eq!(frames.len(), 1);
        assert_eq!((frames[0]["id"].as_str(), frames[0]["text"].as_str()), (Some("item_idea"), Some("Okay. So I have an idea.")));
    }

    #[test]
    fn an_unfinished_thought_goes_after_the_hold_of_quiet() {
        let mut f = Fixture { verdict: None, ..Fixture::new() };
        f.hear("Just need to figure out");
        f.delegate("item_figure");
        f.judged(false);
        f.wait(hold() - Duration::from_millis(1), false);
        assert!(f.frames("delegate").is_empty());
        f.wait(Duration::from_millis(1), false);
        assert_eq!(f.frames("delegate")[0]["text"], "Just need to figure out");
    }

    #[test]
    fn a_judgement_of_speech_that_went_on_is_dropped() {
        let mut f = Fixture { verdict: None, ..Fixture::new() };
        f.hear("Can you try showing");
        f.delegate("item_showing");
        f.hear(" me the doodad?");
        f.judged(true);
        assert!(f.frames("delegate").is_empty(), "the verdict was on speech that has since continued");
        f.wait(judge_quiet(), false);
        f.judged(true);
        assert_eq!(f.frames("delegate")[0]["text"], "Can you try showing me the doodad?");
    }

    #[test]
    fn a_turn_the_model_ignores_is_held_like_its_own_hand_offs() {
        let mut f = Fixture::new();
        f.hear("Grow the trees.");
        f.delegate("item_trees");
        f.verdict = None;
        f.hear(" When I pick it back up");
        f.wait(Duration::from_secs(3), false);
        f.judged(false);
        f.wait(hold() - Duration::from_secs(3) - Duration::from_millis(1), false);
        assert_eq!(f.frames("delegate").len(), 1);
        f.wait(Duration::from_millis(1), false);
        let frames = f.frames("delegate");
        assert!(frames[1]["id"].as_str().unwrap().starts_with("turn_"));
        assert_eq!(frames[1]["text"], "When I pick it back up");
    }

    #[test]
    fn the_hand_off_phrase_ends_a_thought_without_a_judgement() {
        let mut f = Fixture { verdict: None, ..Fixture::new() };
        f.hear("Is the printer busy? Fly, Corvus.");
        f.delegate("item_printer");
        assert!(f.asks.is_empty());
        assert_eq!(f.frames("delegate")[0]["text"], "Is the printer busy?");
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
        assert_eq!(f.told(), [speakable(Value::Null, "The printer is idle.")]);
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
            let mut recorded = Vec::new();
            for row in rows.iter().filter(|row| row["ch"] == session) {
                f.now = start + Duration::from_millis(row["at"].as_u64().unwrap());
                f.wait(Duration::ZERO, false);
                let event = &row["event"];
                match (row["dir"].as_str().unwrap(), event["type"].as_str().unwrap_or("")) {
                    ("in", "input_transcript.added") => f.hear(event["item"]["text"].as_str().unwrap()),
                    ("in", "output_transcript.added") => f.speak(event["item"]["text"].as_str().unwrap()),
                    ("in", "delegation.created") => {
                        let id = format!("recorded_{session}_{}", delegations.len());
                        f.delegate(&id);
                        delegations.push(id);
                        recorded.push(event["item"]["id"].clone());
                    }
                    ("out", "delegation.context.append") => {
                        assert_eq!(event["delegation_item_id"], recorded[0], "session {session}");
                        let before = f.told().len();
                        f.reply(json!({ "id": delegations[0], "stamp": format!("reply-{session}"), "first": true, "commentary": [event["content"][0]["text"]], "instructions": [], "timing_ms": 1 }));
                        assert_eq!(f.told()[before..], [speakable(json!(delegations[0]), expected.as_str().unwrap())], "session {session}");
                    }
                    _ => {}
                }
            }
            assert_eq!(delegations.len(), 1, "session {session}");
            let delegate = f.frames("delegate");
            assert_eq!(delegate.len(), 1, "session {session}");
            assert_eq!(delegate[0]["id"], delegations[0].as_str());
            let heard: String = rows.iter().filter(|row| row["ch"] == session && row["event"]["type"] == "input_transcript.added" && row["at"].as_u64() < rows.iter().find(|row| row["ch"] == session && row["event"]["type"] == "delegation.created").unwrap()["at"].as_u64()).map(|row| row["event"]["item"]["text"].as_str().unwrap()).collect();
            assert_eq!(delegate[0]["text"], heard.trim(), "session {session}");
            assert_eq!(f.frames("hub-ack").len(), 1, "session {session}");
            assert!(f.hub.waiting.is_empty(), "session {session}");
        }
    }
}
