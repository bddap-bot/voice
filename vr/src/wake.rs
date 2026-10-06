use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use pulseaudio::protocol;
use serde::Deserialize;
use tract_onnx::prelude::*;

use crate::identity::identity;

const RATE: u32 = 16_000;
const CHUNK: usize = 1280;
const BACKLOG: usize = 8;
const WINDOW: usize = 16;
const WIDTH: usize = 96;
const CONTEXT: usize = 480;
const BINS: usize = 32;
const FRAMES: usize = 76;
const REFRACTORY: u32 = 25;
const MEL: &[u8] = include_bytes!("../../docs/wake/melspectrogram.onnx");
const EMBEDDING: &[u8] = include_bytes!("../../docs/wake/embedding_model.onnx");

type Plan = TypedSimplePlan<TypedModel>;

struct Models {
    mel: Plan,
    embed: Plan,
}

fn plan(bytes: &[u8], shape: &[usize]) -> TractResult<Plan> {
    tract_onnx::onnx().model_for_read(&mut &bytes[..])?.with_input_fact(0, f32::fact(shape).into())?.into_optimized()?.into_runnable()
}

fn models() -> Result<&'static Models, String> {
    static MODELS: OnceLock<Result<Models, String>> = OnceLock::new();
    MODELS
        .get_or_init(|| {
            let failed = |error: TractError| format!("wake models: {error}");
            Ok(Models { mel: plan(MEL, &[1, CONTEXT + CHUNK]).map_err(failed)?, embed: plan(EMBEDDING, &[1, FRAMES, BINS, 1]).map_err(failed)? })
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn run(plan: &Plan, shape: &[usize], input: &[f32]) -> Result<Vec<f32>, String> {
    let failed = |error: TractError| format!("wake models: {error}");
    let output = plan.run(tvec!(Tensor::from_shape(shape, input).map_err(failed)?.into())).map_err(failed)?;
    Ok(output[0].as_slice::<f32>().map_err(failed)?.to_vec())
}

struct Features {
    audio: Vec<f32>,
    frames: Vec<f32>,
    filled: usize,
    embeddings: Vec<f32>,
    count: usize,
}

impl Features {
    fn new() -> Features {
        Features { audio: vec![0.0; CONTEXT + CHUNK], frames: vec![0.0; FRAMES * BINS], filled: 0, embeddings: vec![0.0; WINDOW * WIDTH], count: 0 }
    }

    fn push(&mut self, chunk: &[f32]) -> Result<Option<&[f32]>, String> {
        let models = models()?;
        self.audio.copy_within(CHUNK.., 0);
        for (slot, sample) in self.audio[CONTEXT..].iter_mut().zip(chunk) {
            *slot = sample * 32767.0;
        }
        let raw = run(&models.mel, &[1, CONTEXT + CHUNK], &self.audio)?;
        let added = raw.len() / BINS;
        self.frames.copy_within(added * BINS.., 0);
        for (slot, value) in self.frames[(FRAMES - added) * BINS..].iter_mut().zip(&raw) {
            *slot = value / 10.0 + 2.0;
        }
        self.filled = FRAMES.min(self.filled + added);
        if self.filled < FRAMES {
            return Ok(None);
        }
        let embedding = run(&models.embed, &[1, FRAMES, BINS, 1], &self.frames)?;
        self.embeddings.copy_within(WIDTH.., 0);
        self.embeddings[(WINDOW - 1) * WIDTH..].copy_from_slice(&embedding);
        self.count = WINDOW.min(self.count + 1);
        Ok((self.count == WINDOW).then_some(&self.embeddings[..]))
    }
}

#[derive(Deserialize)]
struct Stored {
    phrase: String,
    threshold: f32,
    mean: String,
    scale: String,
    w1: String,
    b1: String,
    w2: String,
    b2: f32,
}

pub struct Head {
    threshold: f32,
    mean: Vec<f32>,
    scale: Vec<f32>,
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: f32,
}

fn floats(encoded: &str) -> Vec<f32> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap_or_default();
    bytes.chunks_exact(4).map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap())).collect()
}

impl Head {
    pub fn parse(bytes: &[u8], phrase: &str) -> Result<Head, String> {
        let stored: Stored = serde_json::from_slice(bytes).map_err(|error| format!("invalid wake model: {error}"))?;
        if stored.phrase != phrase {
            return Err(format!("the wake model listens for {:?}, not {phrase:?}", stored.phrase));
        }
        let head = Head { threshold: stored.threshold, mean: floats(&stored.mean), scale: floats(&stored.scale), w1: floats(&stored.w1), b1: floats(&stored.b1), w2: floats(&stored.w2), b2: stored.b2 };
        let inputs = WINDOW * WIDTH;
        let sized = head.mean.len() == inputs && head.scale.len() == inputs && !head.b1.is_empty() && head.w1.len() == head.b1.len() * inputs && head.w2.len() == head.b1.len();
        if !sized || !head.b2.is_finite() || !(head.threshold > 0.0 && head.threshold < 1.0) {
            return Err("invalid wake model".into());
        }
        Ok(head)
    }

    fn score(&self, window: &[f32]) -> f32 {
        let mut logit = self.b2;
        for (unit, (bias, weight)) in self.b1.iter().zip(&self.w2).enumerate() {
            let row = &self.w1[unit * window.len()..(unit + 1) * window.len()];
            let sum = bias + row.iter().zip(window).zip(self.mean.iter().zip(&self.scale)).map(|((w, x), (mean, scale))| w * (x - mean) * scale).sum::<f32>();
            if sum > 0.0 {
                logit += weight * sum;
            }
        }
        1.0 / (1.0 + (-logit).exp())
    }
}

#[cfg_attr(test, derive(Debug, PartialEq))]
pub enum Heard {
    Wake(f32),
    Miss(f32),
}

struct Decision {
    threshold: f32,
    rest: u32,
    peak: f32,
}

impl Decision {
    fn new(threshold: f32) -> Decision {
        Decision { threshold, rest: 0, peak: 0.0 }
    }

    fn decide(&mut self, score: f32) -> Option<Heard> {
        if self.rest > 0 {
            self.rest -= 1;
            return None;
        }
        if score >= self.threshold {
            self.rest = REFRACTORY;
            self.peak = 0.0;
            return Some(Heard::Wake(score));
        }
        if score >= self.threshold / 2.0 {
            self.peak = self.peak.max(score);
            return None;
        }
        (self.peak > 0.0).then(|| Heard::Miss(std::mem::take(&mut self.peak)))
    }
}

/// The overlay's own wake phrase, never the page's, so one phrase wakes one of them.
pub fn phrase() -> &'static str {
    &identity().vr_wake_phrase
}

pub struct Spotter {
    stop: Arc<AtomicBool>,
    heard: Receiver<Result<Heard, String>>,
}

impl Spotter {
    pub fn start(head: Arc<Head>) -> Spotter {
        let stop = Arc::new(AtomicBool::new(false));
        let (send, heard) = mpsc::channel();
        let stopped = stop.clone();
        std::thread::spawn(move || {
            if let Err(error) = listen(&head, &stopped, &send) {
                let _ = send.send(Err(error));
            }
        });
        Spotter { stop, heard }
    }

    pub fn heard(&self) -> Option<Result<Heard, String>> {
        match self.heard.try_recv() {
            Ok(heard) => Some(heard),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the wake spotter stopped".into())),
        }
    }
}

impl Drop for Spotter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Holds the microphone until `stop`; a backlog beyond `BACKLOG` blocks is dropped so a wake is never heard late.
fn listen(head: &Head, stop: &AtomicBool, heard: &Sender<Result<Heard, String>>) -> Result<(), String> {
    let client = pulseaudio::Client::from_env(c"voice-vr-wake").map_err(|error| format!("sound server: {error}"))?;
    let (samples, audio) = mpsc::sync_channel::<Vec<f32>>(BACKLOG);
    let write = move |data: &[u8]| {
        let _ = samples.try_send(data.chunks_exact(4).map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())).collect());
    };
    let params = protocol::RecordStreamParams {
        sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::Float32Le, channels: 1, sample_rate: RATE },
        channel_map: protocol::ChannelMap::mono(),
        source_name: Some(protocol::DEFAULT_SOURCE.to_owned()),
        buffer_attr: protocol::stream::BufferAttr { fragment_size: (CHUNK * 4) as u32, ..Default::default() },
        flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
        ..Default::default()
    };
    let capture = futures::executor::block_on(client.create_record_stream(params, write)).map_err(|error| format!("microphone: {error}"))?;
    let mut features = Features::new();
    let mut decision = Decision::new(head.threshold);
    let mut pending = Vec::new();
    let (mut chunks, mut busy, mut since) = (0u32, Duration::ZERO, Instant::now());
    while !stop.load(Ordering::Relaxed) {
        match audio.recv_timeout(Duration::from_millis(200)) {
            Ok(block) => pending.extend(block),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Err("the microphone stream closed".into()),
        }
        while pending.len() >= CHUNK {
            let chunk: Vec<f32> = pending.drain(..CHUNK).collect();
            let started = Instant::now();
            let event = features.push(&chunk)?.and_then(|window| decision.decide(head.score(window)));
            busy += started.elapsed();
            chunks += 1;
            if since.elapsed() >= Duration::from_secs(60) {
                eprintln!("wake spotter: {:.1} ms of work per 80 ms chunk", busy.as_secs_f64() * 1e3 / chunks as f64);
                (chunks, busy, since) = (0, Duration::ZERO, Instant::now());
            }
            if let Some(event) = event {
                if heard.send(Ok(event)).is_err() {
                    return Ok(());
                }
            }
        }
    }
    futures::executor::block_on(capture.delete()).map_err(|error| format!("microphone release: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{includes_phrase, page_wake_phrase};
    use serde_json::Value;

    fn golden() -> Value {
        serde_json::from_str(include_str!("../golden/wake.json")).unwrap()
    }

    fn numbers(value: &Value) -> Vec<f32> {
        value.as_array().unwrap().iter().map(|number| number.as_f64().unwrap() as f32).collect()
    }

    #[test]
    fn the_overlay_and_the_page_listen_for_different_phrases() {
        let page = &page_wake_phrase();
        assert!(!includes_phrase(phrase(), page) && !includes_phrase(page, phrase()), "{page:?} / {:?}", phrase());
    }

    #[test]
    fn a_model_for_another_phrase_is_refused() {
        let demo = include_bytes!("../../scripts/wake-demo.json");
        assert!(Head::parse(demo, &page_wake_phrase()).is_ok());
        assert!(Head::parse(demo, phrase()).err().unwrap().contains("listens for"));
        assert!(Head::parse(br#"{"phrase":"x","threshold":0.5,"mean":"","scale":"","w1":"","b1":"","w2":"","b2":0}"#, "x").is_err());
    }

    #[test]
    fn features_scores_and_decisions_match_the_page_s() {
        let golden = golden();
        let pcm = base64::engine::general_purpose::STANDARD.decode(golden["audio"].as_str().unwrap()).unwrap();
        let audio: Vec<f32> = pcm.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32767.0).collect();
        let head = Head::parse(include_bytes!("../../scripts/wake-demo.json"), &page_wake_phrase()).unwrap();
        let mut features = Features::new();
        let mut decision = Decision::new(head.threshold);
        let (mut embeddings, mut scores, mut events) = (Vec::new(), Vec::new(), Vec::new());
        for (index, chunk) in audio.chunks_exact(CHUNK).enumerate() {
            if let Some(window) = features.push(chunk).unwrap() {
                embeddings.extend(window[(WINDOW - 1) * WIDTH..].iter().step_by(7).copied());
                let score = head.score(window);
                scores.push(score);
                match decision.decide(score) {
                    Some(Heard::Wake(_)) => events.push((index, "wake")),
                    Some(Heard::Miss(_)) => events.push((index, "miss")),
                    None => {}
                }
            }
        }
        let expected = numbers(&golden["embeddings"]);
        assert_eq!(embeddings.len(), expected.len());
        let worst = embeddings.iter().zip(&expected).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 2e-3, "embedding error {worst}");
        let expected = numbers(&golden["scores"]);
        assert_eq!(scores.len(), expected.len());
        let worst = scores.iter().zip(&expected).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-3, "score error {worst}");
        let expected: Vec<(usize, &str)> = golden["events"].as_array().unwrap().iter().map(|event| (event["chunk"].as_u64().unwrap() as usize, event["kind"].as_str().unwrap())).collect();
        assert_eq!(events, expected);
    }

    #[test]
    fn a_wake_rests_before_the_next_and_a_rising_miss_reports_its_peak() {
        let mut decision = Decision::new(0.9);
        assert_eq!(decision.decide(0.5), None);
        assert_eq!(decision.decide(0.7), None);
        assert_eq!(decision.decide(0.1), Some(Heard::Miss(0.7)));
        assert_eq!(decision.decide(0.95), Some(Heard::Wake(0.95)));
        for _ in 0..REFRACTORY {
            assert_eq!(decision.decide(0.99), None);
        }
        assert_eq!(decision.decide(0.99), Some(Heard::Wake(0.99)));
    }
}
