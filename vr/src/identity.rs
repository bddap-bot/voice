use std::sync::OnceLock;
use std::time::Duration;

const SOURCE: &str = include_str!("../../docs/identity.js");

/// The page's identity constants, read from its own module so the two never drift.
pub struct Identity {
    pub sign_off: String,
    pub inactivity: Duration,
    pub instructions: String,
    pub woken: String,
    pub vr_wake_phrase: String,
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
        let names = [("NAME", name.as_str()), ("SIGN_OFF", sign_off.as_str())];
        Identity {
            instructions: text("IDENTITY", &names),
            woken: text("WOKEN", &names),
            vr_wake_phrase: text("VR_WAKE_PHRASE", &names),
            inactivity: Duration::from_millis(milliseconds("INACTIVITY_MS")),
            sign_off,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_identity_parses_with_its_constants_interpolated() {
        let identity = identity();
        assert!(SOURCE.contains(&format!("export const SIGN_OFF = '{}';", identity.sign_off)));
        assert_eq!(identity.inactivity, Duration::from_secs(600));
        assert!(identity.instructions.contains(&format!("\"{}\"", identity.sign_off)));
        assert!(identity.woken.starts_with("Context: ") && !identity.woken.contains("${"));
    }

    #[test]
    fn a_phrase_matches_across_case_punctuation_and_split_deltas() {
        let sign_off = &identity().sign_off;
        assert!(includes_phrase(&format!("Sure. {}", sign_off.to_uppercase().replace(' ', "  ")), sign_off));
        assert!(includes_phrase("my labor here, is ENDED!", "My labor here is ended."));
        assert!(!includes_phrase("my labor here is", "My labor here is ended."));
    }
}
