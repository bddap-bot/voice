use std::collections::VecDeque;

const KEPT: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Waiting,
    Replied,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    id: String,
    pub request: String,
    pub state: State,
    pub replies: Vec<String>,
}

/// The recent hub calls, newest first, kept across sessions until Reset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Calls {
    calls: VecDeque<Call>,
}

impl Calls {
    pub fn iter(&self) -> impl Iterator<Item = &Call> {
        self.calls.iter()
    }

    pub fn asked(&mut self, id: &str, request: &str) {
        self.calls.retain(|call| call.id != id);
        self.calls.push_front(Call { id: id.to_owned(), request: request.to_owned(), state: State::Waiting, replies: Vec::new() });
        self.calls.truncate(KEPT);
    }

    /// A reply to a call not in the log, such as a push the hub started, enters it with no request.
    pub fn replied(&mut self, id: &str, commentary: &[String]) {
        if commentary.is_empty() {
            return;
        }
        if !self.calls.iter().any(|call| call.id == id) {
            self.asked(id, "");
        }
        let call = self.calls.iter_mut().find(|call| call.id == id).unwrap();
        call.state = State::Replied;
        call.replies.extend(commentary.iter().cloned());
    }

    pub fn failed(&mut self, id: &str) {
        if let Some(call) = self.calls.iter_mut().find(|call| call.id == id && call.state == State::Waiting) {
            call.state = State::Failed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| text.to_string()).collect()
    }

    #[test]
    fn calls_list_newest_first_with_their_state_and_replies_and_only_the_recent_ones_stay() {
        let mut calls = Calls::default();
        calls.asked("a", "Is the printer busy?");
        calls.asked("b", "What is queued?");
        calls.replied("a", &strings(&["It is idle.", "Bed is clear."]));
        calls.replied("b", &[]);
        calls.failed("b");
        calls.failed("a");
        calls.replied("push", &strings(&["A job finished."]));
        let shown: Vec<_> = calls.iter().map(|call| (call.request.as_str(), call.state, call.replies.clone())).collect();
        assert_eq!(shown, [
            ("", State::Replied, strings(&["A job finished."])),
            ("What is queued?", State::Failed, vec![]),
            ("Is the printer busy?", State::Replied, strings(&["It is idle.", "Bed is clear."])),
        ]);
        for index in 0..10 {
            calls.asked(&index.to_string(), "again");
        }
        assert_eq!(calls.iter().map(|call| call.id.as_str()).collect::<Vec<_>>(), ["9", "8", "7", "6", "5"]);
    }
}
