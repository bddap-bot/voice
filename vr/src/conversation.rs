use std::time::{Duration, Instant};

use serde::Serialize;

const MEMORY: &str = "Context: Archived transcripts of completed conversations, oldest first, for memory only. Each ended when you went to sleep. These utterances already happened; do not repeat or continue them.\n";
const MAX_CONTEXT: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    User,
    Live,
}

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct Turn {
    pub speaker: Speaker,
    pub text: String,
}

#[derive(Serialize)]
struct Archived {
    turns: Vec<Turn>,
    went_to_sleep: String,
}

/// What the offer carries so a new session remembers the earlier ones: the page's `ConversationTrace.wake()`.
#[derive(Serialize, Default, Debug, PartialEq)]
pub struct Wake {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake: Option<String>,
}

/// What a delegation hands the hub: the page's `ConversationTrace.delegated()`.
#[derive(Debug, PartialEq)]
pub struct Delegation {
    pub text: String,
    pub context: Vec<Turn>,
    pub duration_ms: u64,
}

/// The conversation carried between sessions, as the page's `ConversationTrace` keeps it; a delegation leaves only a boundary.
#[derive(Default)]
pub struct Trace {
    turns: Vec<Turn>,
    sleeps: Vec<(usize, Instant)>,
    /// The first turn heard since the last delegation or sleep; user turns from here on are the next delegation's text.
    pending: usize,
    heard_at: Option<Instant>,
    /// The page's delegation entry: the next turn of either speaker starts fresh.
    split: bool,
    /// The page's `forceNewSpeech` after a hub reply.
    split_speech: bool,
}

fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs_f64().round();
    let (unit, size) = [("day", 86400.0), ("hour", 3600.0), ("minute", 60.0)].into_iter().find(|&(_, size)| seconds >= 2.0 * size).unwrap_or(("second", 1.0));
    let count = (seconds / size).round();
    format!("{count} {unit}{}", if count == 1.0 { "" } else { "s" })
}

/// The page's `boundedText`: the newest parts joined by newlines, then the newest bytes on a character boundary.
fn bounded_text(parts: &[&str], maximum: usize) -> String {
    let mut parts = parts;
    while parts.len() > 1 && parts.join("\n").len() > maximum {
        parts = &parts[1..];
    }
    let joined = parts.join("\n");
    let mut start = joined.len().saturating_sub(maximum);
    while !joined.is_char_boundary(start) {
        start += 1;
    }
    joined[start..].to_owned()
}

impl Trace {
    fn at_sleep(&self) -> bool {
        self.sleeps.last().is_some_and(|&(index, _)| index == self.turns.len())
    }

    fn add(&mut self, speaker: Speaker, delta: &str) {
        let fresh = self.at_sleep() || std::mem::take(&mut self.split) || speaker == Speaker::Live && std::mem::take(&mut self.split_speech);
        match self.turns.last_mut() {
            Some(turn) if turn.speaker == speaker && !fresh => turn.text.push_str(delta),
            _ => self.turns.push(Turn { speaker, text: delta.to_owned() }),
        }
    }

    pub fn heard(&mut self, delta: &str, now: Instant) {
        self.heard_at.get_or_insert(now);
        self.add(Speaker::User, delta);
    }

    pub fn spoke(&mut self, delta: &str) {
        self.add(Speaker::Live, delta);
    }

    pub fn slept(&mut self, now: Instant) {
        self.sleeps.push((self.turns.len(), now));
        self.pending = self.turns.len();
        self.heard_at = None;
    }

    fn is_pending(&self, index: usize) -> bool {
        index >= self.pending && self.turns[index].speaker == Speaker::User
    }

    /// The user's words since the last delegation, and up to twenty turns before them.
    pub fn delegated(&mut self, now: Instant) -> Delegation {
        let heard: Vec<&str> = (self.pending..self.turns.len()).filter(|&index| self.is_pending(index)).map(|index| self.turns[index].text.trim()).filter(|text| !text.is_empty()).collect();
        let text = bounded_text(&heard, MAX_CONTEXT);
        let visible: Vec<Turn> = (0..self.turns.len()).filter(|&index| !self.is_pending(index)).map(|index| self.turns[index].clone()).collect();
        let mut context = visible[visible.len().saturating_sub(20)..].to_vec();
        while !context.is_empty() && serde_json::to_string(&context).unwrap().len() > MAX_CONTEXT {
            context.remove(0);
        }
        let duration_ms = self.heard_at.take().map_or(0, |at| now.saturating_duration_since(at).as_millis() as u64);
        self.pending = self.turns.len();
        self.split = true;
        Delegation { text, context, duration_ms }
    }

    pub fn hub_replied(&mut self) {
        self.split_speech = true;
    }

    pub fn wake(&self, now: Instant) -> Wake {
        let Some(&(_, last)) = self.sleeps.last() else { return Wake::default() };
        let mut conversations: Vec<Archived> = self
            .sleeps
            .iter()
            .enumerate()
            .map(|(i, &(index, at))| Archived { turns: self.turns[if i == 0 { 0 } else { self.sleeps[i - 1].0 }..index].to_vec(), went_to_sleep: format!("about {} ago", elapsed(now - at)) })
            .filter(|archived| !archived.turns.is_empty())
            .collect();
        let memory = |conversations: &[Archived]| serde_json::json!([{ "speaker": "user", "text": format!("{MEMORY}{}", serde_json::to_string(conversations).unwrap()) }]);
        while !conversations.is_empty() && memory(&conversations).to_string().len() > MAX_CONTEXT {
            conversations[0].turns.remove(0);
            if conversations[0].turns.is_empty() {
                conversations.remove(0);
            }
        }
        Wake {
            context: Some(if conversations.is_empty() { Vec::new() } else { memory(&conversations).as_array().unwrap().clone() }),
            wake: Some(format!("The previous conversation ended and you went to sleep about {} ago. You have just been woken for a new conversation. Any earlier goodbye or request to sleep was already completed. Keep the earlier conversations as memory; greet briefly and listen for a new request.", elapsed(now - last))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wake_texts_are_the_pages() {
        let page = include_str!("../../docs/live.js");
        assert!(page.contains(&MEMORY.replace('\n', "\\n")));
        let trace = Trace { turns: vec![Turn { speaker: Speaker::User, text: "hi".into() }], sleeps: vec![(1, Instant::now())], ..Trace::default() };
        let wake = trace.wake(Instant::now()).wake.unwrap();
        for piece in wake.split("about 0 seconds ago") {
            assert!(page.contains(piece), "{piece}");
        }
    }

    #[test]
    fn elapsed_names_the_largest_unit_reached_twice() {
        let cases = [(0, "0 seconds"), (1, "1 second"), (119, "119 seconds"), (120, "2 minutes"), (7199, "120 minutes"), (7200, "2 hours"), (3 * 86400, "3 days")];
        for (seconds, text) in cases {
            assert_eq!(elapsed(Duration::from_secs(seconds)), text);
        }
    }

    #[test]
    fn a_first_session_carries_no_memory() {
        assert_eq!(Trace::default().wake(Instant::now()), Wake::default());
        assert_eq!(serde_json::to_string(&Wake::default()).unwrap(), "{}");
    }

    #[test]
    fn turns_merge_by_speaker_and_split_at_each_sleep() {
        let start = Instant::now();
        let mut trace = Trace::default();
        trace.heard("Hel", start);
        trace.heard("lo", start);
        trace.spoke("Hi.");
        trace.slept(start);
        trace.spoke("Back.");
        trace.heard("Bye", start);
        trace.slept(start + Duration::from_secs(30));
        let wake = trace.wake(start + Duration::from_secs(150));
        assert_eq!(wake.wake.unwrap(), wake_text("2 minutes"));
        let context = wake.context.unwrap();
        assert_eq!(context.len(), 1);
        assert_eq!(context[0]["speaker"], "user");
        let archived: serde_json::Value = serde_json::from_str(context[0]["text"].as_str().unwrap().strip_prefix(MEMORY).unwrap()).unwrap();
        assert_eq!(
            archived,
            serde_json::json!([
                { "turns": [{ "speaker": "user", "text": "Hello" }, { "speaker": "live", "text": "Hi." }], "went_to_sleep": "about 3 minutes ago" },
                { "turns": [{ "speaker": "live", "text": "Back." }, { "speaker": "user", "text": "Bye" }], "went_to_sleep": "about 2 minutes ago" },
            ])
        );
    }

    fn wake_text(ago: &str) -> String {
        format!("The previous conversation ended and you went to sleep about {ago} ago. You have just been woken for a new conversation. Any earlier goodbye or request to sleep was already completed. Keep the earlier conversations as memory; greet briefly and listen for a new request.")
    }

    #[test]
    fn a_session_with_no_turns_after_a_sleep_adds_no_archive() {
        let start = Instant::now();
        let mut trace = Trace::default();
        trace.heard("one", start);
        trace.slept(start);
        trace.slept(start);
        let context = trace.wake(start).context.unwrap();
        assert_eq!(context.len(), 1);
        assert_eq!(serde_json::from_str::<serde_json::Value>(context[0]["text"].as_str().unwrap().strip_prefix(MEMORY).unwrap()).unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn memory_drops_its_oldest_turns_to_fit_the_bound() {
        let start = Instant::now();
        let mut trace = Trace::default();
        for index in 0..400 {
            trace.heard(&format!("question {index} "), start);
            trace.spoke(&format!("answer {index} "));
        }
        trace.slept(start);
        let context = trace.wake(start).context.unwrap();
        let length = serde_json::to_string(&context).unwrap().len();
        assert!(length <= MAX_CONTEXT && length > MAX_CONTEXT - 200, "{length}");
        let text = context[0]["text"].as_str().unwrap();
        assert!(text.contains("answer 399") && !text.contains("question 0 "));
    }

    fn turn(speaker: Speaker, text: &str) -> Turn {
        Turn { speaker, text: text.into() }
    }

    #[test]
    fn a_delegation_hands_over_what_was_heard_since_the_last_and_the_turns_before_it() {
        let start = Instant::now();
        let mut trace = Trace::default();
        trace.heard("Hello there.", start);
        let first = trace.delegated(start + Duration::from_millis(700));
        assert_eq!(first, Delegation { text: "Hello there.".into(), context: vec![], duration_ms: 700 });
        trace.spoke("Hi.");
        trace.heard("What's in ", start + Duration::from_secs(1));
        trace.heard("the build ", start + Duration::from_secs(2));
        trace.heard("queue right now?", start + Duration::from_secs(3));
        let second = trace.delegated(start + Duration::from_secs(4));
        assert_eq!(second.text, "What's in the build queue right now?");
        assert_eq!(second.context, vec![turn(Speaker::User, "Hello there."), turn(Speaker::Live, "Hi.")]);
        assert_eq!(second.duration_ms, 3000);
        assert_eq!(trace.delegated(start).text, "");
    }

    #[test]
    fn a_delegation_splits_turns_and_live_turns_after_it_are_context() {
        let start = Instant::now();
        let mut trace = Trace::default();
        trace.heard("one", start);
        trace.delegated(start);
        trace.heard("two", start);
        trace.spoke("ok");
        trace.heard("three", start);
        let delegation = trace.delegated(start);
        assert_eq!(delegation.text, "two\nthree");
        assert_eq!(delegation.context, vec![turn(Speaker::User, "one"), turn(Speaker::Live, "ok")]);
    }

    #[test]
    fn a_hub_reply_starts_a_new_spoken_turn_only() {
        let start = Instant::now();
        let mut trace = Trace::default();
        trace.spoke("Checking.");
        trace.hub_replied();
        trace.spoke("Four jobs.");
        trace.heard("a", start);
        trace.hub_replied();
        trace.heard("b", start);
        assert_eq!(trace.turns.iter().map(|turn| turn.text.as_str()).collect::<Vec<_>>(), ["Checking.", "Four jobs.", "ab"]);
    }

    #[test]
    fn a_sleep_ends_the_pending_words_without_sending_them() {
        let start = Instant::now();
        let mut trace = Trace::default();
        trace.heard("before", start);
        trace.slept(start);
        trace.heard("after", start);
        let delegation = trace.delegated(start);
        assert_eq!(delegation.text, "after");
        assert_eq!(delegation.context, vec![turn(Speaker::User, "before")]);
    }

    #[test]
    fn context_keeps_the_last_twenty_turns_within_the_byte_bound() {
        let start = Instant::now();
        let mut trace = Trace::default();
        for index in 0..30 {
            trace.heard(&format!("q{index}"), start);
            trace.spoke(&format!("a{index}"));
        }
        trace.slept(start);
        let context = trace.delegated(start).context;
        assert_eq!(context.len(), 20);
        assert_eq!(context[0].text, "q20");
        let mut trace = Trace::default();
        for _ in 0..4 {
            trace.heard(&"x".repeat(3000), start);
            trace.spoke("y");
        }
        trace.slept(start);
        let context = trace.delegated(start).context;
        assert!(serde_json::to_string(&context).unwrap().len() <= MAX_CONTEXT);
        assert_eq!(context.len(), 5);
    }

    #[test]
    fn bounded_text_keeps_the_newest_parts_and_bytes() {
        assert_eq!(bounded_text(&["aaa", "bb", "c"], 4), "bb\nc");
        assert_eq!(bounded_text(&["abcdef"], 3), "def");
        assert_eq!(bounded_text(&["aé"], 2), "é");
        assert_eq!(bounded_text(&["éa"], 2), "a");
        assert_eq!(bounded_text(&[], 5), "");
    }
}
