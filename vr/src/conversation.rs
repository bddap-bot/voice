use std::time::{Duration, Instant};

use serde::Serialize;

const MEMORY: &str = "Context: Archived transcripts of completed conversations, oldest first, for memory only. Each ended when you went to sleep. These utterances already happened; do not repeat or continue them.\n";
const MAX_CONTEXT: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaker {
    User,
    Live,
}

#[derive(Clone, Serialize)]
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

/// The conversation carried between sessions, as the page's `ConversationTrace` keeps it, without delegations.
#[derive(Default)]
pub struct Trace {
    turns: Vec<Turn>,
    sleeps: Vec<(usize, Instant)>,
}

fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs_f64().round();
    let (unit, size) = [("day", 86400.0), ("hour", 3600.0), ("minute", 60.0)].into_iter().find(|&(_, size)| seconds >= 2.0 * size).unwrap_or(("second", 1.0));
    let count = (seconds / size).round();
    format!("{count} {unit}{}", if count == 1.0 { "" } else { "s" })
}

impl Trace {
    fn at_sleep(&self) -> bool {
        self.sleeps.last().is_some_and(|&(index, _)| index == self.turns.len())
    }

    fn add(&mut self, speaker: Speaker, delta: &str) {
        let at_sleep = self.at_sleep();
        match self.turns.last_mut() {
            Some(turn) if turn.speaker == speaker && !at_sleep => turn.text.push_str(delta),
            _ => self.turns.push(Turn { speaker, text: delta.to_owned() }),
        }
    }

    pub fn heard(&mut self, delta: &str) {
        self.add(Speaker::User, delta);
    }

    pub fn spoke(&mut self, delta: &str) {
        self.add(Speaker::Live, delta);
    }

    pub fn slept(&mut self, now: Instant) {
        self.sleeps.push((self.turns.len(), now));
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
        let trace = Trace { turns: vec![Turn { speaker: Speaker::User, text: "hi".into() }], sleeps: vec![(1, Instant::now())] };
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
        trace.heard("Hel");
        trace.heard("lo");
        trace.spoke("Hi.");
        trace.slept(start);
        trace.spoke("Back.");
        trace.heard("Bye");
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
        trace.heard("one");
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
            trace.heard(&format!("question {index} "));
            trace.spoke(&format!("answer {index} "));
        }
        trace.slept(start);
        let context = trace.wake(start).context.unwrap();
        let length = serde_json::to_string(&context).unwrap().len();
        assert!(length <= MAX_CONTEXT && length > MAX_CONTEXT - 200, "{length}");
        let text = context[0]["text"].as_str().unwrap();
        assert!(text.contains("answer 399") && !text.contains("question 0 "));
    }
}
