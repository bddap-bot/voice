use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Deserialize;
use tract_onnx::prelude::*;
use unicode_normalization::UnicodeNormalization;

/// docs/puppet-drivers.js as its golden records it: the labels and posture override the page classifies with.
const PAGE: &str = include_str!("../golden/actions.json");
/// The page driver's `minimumMs` between two actions.
const MINIMUM: Duration = Duration::from_millis(900);
const MAX_TOKENS: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    None,
    Gesture,
    Mood,
}

#[derive(Deserialize)]
struct Label {
    kind: Kind,
    name: String,
    examples: Vec<String>,
}

#[derive(Deserialize)]
struct Posture {
    source: String,
    flags: String,
}

#[derive(Deserialize)]
struct Page {
    labels: Vec<Label>,
    posture: Posture,
}

fn page() -> &'static Page {
    static PARSED: OnceLock<Page> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(PAGE).expect("vr/golden/actions.json"))
}

/// The gestures the page can play; each is a clip of the same name in the catalog.
pub fn gestures() -> impl Iterator<Item = &'static str> {
    page().labels.iter().filter(|label| label.kind == Kind::Gesture).map(|label| label.name.as_str())
}

/// One decision, as the page's driver hands it to `applyTranscriptAction`.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub kind: Kind,
    pub name: String,
    pub score: f32,
    pub sentence: String,
}

/// The page's BERT tokenizer for the embedding model: BertNormalizer, BertPreTokenizer, WordPiece, then `[CLS] … [SEP]`.
struct Tokenizer {
    vocab: HashMap<String, i64>,
    words: Regex,
    unknown: i64,
    first: i64,
    last: i64,
}

#[derive(Deserialize)]
struct TokenizerFile {
    model: WordPiece,
}

#[derive(Deserialize)]
struct WordPiece {
    vocab: HashMap<String, i64>,
}

const PUNCTUATION: &str = r"\p{P}\x21-\x2F\x3A-\x40\x5B-\x60\x7B-\x7E";

impl Tokenizer {
    fn parse(bytes: &[u8]) -> Result<Tokenizer, String> {
        let file: TokenizerFile = serde_json::from_slice(bytes).map_err(|error| format!("tokenizer: {error}"))?;
        let id = |token: &str| file.model.vocab.get(token).copied().ok_or(format!("tokenizer: no {token}"));
        Ok(Tokenizer { unknown: id("[UNK]")?, first: id("[CLS]")?, last: id("[SEP]")?, words: Regex::new(&format!(r"[^\s{PUNCTUATION}]+|[{PUNCTUATION}]")).unwrap(), vocab: file.model.vocab })
    }

    fn normalize(text: &str) -> String {
        static CONTROL: OnceLock<Regex> = OnceLock::new();
        let control = CONTROL.get_or_init(|| Regex::new(r"^[\p{Cc}\p{Cf}\p{Co}]$").unwrap());
        let mut clean = String::new();
        for char in text.chars() {
            let mut buffer = [0; 4];
            let single = char.encode_utf8(&mut buffer);
            if char == '\0' || char == '\u{FFFD}' || (!matches!(char, '\t' | '\n' | '\r') && control.is_match(single)) {
                continue;
            }
            if char.is_whitespace() {
                clean.push(' ');
            } else if matches!(char as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF) {
                clean.extend([' ', char, ' ']);
            } else {
                clean.push(char);
            }
        }
        clean.to_lowercase().nfd().filter(|char| !is_nonspacing(*char)).collect()
    }

    fn ids(&self, text: &str) -> Vec<i64> {
        let normal = Self::normalize(text);
        let mut ids = vec![self.first];
        for word in self.words.find_iter(normal.trim()).map(|found| found.as_str()) {
            ids.extend(self.pieces(word));
        }
        ids.truncate(MAX_TOKENS - 1);
        ids.push(self.last);
        ids
    }

    fn pieces(&self, word: &str) -> Vec<i64> {
        let chars: Vec<char> = word.chars().collect();
        if chars.len() > 100 {
            return vec![self.unknown];
        }
        let mut pieces = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let found = (start + 1..=chars.len()).rev().find_map(|end| {
                let piece: String = chars[start..end].iter().collect();
                let piece = if start > 0 { format!("##{piece}") } else { piece };
                self.vocab.get(&piece).map(|&id| (id, end))
            });
            let Some((id, end)) = found else { return vec![self.unknown] };
            pieces.push(id);
            start = end;
        }
        pieces
    }
}

fn is_nonspacing(char: char) -> bool {
    static MARK: OnceLock<Regex> = OnceLock::new();
    let mut buffer = [0; 4];
    MARK.get_or_init(|| Regex::new(r"^\p{Mn}$").unwrap()).is_match(char.encode_utf8(&mut buffer))
}

type Plan = TypedSimplePlan<TypedModel>;

/// Plans kept for recent token counts; each costs about 13 MB.
const PLANS: usize = 4;

/// The page's embedder: the same quantized sentence model, mean-pooled and normalized, one text at a time.
/// The model's integer kernels are only planned for a fixed token count, so each count gets its own plan.
struct Embedder {
    tokenizer: Tokenizer,
    model: TypedModel,
    inputs: Vec<String>,
    plans: Mutex<VecDeque<(usize, Arc<Plan>)>>,
}

impl Embedder {
    fn load(directory: &Path) -> Result<Embedder, String> {
        let read = |name: &str| std::fs::read(directory.join(name)).map_err(|error| format!("{}: {error}", directory.join(name).display()));
        let tokenizer = Tokenizer::parse(&read("tokenizer.json")?)?;
        let failed = |error: TractError| format!("action model: {error}");
        let model = tract_onnx::onnx().model_for_read(&mut &read("model_quantized.onnx")?[..]).and_then(|model| model.into_typed()?.into_decluttered()).map_err(failed)?;
        let inputs = model.input_outlets().map_err(failed)?.iter().map(|outlet| model.node(outlet.node).name.clone()).collect();
        Ok(Embedder { tokenizer, model, inputs, plans: Mutex::default() })
    }

    fn plan(&self, count: usize) -> Result<Arc<Plan>, String> {
        let mut plans = self.plans.lock().unwrap();
        if let Some(at) = plans.iter().position(|(known, _)| *known == count) {
            let entry = plans.remove(at).unwrap();
            plans.push_front(entry);
            return Ok(plans[0].1.clone());
        }
        let failed = |error: TractError| format!("action model: {error}");
        let values = SymbolValues::default().with(&self.model.symbols.sym("batch_size"), 1).with(&self.model.symbols.sym("sequence_length"), count as i64);
        let plan = Arc::new(self.model.concretize_dims(&values).and_then(|model| model.into_optimized()?.into_runnable()).map_err(failed)?);
        plans.push_front((count, plan.clone()));
        plans.truncate(PLANS);
        Ok(plan)
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        self.embed_ids(&self.tokenizer.ids(text))
    }

    fn embed_ids(&self, ids: &[i64]) -> Result<Vec<f32>, String> {
        let failed = |error: TractError| format!("action model: {error}");
        let count = ids.len();
        let inputs: TVec<TValue> = self
            .inputs
            .iter()
            .map(|name| {
                let values = match name.as_str() {
                    "input_ids" => ids.to_vec(),
                    "attention_mask" => vec![1; count],
                    _ => vec![0; count],
                };
                Tensor::from_shape(&[1, count], &values).map(TValue::from)
            })
            .collect::<TractResult<_>>()
            .map_err(failed)?;
        let output = self.plan(count)?.run(inputs).map_err(failed)?;
        let hidden = output[0].as_slice::<f32>().map_err(failed)?;
        let width = hidden.len() / count;
        let mut mean = vec![0.0f32; width];
        for token in hidden.chunks_exact(width) {
            for (sum, value) in mean.iter_mut().zip(token) {
                *sum += value / count as f32;
            }
        }
        let norm = mean.iter().map(|value| value * value).sum::<f32>().sqrt().max(1e-12);
        Ok(mean.into_iter().map(|value| value / norm).collect())
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        aa += x * x;
        bb += y * y;
    }
    dot / (aa * bb).sqrt()
}

/// The page's EmbeddingActionClassifier: the label whose examples' mean embedding lies nearest the sentence.
pub struct Classifier {
    embedder: Embedder,
    posture: Regex,
    centroids: Vec<(Kind, &'static str, Vec<f32>)>,
}

impl Classifier {
    pub fn load(directory: &Path) -> Result<Classifier, String> {
        let embedder = Embedder::load(directory)?;
        let posture = &page().posture;
        let flags = if posture.flags.contains('i') { "(?i)" } else { "" };
        let posture = Regex::new(&format!("{flags}{}", posture.source)).map_err(|error| format!("the page's posture pattern: {error}"))?;
        let examples: Vec<(usize, Vec<i64>)> = page().labels.iter().enumerate().flat_map(|(label, entry)| entry.examples.iter().map(move |example| (label, example))).map(|(label, example)| (label, embedder.tokenizer.ids(example))).collect();
        let mut order: Vec<usize> = (0..examples.len()).collect();
        order.sort_by_key(|&at| examples[at].1.len());
        let mut vectors = vec![Vec::new(); examples.len()];
        for at in order {
            vectors[at] = embedder.embed_ids(&examples[at].1)?;
        }
        let mut centroids = Vec::new();
        for (index, label) in page().labels.iter().enumerate() {
            let own: Vec<&Vec<f32>> = examples.iter().zip(&vectors).filter(|((owner, _), _)| *owner == index).map(|(_, vector)| vector).collect();
            let centroid = (0..own[0].len()).map(|at| own.iter().map(|vector| vector[at]).sum::<f32>() / own.len() as f32).collect();
            centroids.push((label.kind, label.name.as_str(), centroid));
        }
        Ok(Classifier { embedder, posture, centroids })
    }

    pub fn classify(&self, text: &str) -> Result<(Kind, &'static str, f32), String> {
        if self.posture.is_match(text) {
            return Ok((Kind::None, "neutral", 1.0));
        }
        let vector = self.embedder.embed(text)?;
        let mut best: Option<(Kind, &'static str, f32)> = None;
        for (kind, name, centroid) in &self.centroids {
            let score = cosine(&vector, centroid);
            if best.map_or(true, |(_, _, top)| score > top) {
                best = Some((*kind, name, score));
            }
        }
        best.ok_or_else(|| "no labels".into())
    }
}

/// The page's TranscriptActionDriver over Live's output transcript: completed sentences.
#[derive(Default)]
struct Driver {
    pending: String,
}

fn completed_sentences(text: &str) -> (Vec<&str>, &str) {
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, char)) = chars.next() {
        if matches!(char, '.' | '!' | '?') {
            let mut end = at + 1;
            while let Some(&(next, '.' | '!' | '?')) = chars.peek() {
                end = next + 1;
                chars.next();
            }
            sentences.push(&text[start..end]);
            start = end;
        }
    }
    (sentences, &text[start..])
}

impl Driver {
    /// Takes one output delta; returns the sentences it completed.
    fn push(&mut self, delta: &str) -> Vec<String> {
        self.pending.push_str(delta);
        let pending = std::mem::take(&mut self.pending);
        let (sentences, rest) = completed_sentences(&pending);
        let sentences = sentences.into_iter().filter(|sentence| !sentence.chars().all(|char| matches!(char, '.' | '!' | '?') || char.is_whitespace())).map(|sentence| sentence.trim().to_owned()).collect();
        self.pending = rest.to_owned();
        sentences
    }
}

/// The page's actions for the overlay: the driver here, the classifier on its own thread, decisions in order and spaced as the page spaces them.
pub struct Actions {
    driver: Driver,
    epoch: u64,
    requests: Sender<(u64, String)>,
    decisions: Receiver<(u64, Action)>,
    due: VecDeque<Action>,
    next: Option<Instant>,
}

fn serve(load: impl FnOnce() -> Result<Classifier, String>, requests: Receiver<(u64, String)>, decisions: Sender<(u64, Action)>) {
    let mut classifier = None;
    let mut load = Some(load);
    for (epoch, sentence) in requests {
        if let Some(load) = load.take() {
            let started = Instant::now();
            match load() {
                Ok(loaded) => {
                    eprintln!("action classifier loaded in {:.1} s", started.elapsed().as_secs_f32());
                    classifier = Some(loaded);
                }
                Err(error) => eprintln!("speech-driven actions are off: {error}"),
            }
        }
        let Some(classifier) = &classifier else { continue };
        let action = match classifier.classify(&sentence) {
            Ok((kind, name, score)) => Action { kind, name: name.to_owned(), score, sentence },
            Err(error) => {
                eprintln!("{error}");
                continue;
            }
        };
        if decisions.send((epoch, action)).is_err() {
            return;
        }
    }
}

/// Where the bundle or the build put the page's embedding model and its tokenizer.
fn model_directory() -> Result<PathBuf, String> {
    std::env::var_os("VOICE_VR_ACTION_MODEL").map(PathBuf::from).ok_or_else(|| "VOICE_VR_ACTION_MODEL is not set".into())
}

impl Actions {
    pub fn start() -> Actions {
        Actions::with(|| Classifier::load(&model_directory()?))
    }

    fn with(load: impl FnOnce() -> Result<Classifier, String> + Send + 'static) -> Actions {
        let (requests, served) = mpsc::channel();
        let (decided, decisions) = mpsc::channel();
        std::thread::spawn(move || serve(load, served, decided));
        Actions { driver: Driver::default(), epoch: 0, requests, decisions, due: VecDeque::new(), next: None }
    }

    /// One output transcript delta.
    pub fn spoke(&mut self, delta: &str) {
        for sentence in self.driver.push(delta) {
            let _ = self.requests.send((self.epoch, sentence));
        }
    }

    /// The conversation ended: pending decisions are dropped, as the page's reset drops them.
    pub fn reset(&mut self) {
        self.epoch += 1;
        self.driver = Driver::default();
        self.due.clear();
        self.next = None;
    }

    /// Decisions ready to act on now, at most one per MINIMUM.
    pub fn take(&mut self, now: Instant) -> Option<Action> {
        let epoch = self.epoch;
        self.due.extend(self.decisions.try_iter().filter(|(at, _)| *at == epoch).map(|(_, action)| action));
        if self.next.is_some_and(|next| now < next) {
            return None;
        }
        let action = self.due.pop_front()?;
        self.next = Some(now + MINIMUM);
        Some(action)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Case {
        text: String,
        ids: Vec<i64>,
        kind: Kind,
        name: String,
        score: f32,
        margin: f32,
    }

    #[derive(Deserialize)]
    struct Applied {
        sentence: String,
    }

    #[derive(Deserialize)]
    pub struct Replay {
        pub events: Vec<String>,
        actions: Vec<Applied>,
    }

    #[derive(Deserialize)]
    struct Golden {
        cases: Vec<Case>,
        replays: Vec<Replay>,
    }

    fn golden() -> Golden {
        serde_json::from_str(PAGE).unwrap()
    }

    pub fn replays() -> Vec<Replay> {
        golden().replays
    }

    pub fn classifier() -> &'static Classifier {
        static LOADED: OnceLock<Classifier> = OnceLock::new();
        LOADED.get_or_init(|| Classifier::load(&model_directory().unwrap()).unwrap())
    }

    #[test]
    fn the_tokenizer_matches_the_pages() {
        let directory = model_directory().unwrap();
        let tokenizer = Tokenizer::parse(&std::fs::read(directory.join("tokenizer.json")).unwrap()).unwrap();
        for case in golden().cases {
            assert_eq!(tokenizer.ids(&case.text), case.ids, "{}", case.text);
        }
    }

    /// The overlay's kernels and onnxruntime's round the quantized model differently, moving a score by up to about 0.012;
    /// a sentence the page itself decided by less than twice that may fall either way.
    const DRIFT: f32 = 0.015;

    fn agrees(case: &Case, kind: Kind, name: &str, score: f32) -> bool {
        (score - case.score).abs() < DRIFT && ((kind, name) == (case.kind, case.name.as_str()) || case.margin < 2.0 * DRIFT)
    }

    #[test]
    fn the_classifier_decides_as_the_page_on_the_same_model() {
        let cases = golden().cases;
        let mut flipped = Vec::new();
        for case in &cases {
            let (kind, name, score) = classifier().classify(&case.text).unwrap();
            assert!(agrees(case, kind, name, score), "{}: {kind:?} {name} {score} where the page decided {:?} {} {}", case.text, case.kind, case.name, case.score);
            if (kind, name) != (case.kind, case.name.as_str()) {
                flipped.push(&case.text);
            }
        }
        assert!(flipped.len() * 20 <= cases.len(), "near-ties only, and few: {flipped:?}");
    }

    /// Runs a recorded transcript through the overlay's actions; every decision is returned in order.
    pub fn replay(replay: &Replay) -> Vec<Action> {
        let mut actions = Actions::with(|| Classifier::load(&model_directory()?));
        let mut decided = Vec::new();
        let mut now = Instant::now();
        for delta in &replay.events {
            actions.spoke(delta);
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        let expected = replay.actions.len();
        while decided.len() < expected && Instant::now() < deadline {
            now += MINIMUM;
            match actions.take(now) {
                Some(action) => decided.push(action),
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        decided
    }

    #[test]
    fn recorded_transcripts_act_as_the_page_acted_on_them() {
        for replay in golden().replays {
            let decided = super::tests::replay(&replay);
            let cases = golden().cases;
            assert_eq!(decided.len(), replay.actions.len(), "{decided:?}");
            for (action, applied) in decided.iter().zip(&replay.actions) {
                assert_eq!(action.sentence, applied.sentence);
                assert!(agrees(cases.iter().find(|case| case.text == applied.sentence).unwrap(), action.kind, &action.name, action.score), "{action:?}");
            }
        }
    }

    #[test]
    fn decisions_are_spaced_by_the_pages_minimum_and_a_reset_drops_them() {
        let directory = model_directory().unwrap();
        let mut actions = Actions::with(move || Classifier::load(&directory));
        let decided = |actions: &mut Actions, now: Instant| {
            let deadline = Instant::now() + Duration::from_secs(120);
            while Instant::now() < deadline {
                if let Some(action) = actions.take(now) {
                    return Some(action.sentence);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            None
        };
        actions.spoke("Yes. No idea. Hm");
        let start = Instant::now() + Duration::from_secs(1);
        assert_eq!(decided(&mut actions, start), Some("Yes.".into()));
        assert_eq!(actions.take(start + MINIMUM / 2), None);
        assert_eq!(decided(&mut actions, start + MINIMUM), Some("No idea.".into()));
        actions.spoke("m.");
        actions.reset();
        actions.spoke("Okay.");
        assert_eq!(decided(&mut actions, start + MINIMUM * 3), Some("Okay.".into()), "a reset drops a decision still in flight");
    }

    #[test]
    fn the_page_reaches_a_wave_and_every_gesture_has_a_name() {
        assert!(gestures().any(|name| name == "wave"));
    }
}
