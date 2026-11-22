use std::sync::OnceLock;
use std::time::Duration;

const SOURCE: &str = include_str!("../../docs/identity.js");

/// The page's identity constants, read from its own module so the two never drift.
pub struct Identity {
    pub name: String,
    /// The hand-off phrase's first word and the words it has been heard as, as phrase keys.
    pub call_sounds: Vec<String>,
    /// The name and how it has been heard, as phrase keys.
    pub name_sounds: [String; 2],
    pub sign_off: String,
    pub hand_off_phrase: String,
    pub inactivity: Duration,
    pub instructions: String,
    pub closing: String,
    pub woken: String,
    pub renewed: String,
    pub undelegated_reply: String,
    pub vr_wake_phrase: String,
    /// Turns Live answers itself, as word sequences, and the sounds no request is made of.
    pub pleasantries: Vec<Vec<String>>,
    pub closings: Vec<Vec<String>>,
    pub noise: Vec<String>,
    pub request_words: usize,
}

/// The source text of `export const NAME = …;`: a quoted literal, closed by its own quote, or an expression up to the line's `;`.
fn value(name: &str) -> &'static str {
    let prefix = format!("export const {name} = ");
    let start = SOURCE.find(&prefix).unwrap_or_else(|| panic!("identity.js has no {name}")) + prefix.len();
    let rest = &SOURCE[start..];
    let end = match rest.chars().next() {
        Some(quote @ ('`' | '\'')) => rest[1..].find(quote).map(|end| end + 2),
        _ => rest.find(';'),
    };
    &rest[..end.unwrap_or_else(|| panic!("identity.js: {name} is unterminated"))]
}

fn text(name: &str, names: &[(&str, &str)]) -> String {
    let raw = value(name);
    let body = &raw[1..raw.len() - 1];
    assert!(!body.contains('\\'), "identity.js: {name} uses an escape");
    let mut out = body.to_owned();
    for (key, value) in names {
        out = out.replace(&format!("${{{key}}}"), value);
    }
    assert!(!out.contains("${"), "identity.js: {name} interpolates an unknown constant");
    out
}

fn milliseconds(name: &str) -> u64 {
    value(name).split('*').map(|factor| factor.trim().parse::<u64>().unwrap_or_else(|_| panic!("identity.js: {name} is not a product of integers"))).product()
}

pub fn identity() -> &'static Identity {
    static IDENTITY: OnceLock<Identity> = OnceLock::new();
    IDENTITY.get_or_init(|| {
        let name = text("NAME", &[]);
        let sign_off = text("SIGN_OFF", &[]);
        let hand_off_phrase = text("HAND_OFF_PHRASE", &[("NAME", name.as_str())]);
        let names = [("NAME", name.as_str()), ("SIGN_OFF", sign_off.as_str()), ("HAND_OFF_PHRASE", hand_off_phrase.as_str())];
        let sounds = text("HAND_OFF_SOUNDS", &[]);
        Identity {
            instructions: text("IDENTITY", &names),
            closing: text("CLOSING", &names),
            woken: text("WOKEN", &names),
            renewed: text("RENEWED", &names),
            undelegated_reply: text("UNDELEGATED_REPLY", &names),
            vr_wake_phrase: text("VR_WAKE_PHRASE", &names),
            inactivity: Duration::from_millis(milliseconds("INACTIVITY_MS")),
            call_sounds: std::iter::once(without_trailing_phrase(&hand_off_phrase, &name)).chain(sounds.split(", ")).map(phrase_key).collect(),
            pleasantries: text("PLEASANTRIES", &[]).split(", ").map(words).collect(),
            closings: text("CLOSINGS", &[]).split(", ").map(words).collect(),
            noise: text("NOISE", &[]).split(", ").map(str::to_owned).collect(),
            request_words: value("REQUEST_WORDS").parse().expect("identity.js: REQUEST_WORDS is not an integer"),
            name_sounds: {
                let called = phrase_key(&name);
                let heard = called.strip_prefix('c').map_or_else(|| called.clone(), |rest| format!("k{rest}"));
                [called, heard]
            },
            name,
            sign_off,
            hand_off_phrase,
        }
    })
}

#[cfg(test)]
pub fn page_wake_phrase() -> String {
    text("WAKE_PHRASE", &[("NAME", &text("NAME", &[]))])
}

fn phrase_key(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The page's `includesPhrase`: letters and digits only, case-folded.
pub fn includes_phrase(text: &str, phrase: &str) -> bool {
    phrase_key(text).contains(&phrase_key(phrase))
}

/// The page's `beforeNameCall`: text that ends with the hand-off phrase or a near sound of it, said as its own phrase and not asked, without the phrase.
pub fn before_name_call(text: &str) -> Option<&str> {
    let word = |c: char| c.is_alphanumeric() || c == '\'';
    let name_end = text.trim_end_matches(|c: char| !word(c));
    let name_start = name_end.trim_end_matches(word).len();
    let name = phrase_key(&name_end[name_start..]);
    if !identity().name_sounds.contains(&name) {
        return None;
    }
    let before = name_end[..name_start].trim_end_matches(|c: char| !word(c));
    if before.len() == name_start {
        return None;
    }
    let call_start = before.trim_end_matches(word).len();
    let preceding = text[..call_start].trim_end_matches([' ', '\t']);
    let own_phrase = preceding.is_empty() || preceding.ends_with(['.', ',', '!', '?', ';', ':', '\n']);
    let asked = text[name_end.len()..].contains('?');
    (own_phrase && !asked && identity().call_sounds.contains(&phrase_key(&before[call_start..]))).then(|| preceding.trim_end_matches(|c: char| c.is_whitespace() || c == ','))
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase().replace('\u{2019}', "'").split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|word| !word.is_empty()).map(str::to_owned).collect()
}

/// The page's `requestWords`: the words of a turn by the rule beside `PLEASANTRIES` in identity.js, and whether it held a closing.
fn request_words(text: &str) -> (usize, bool) {
    let identity = identity();
    let mut words: Vec<String> = words(text).into_iter().filter(|word| !identity.name_sounds.contains(&phrase_key(word)) && !identity.call_sounds.contains(&phrase_key(word)) && !identity.noise.contains(word)).collect();
    let mut strip = |phrase: &[String]| {
        let mut found = false;
        while let Some(start) = (0..words.len()).find(|&start| words[start..].starts_with(phrase)) {
            words.drain(start..start + phrase.len());
            found = true;
        }
        found
    };
    let closed = identity.closings.iter().fold(false, |closed, phrase| strip(phrase) || closed);
    identity.pleasantries.iter().for_each(|phrase| {
        strip(phrase);
    });
    (words.len(), closed)
}

/// The page's `closes`: a closing with no more than `REQUEST_WORDS` words beside it.
pub fn closes(text: &str) -> bool {
    let (count, closed) = request_words(text);
    closed && count <= identity().request_words
}

/// The page's `forHub`: whether the page relays a turn to the hub itself; `least` lowers the words a turn needs while the hub owes a reply or a hand-off is held.
pub fn for_hub(text: &str, least: Option<usize>) -> bool {
    let (count, closed) = request_words(text);
    if closed {
        count > identity().request_words
    } else {
        count >= least.unwrap_or(identity().request_words)
    }
}

/// The page's `withoutTrailingPhrase`.
pub fn without_trailing_phrase<'a>(text: &'a str, phrase: &str) -> &'a str {
    let mut key = phrase_key(phrase);
    let mut start = text.len();
    for (index, c) in text.char_indices().rev() {
        if key.is_empty() {
            break;
        }
        if !c.is_alphanumeric() {
            continue;
        }
        let letter: String = c.to_lowercase().collect();
        if !key.ends_with(&letter) {
            return text;
        }
        key.truncate(key.len() - letter.len());
        start = index;
    }
    if !key.is_empty() {
        return text;
    }
    text[..start].trim_end_matches(|c: char| c.is_whitespace() || c == ',')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_identity_parses_with_its_constants_interpolated() {
        let identity = identity();
        assert!(SOURCE.contains(&format!("export const SIGN_OFF = '{}';", identity.sign_off)));
        assert_eq!(identity.inactivity, Duration::from_secs(600));
        assert!(identity.closing.contains(&format!("\"{}\"", identity.sign_off)));
        assert!(identity.woken.starts_with("Context: ") && !identity.woken.contains("${"));
    }

    #[test]
    fn a_phrase_matches_across_case_punctuation_and_split_deltas() {
        let sign_off = &identity().sign_off;
        assert!(includes_phrase(&format!("Sure. {}", sign_off.to_uppercase().replace(' ', "  ")), sign_off));
        assert!(includes_phrase("my labor here, is ENDED!", "My labor here is ended."));
        assert!(!includes_phrase("my labor here is", "My labor here is ended."));
    }

    #[test]
    fn a_request_closes_with_the_hand_off_phrase_or_a_near_sound_of_its_first_word_before_the_name() {
        assert_eq!(before_name_call(&format!("Check the printer. {}.", identity().hand_off_phrase)), Some("Check the printer."));
        for call in ["Quiet, Corvus.", "Fight, Corvus", "Flight Corvus!", "Fly, Korvus."] {
            assert_eq!(before_name_call(&format!("Is the backup done?\n{call}")), Some("Is the backup done?"), "{call}");
        }
        assert_eq!(before_name_call("Fly, Corvus"), Some(""));
        assert_eq!(before_name_call("Alright, fly Corvus."), Some("Alright"));
        for text in ["Hey Corvus", "Thanks, Corvus.", "Good night, Corvus.", "I was telling her about Corvus.", "What time is my flight, Corvus?", "Who won the fight, Corvus.", "Why so quiet, Corvus?", "Fly, Corvus?", "Corvus", "Corvus, check the printer.", "Fly, Corvids.", ""] {
            assert_eq!(before_name_call(text), None, "{text}");
        }
    }

    #[test]
    fn pleasantries_closings_and_stray_sounds_stay_lives_and_every_other_turn_goes_to_the_hub() {
        for text in ["Hi there.", "Good morning, Corvus.", "Okay, goodbye.", "That\u{2019}ll be all for now.", "Um.", "So, uh", "Yes.", "Quiet, Corvus. Thanks, Corvus.", "Okay, that's all, thanks."] {
            assert!(!for_hub(text, None), "{text}");
        }
        for text in ["I'm curious about our compression experiments.", "Dinner is ready, come wash your hands.", "Hey, can you check the build?", "Why won't my laptop go to sleep?", "Remind me to say goodbye to the neighbours."] {
            assert!(for_hub(text, None), "{text}");
        }
        assert!(for_hub("Yes.", Some(1)));
        assert!(!for_hub("Okay, thanks.", Some(1)));
        assert!(!for_hub("Hm.", Some(1)));
        assert!(for_hub("Hm.", Some(0)));
        assert!(!for_hub("Goodbye.", Some(0)));
        assert!(closes("That'll be all\nfor now."));
        assert!(!closes("Check the printer and the backup, then goodbye."));
        assert!(!closes("for now."));
    }

    #[test]
    fn a_trailing_hand_off_phrase_is_dropped_with_the_commas_before_it() {
        let phrase = &identity().hand_off_phrase;
        assert_eq!(phrase, "Fly, Corvus");
        assert!(identity().instructions.contains(&format!("\"{phrase}\"")));
        assert_eq!(without_trailing_phrase("Check the printer. Okay, fly corvus!", phrase), "Check the printer. Okay");
        assert_eq!(without_trailing_phrase("Fly, Corvus.", phrase), "");
        assert_eq!(without_trailing_phrase("Fly, Corvus, check the printer.", phrase), "Fly, Corvus, check the printer.");
        assert_eq!(without_trailing_phrase("Corvus", phrase), "Corvus");
        assert_eq!(without_trailing_phrase("Compare the evalFly Corvus", phrase), "Compare the eval");
    }
}
