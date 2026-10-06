use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tract_onnx::prelude::*;

const SOURCE: &str = include_str!("../../docs/speaker.js");
pub const DECIMATION: usize = crate::audio::RATE as usize / 16_000;
const TAPS: usize = 127;

/// The source text of `const NAME = …;`, or of `name: …,` inside `export const GATE = { … }`.
fn number(name: &str) -> f64 {
    let (text, end) = match SOURCE.find(&format!("const {name} = ")) {
        Some(start) => (&SOURCE[start + name.len() + 9..], ';'),
        None => {
            let gate = &SOURCE[SOURCE.find("export const GATE = {").expect("speaker.js has no GATE")..];
            let gate = &gate[..gate.find('}').expect("speaker.js: GATE is unterminated")];
            let start = gate.find(&format!(" {name}: ")).unwrap_or_else(|| panic!("speaker.js has no {name}"));
            (&gate[start + name.len() + 3..], ',')
        }
    };
    text[..text.find(end).unwrap()].trim().parse().unwrap_or_else(|_| panic!("speaker.js: {name} is not a number"))
}

fn quoted(name: &str) -> &'static str {
    let start = SOURCE.find(&format!("{name}: '")).unwrap_or_else(|| panic!("speaker.js has no {name}")) + name.len() + 3;
    &SOURCE[start..start + SOURCE[start..].find('\'').unwrap()]
}

/// The page's speaker-filter constants, read from its own module so the two never drift.
pub struct Constants {
    pub rate: usize,
    pub hop: usize,
    pub chunk: usize,
    pub threshold: f32,
    preroll: i64,
    tail: i64,
    shortest: i64,
    every: i64,
    window: i64,
    longest: i64,
    offset: usize,
    onset: usize,
    windows: usize,
    frame: usize,
    fft: usize,
    bins: usize,
    ring: usize,
    pub sha256: &'static str,
}

pub fn page() -> &'static Constants {
    static PAGE: OnceLock<Constants> = OnceLock::new();
    PAGE.get_or_init(|| {
        let count = |name| number(name) as usize;
        let frames = |name| number(name) as i64;
        Constants {
            rate: count("RATE"),
            hop: count("HOP"),
            chunk: count("CHUNK"),
            threshold: number("threshold") as f32,
            preroll: frames("preroll"),
            tail: frames("tail"),
            shortest: frames("shortest"),
            every: frames("every"),
            window: frames("window"),
            longest: frames("longest"),
            offset: count("OFFSET"),
            onset: count("ONSET"),
            windows: count("ENROLL_WINDOWS"),
            frame: count("FRAME"),
            fft: count("FFT"),
            bins: count("BINS"),
            ring: count("RING"),
            sha256: quoted("sha256"),
        }
    })
}

/// The page's Kaldi-compatible log-mel filter bank.
struct Bank {
    hamming: Vec<f32>,
    filters: Vec<(usize, Vec<f32>)>,
    cosines: Vec<f64>,
    sines: Vec<f64>,
    reversed: Vec<usize>,
}

fn mel(hz: f64) -> f64 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

fn bank() -> &'static Bank {
    static BANK: OnceLock<Bank> = OnceLock::new();
    BANK.get_or_init(|| {
        let Constants { rate, frame, fft, bins, .. } = *page();
        let hamming = (0..frame).map(|n| (0.54 - 0.46 * (2.0 * std::f64::consts::PI * n as f64 / (frame - 1) as f64).cos()) as f32).collect();
        let filters = (0..bins)
            .map(|bin| {
                let low = mel(20.0);
                let step = (mel(rate as f64 / 2.0) - low) / (bins + 1) as f64;
                let (left, center, right) = (low + bin as f64 * step, low + (bin + 1) as f64 * step, low + (bin + 2) as f64 * step);
                let weights: Vec<f32> = (0..fft / 2)
                    .map(|index| {
                        let m = mel((index * rate) as f64 / fft as f64);
                        (if m <= left || m >= right { 0.0 } else if m <= center { (m - left) / (center - left) } else { (right - m) / (right - center) }) as f32
                    })
                    .collect();
                let first = weights.iter().position(|&weight| weight > 0.0).unwrap();
                let last = weights.iter().rposition(|&weight| weight > 0.0).unwrap();
                (first, weights[first..=last].to_vec())
            })
            .collect();
        let angle = |index: usize| -2.0 * std::f64::consts::PI * index as f64 / fft as f64;
        let bits = fft.trailing_zeros();
        Bank {
            hamming,
            filters,
            cosines: (0..fft / 2).map(|index| angle(index).cos()).collect(),
            sines: (0..fft / 2).map(|index| angle(index).sin()).collect(),
            reversed: (0..fft).map(|index| index.reverse_bits() >> (usize::BITS - bits)).collect(),
        }
    })
}

/// One frame's log-mel bins into `into`; the result is its energy in decibels.
fn log_mel(samples: &[f32], into: &mut [f32]) -> f64 {
    let Constants { frame: length, fft, .. } = *page();
    let bank = bank();
    let mean = samples[..length].iter().map(|&sample| sample as f64).sum::<f64>() / length as f64;
    let mut frame: Vec<f64> = samples[..length].iter().map(|&sample| (sample as f64 - mean) * 32768.0).collect();
    let energy = samples[..length].iter().map(|&sample| sample as f64 * sample as f64).sum::<f64>();
    for index in (1..length).rev() {
        frame[index] -= 0.97 * frame[index - 1];
    }
    frame[0] -= 0.97 * frame[0];
    let mut re: Vec<f64> = bank.reversed.iter().map(|&source| if source < length { frame[source] * bank.hamming[source] as f64 } else { 0.0 }).collect();
    let mut im = vec![0.0f64; fft];
    let mut size = 2;
    while size <= fft {
        let half = size / 2;
        let stride = fft / size;
        for start in (0..fft).step_by(size) {
            for k in 0..half {
                let (cos, sin) = (bank.cosines[k * stride], bank.sines[k * stride]);
                let (a, b) = (start + k, start + k + half);
                let tr = re[b] * cos - im[b] * sin;
                let ti = re[b] * sin + im[b] * cos;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        size <<= 1;
    }
    for (slot, (first, weights)) in into.iter_mut().zip(&bank.filters) {
        let sum: f64 = weights.iter().enumerate().map(|(index, &weight)| weight as f64 * (re[first + index] * re[first + index] + im[first + index] * im[first + index])).sum();
        *slot = sum.max(f32::EPSILON as f64).ln() as f32;
    }
    10.0 * (energy / length as f64 + 1e-12).log10()
}

fn center(features: &mut [f32], count: usize) {
    let bins = page().bins;
    for bin in 0..bins {
        let mean = (0..count).map(|frame| features[frame * bins + bin] as f64).sum::<f64>() / count.max(1) as f64;
        for frame in 0..count {
            features[frame * bins + bin] = (features[frame * bins + bin] as f64 - mean) as f32;
        }
    }
}

/// The page's `fbank`: the centred features of a whole clip.
#[cfg(test)]
pub fn fbank(samples: &[f32]) -> Vec<f32> {
    let Constants { hop, frame, bins, .. } = *page();
    let count = if samples.len() < frame { 0 } else { 1 + (samples.len() - frame) / hop };
    let mut features = vec![0.0; count * bins];
    for index in 0..count {
        log_mel(&samples[index * hop..index * hop + frame], &mut features[index * bins..(index + 1) * bins]);
    }
    center(&mut features, count);
    features
}

pub fn normalized(vector: &[f32]) -> Vec<f32> {
    let norm = vector.iter().map(|&value| value as f64 * value as f64).sum::<f64>().sqrt();
    let norm = if norm == 0.0 { 1.0 } else { norm };
    vector.iter().map(|&value| (value as f64 / norm) as f32).collect()
}

pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(&a, &b)| a as f64 * b as f64).sum::<f64>() as f32
}

pub fn voiceprint(embeddings: &[Vec<f32>]) -> Vec<f32> {
    let mut sum = vec![0.0f32; embeddings[0].len()];
    for embedding in embeddings {
        for (total, value) in sum.iter_mut().zip(embedding) {
            *total += value;
        }
    }
    normalized(&sum)
}

/// The page's `SpeakerFrames`: streamed log-mel frames in a ring, counted from the first.
pub struct SpeakerFrames {
    pending: Vec<f32>,
    pub count: i64,
    mels: Vec<f32>,
}

impl SpeakerFrames {
    pub fn new() -> SpeakerFrames {
        SpeakerFrames { pending: Vec::new(), count: 0, mels: vec![0.0; page().ring * page().bins] }
    }

    /// The energies of the frames completed by these samples.
    pub fn push(&mut self, samples: &[f32]) -> Vec<f64> {
        let Constants { hop, frame, bins, ring, .. } = *page();
        self.pending.extend_from_slice(samples);
        let mut energies = Vec::new();
        let mut offset = 0;
        while offset + frame <= self.pending.len() {
            let slot = self.count as usize % ring;
            energies.push(log_mel(&self.pending[offset..offset + frame], &mut self.mels[slot * bins..(slot + 1) * bins]) as f32 as f64);
            self.count += 1;
            offset += hop;
        }
        self.pending.drain(..offset);
        energies
    }

    pub fn features(&self, from: i64, to: i64) -> (Vec<f32>, usize) {
        let Constants { bins, ring, .. } = *page();
        let from = from.max(self.count - ring as i64);
        let count = (to - from).max(0) as usize;
        let mut features = vec![0.0; count * bins];
        for frame in 0..count {
            let slot = (from as usize + frame) % ring;
            features[frame * bins..(frame + 1) * bins].copy_from_slice(&self.mels[slot * bins..(slot + 1) * bins]);
        }
        center(&mut features, count);
        (features, count)
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Detection {
    pub speech: bool,
    pub start: Option<i64>,
    pub end: Option<i64>,
}

/// The page's `SpeechDetector`.
pub struct SpeechDetector {
    heard: VecDeque<f64>,
    seen: usize,
    floor: f64,
    recent: VecDeque<bool>,
    silent: usize,
    speaking: bool,
}

impl SpeechDetector {
    pub fn new() -> SpeechDetector {
        SpeechDetector { heard: VecDeque::new(), seen: 0, floor: f64::NEG_INFINITY, recent: VecDeque::new(), silent: 0, speaking: false }
    }

    pub fn frame(&mut self, energy: f64) -> Detection {
        if energy > -100.0 {
            self.heard.push_back(energy);
            if self.heard.len() > 1000 {
                self.heard.pop_front();
            }
            if self.seen % 10 == 0 {
                let mut sorted: Vec<f64> = self.heard.iter().copied().collect();
                sorted.sort_by(f64::total_cmp);
                self.floor = sorted[sorted.len() / 50];
            }
            self.seen += 1;
        }
        let speech = energy > (self.floor + 18.0).max(-70.0);
        self.recent.push_back(speech);
        if self.recent.len() > 6 {
            self.recent.pop_front();
        }
        self.silent = if speech { 0 } else { self.silent + 1 };
        if !self.speaking && self.recent.iter().filter(|&&speech| speech).count() >= page().onset {
            self.speaking = true;
            let first = self.recent.iter().position(|&speech| speech).unwrap();
            return Detection { speech, start: Some((self.recent.len() - first) as i64), end: None };
        }
        if self.speaking && self.silent >= page().offset {
            self.speaking = false;
            return Detection { speech, start: None, end: Some(self.silent as i64) };
        }
        Detection { speech, ..Detection::default() }
    }
}

/// What the filter reports, in samples at the filter's rate.
#[derive(Debug, Clone, PartialEq)]
pub enum Report {
    Open(i64),
    Close(i64),
    Score { score: f32, from: i64, to: i64, ms: u128 },
    Progress(f32),
    Learned(Vec<f32>),
    Error(String),
}

pub type Embed<'a> = dyn FnMut(&[f32], usize) -> Result<Vec<f32>, String> + 'a;

/// The page's `SpeakerEnrollment`.
pub struct Enrollment {
    windows: usize,
    frames: SpeakerFrames,
    detector: SpeechDetector,
    speech: VecDeque<bool>,
    embeddings: Vec<Vec<f32>>,
    next: i64,
}

impl Enrollment {
    pub fn new(windows: usize) -> Enrollment {
        Enrollment { windows, frames: SpeakerFrames::new(), detector: SpeechDetector::new(), speech: VecDeque::new(), embeddings: Vec::new(), next: page().window }
    }

    /// The progress so far and, once every window is in, the voiceprint.
    pub fn push(&mut self, samples: &[f32], embed: &mut Embed) -> Result<(f32, Option<Vec<f32>>), String> {
        let window = page().window;
        let energies = self.frames.push(samples);
        let first = self.frames.count - energies.len() as i64 + 1;
        for (index, energy) in energies.into_iter().enumerate() {
            let now = first + index as i64;
            self.speech.push_back(self.detector.frame(energy).speech);
            if self.speech.len() > window as usize {
                self.speech.pop_front();
            }
            if now < self.next || self.embeddings.len() >= self.windows {
                continue;
            }
            if (self.speech.iter().filter(|&&speech| speech).count() as f64) < 0.6 * window as f64 {
                continue;
            }
            let (features, frames) = self.frames.features(now - window, now);
            self.embeddings.push(normalized(&embed(&features, frames)?));
            self.next = now + window / 2;
        }
        let done = self.embeddings.len() >= self.windows;
        Ok((self.embeddings.len() as f32 / self.windows as f32, done.then(|| voiceprint(&self.embeddings))))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Waiting,
    Open,
}

#[derive(Debug)]
struct Segment {
    start: i64,
    state: State,
    scored: bool,
    due: Option<i64>,
    end: Option<i64>,
}

/// The page's `SpeakerGate`.
pub struct Gate {
    print: Vec<f32>,
    pub frames: SpeakerFrames,
    detector: SpeechDetector,
    segments: Vec<Segment>,
}

impl Gate {
    pub fn new(print: Vec<f32>) -> Gate {
        Gate { print, frames: SpeakerFrames::new(), detector: SpeechDetector::new(), segments: Vec::new() }
    }

    /// Takes samples in and scores whatever came due, running the model in turn as the page's worker does.
    pub fn push(&mut self, samples: &[f32], embed: &mut Embed, send: &mut dyn FnMut(Report)) -> Result<(), String> {
        let energies = self.frames.push(samples);
        let first = self.frames.count - energies.len() as i64 + 1;
        for (index, energy) in energies.into_iter().enumerate() {
            let detection = self.detector.frame(energy);
            self.step(detection, first + index as i64, send);
        }
        self.decide(embed, send)
    }

    fn step(&mut self, detection: Detection, now: i64, send: &mut dyn FnMut(Report)) {
        let Constants { preroll, every, tail, shortest, hop, .. } = *page();
        if let Some(back) = detection.start {
            let start = (now - back - preroll).max(0);
            self.segments.push(Segment { start, state: State::Waiting, scored: false, due: Some(start + every), end: None });
        }
        let (Some(back), Some(segment)) = (detection.end, self.segments.last_mut()) else { return };
        if segment.end.is_some() {
            return;
        }
        let end = now - back + tail;
        segment.end = Some(end);
        if segment.state == State::Open {
            send(Report::Close(end * hop as i64));
        }
        segment.due = (segment.state == State::Waiting && !segment.scored && end - segment.start >= shortest).then_some(now);
    }

    fn decide(&mut self, embed: &mut Embed, send: &mut dyn FnMut(Report)) -> Result<(), String> {
        let Constants { window, longest, every, hop, threshold, .. } = *page();
        let hop = hop as i64;
        loop {
            self.segments.retain(|segment| segment.due.is_some());
            let now = self.frames.count;
            let Some(index) = self.segments.iter().position(|segment| segment.due.is_some_and(|due| due <= now)) else { return Ok(()) };
            let segment = &self.segments[index];
            let to = segment.end.unwrap_or(now);
            let from = segment.start.max(to - if segment.state == State::Open { window } else { longest });
            let (features, frames) = self.frames.features(from, to);
            let started = Instant::now();
            let score = similarity(&self.print, &normalized(&embed(&features, frames)?));
            send(Report::Score { score: (score * 1000.0).round() / 1000.0, from: from * hop, to: to * hop, ms: started.elapsed().as_millis() });
            let next = segment.end.is_none().then_some(self.frames.count + every);
            let segment = &mut self.segments[index];
            segment.scored = true;
            let matched = score >= threshold;
            if segment.state == State::Open && !matched {
                send(Report::Close((from + to + 1) / 2 * hop));
                segment.state = State::Waiting;
                segment.start = to;
            } else if segment.state == State::Waiting && matched {
                send(Report::Open(from * hop));
                if let Some(end) = segment.end {
                    send(Report::Close(end * hop));
                }
                segment.state = State::Open;
            }
            segment.due = next;
        }
    }
}

/// The page's `GrantedAudio`: the captured audio replayed in order, only within the spans the filter granted.
pub struct GrantedAudio {
    ring: Vec<f32>,
    written: i64,
    cursor: i64,
    grants: VecDeque<(i64, i64)>,
}

impl GrantedAudio {
    pub fn new(capacity: usize) -> GrantedAudio {
        GrantedAudio { ring: vec![0.0; capacity], written: 0, cursor: 0, grants: VecDeque::new() }
    }

    pub fn open(&mut self, from: i64) {
        self.grants.push_back((from, i64::MAX));
    }

    pub fn close(&mut self, to: i64) {
        if let Some(last) = self.grants.back_mut() {
            last.1 = last.1.min(to);
        }
    }

    pub fn write(&mut self, samples: &[f32]) {
        let length = self.ring.len() as i64;
        for &sample in samples {
            self.ring[(self.written % length) as usize] = sample;
            self.written += 1;
        }
    }

    pub fn read(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let length = self.ring.len() as i64;
        let mut at = 0;
        while at < out.len() {
            let Some(&(from, to)) = self.grants.front() else { break };
            self.cursor = self.cursor.max(from).max(self.written - length);
            if self.cursor >= to {
                self.grants.pop_front();
                continue;
            }
            let count = (to.min(self.written) - self.cursor).min((out.len() - at) as i64);
            if count <= 0 {
                break;
            }
            for index in 0..count {
                out[at + index as usize] = self.ring[((self.cursor + index) % length) as usize];
            }
            self.cursor += count;
            at += count as usize;
        }
    }
}

/// The capture rate brought down to the filter's, low-passed below the new Nyquist rate.
pub struct Decimator {
    taps: Vec<f32>,
    history: VecDeque<f32>,
    phase: usize,
}

impl Decimator {
    pub fn new() -> Decimator {
        let cutoff = 0.45 / DECIMATION as f64;
        let middle = (TAPS - 1) as f64 / 2.0;
        let raw: Vec<f64> = (0..TAPS)
            .map(|index| {
                let x = index as f64 - middle;
                let sinc = if x == 0.0 { 2.0 * cutoff } else { (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x) };
                let blackman = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / (TAPS - 1) as f64).cos() + 0.08 * (4.0 * std::f64::consts::PI * index as f64 / (TAPS - 1) as f64).cos();
                sinc * blackman
            })
            .collect();
        let gain: f64 = raw.iter().sum();
        Decimator { taps: raw.iter().map(|tap| (tap / gain) as f32).collect(), history: VecDeque::from(vec![0.0; TAPS]), phase: 0 }
    }

    pub fn push(&mut self, input: &[f32], output: &mut Vec<f32>) {
        for &sample in input {
            self.history.pop_front();
            self.history.push_back(sample);
            self.phase += 1;
            if self.phase == DECIMATION {
                self.phase = 0;
                output.push(self.history.iter().zip(&self.taps).map(|(sample, tap)| sample * tap).sum());
            }
        }
    }
}

/// The pinned speaker-embedding model, as the page's worker runs it.
pub struct Model {
    plan: TypedSimplePlan<TypedModel>,
}

impl Model {
    pub fn load(path: &std::path::Path) -> Result<Model, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let digest: String = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        if digest != page().sha256 {
            return Err(format!("{}: the speaker model failed its integrity check", path.display()));
        }
        let failed = |error: TractError| format!("speaker model: {error}");
        let mut model = tract_onnx::onnx().model_for_read(&mut &bytes[..]).map_err(failed)?;
        let frames = model.symbols.sym("T");
        model.set_input_fact(0, f32::fact([1.to_dim(), frames.to_dim(), (page().bins as i64).to_dim()]).into()).map_err(failed)?;
        Ok(Model { plan: model.into_optimized().map_err(failed)?.into_runnable().map_err(failed)? })
    }

    pub fn embed(&self, features: &[f32], frames: usize) -> Result<Vec<f32>, String> {
        let failed = |error: TractError| format!("speaker model: {error}");
        let input = Tensor::from_shape(&[1, frames, page().bins], features).map_err(failed)?;
        let output = self.plan.run(tvec!(input.into())).map_err(failed)?;
        Ok(output[0].as_slice::<f32>().map_err(failed)?.to_vec())
    }
}

/// The model the overlay was built with, loaded once it loads; `prepare` starts the load early, as the page's `prepareSpeaker` does.
pub fn model() -> Result<&'static Model, String> {
    static MODEL: OnceLock<Model> = OnceLock::new();
    static LOADING: Mutex<()> = Mutex::new(());
    let _loading = LOADING.lock().unwrap();
    if let Some(model) = MODEL.get() {
        return Ok(model);
    }
    let path = std::env::var_os("VOICE_VR_SPEAKER_MODEL").map(PathBuf::from).ok_or("VOICE_VR_SPEAKER_MODEL is not set")?;
    let started = Instant::now();
    let model = Model::load(&path)?;
    eprintln!("speaker model loaded in {:.1} s", started.elapsed().as_secs_f32());
    Ok(MODEL.get_or_init(|| model))
}

pub fn prepare() {
    std::thread::spawn(|| {
        if let Err(error) = model() {
            eprintln!("{error}");
        }
    });
}

/// What the microphone feeds: Live directly, Live through the filter for a voiceprint, or an enrollment while Live hears silence.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Mode {
    #[default]
    Open,
    Filter(Arc<[f32]>),
    Learn,
}

/// The wanted mode, set by the overlay, and what the session's filter reports back.
#[derive(Default)]
pub struct Hearing {
    mode: Mode,
    generation: u64,
    pub reports: Vec<Report>,
}

impl Hearing {
    #[cfg(test)]
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn set(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.generation += 1;
            self.reports.clear();
        }
    }
}

/// The page's speaker worker: a thread running the gate or the enrollment over chunks at the filter's rate, one model run at a time.
/// The count of chunks it has finished lets tests pace frames by work done rather than by the host's clock.
fn worker(mode: &Mode) -> (mpsc::Sender<Vec<f32>>, mpsc::Receiver<Report>, Arc<AtomicUsize>) {
    let (chunks, received) = mpsc::channel::<Vec<f32>>();
    let (send, heard) = mpsc::channel();
    let digested = Arc::new(AtomicUsize::new(0));
    let mode = mode.clone();
    let counted = digested.clone();
    std::thread::spawn(move || {
        let model = match model() {
            Ok(model) => model,
            Err(error) => return drop(send.send(Report::Error(error))),
        };
        let mut embed = |features: &[f32], frames: usize| model.embed(features, frames);
        let result = match mode {
            Mode::Filter(print) => {
                let mut gate = Gate::new(print.to_vec());
                received.iter().try_for_each(|mut chunk| {
                    let mut taken = 1;
                    chunk.extend(received.try_iter().inspect(|_| taken += 1).flatten());
                    gate.push(&chunk, &mut embed, &mut |event| drop(send.send(event)))?;
                    counted.fetch_add(taken, Ordering::Release);
                    Ok(())
                })
            }
            Mode::Learn => {
                let mut enrollment = Enrollment::new(page().windows);
                let mut reported = 0.0;
                received.iter().try_for_each(|chunk| {
                    if reported < 1.0 {
                        let (progress, learned) = enrollment.push(&chunk, &mut embed)?;
                        match learned {
                            Some(print) => drop(send.send(Report::Learned(print))),
                            None if progress > reported => drop(send.send(Report::Progress(progress))),
                            None => {}
                        }
                        reported = progress;
                    }
                    counted.fetch_add(1, Ordering::Release);
                    Ok(())
                })
            }
            Mode::Open => unreachable!("an open microphone needs no worker"),
        };
        if let Err(error) = result {
            let _ = send.send(Report::Error(error));
        }
    });
    (chunks, heard, digested)
}

struct Running {
    chunks: mpsc::Sender<Vec<f32>>,
    heard: mpsc::Receiver<Report>,
    decimator: Decimator,
    chunk: Vec<f32>,
    granted: GrantedAudio,
    failed: bool,
    #[cfg(test)]
    sent: usize,
    #[cfg(test)]
    digested: Arc<AtomicUsize>,
}

/// The session side of the filter: what Live hears of each captured frame.
pub struct Ear {
    hearing: Arc<Mutex<Hearing>>,
    running: Option<(u64, Mode, Running)>,
}

impl Ear {
    pub fn new(hearing: Arc<Mutex<Hearing>>) -> Ear {
        Ear { hearing, running: None }
    }

    /// Starts afresh at the next frame, as the page builds a new filter for each microphone stream.
    pub fn reset(&mut self) {
        self.running = None;
    }

    pub fn hear(&mut self, frame: &mut [f32]) {
        let mut hearing = self.hearing.lock().unwrap();
        if hearing.mode == Mode::Open {
            self.running = None;
            return;
        }
        if self.running.as_ref().is_none_or(|(generation, ..)| *generation != hearing.generation) {
            let (chunks, heard, _digested) = worker(&hearing.mode);
            let granted = GrantedAudio::new(10 * crate::audio::RATE as usize);
            let running = Running {
                chunks,
                heard,
                decimator: Decimator::new(),
                chunk: Vec::new(),
                granted,
                failed: false,
                #[cfg(test)]
                sent: 0,
                #[cfg(test)]
                digested: _digested,
            };
            self.running = Some((hearing.generation, hearing.mode.clone(), running));
        }
        let (_, mode, running) = self.running.as_mut().unwrap();
        let mut decimated = Vec::new();
        running.decimator.push(frame, &mut decimated);
        running.chunk.extend(decimated);
        if running.chunk.len() >= page().chunk {
            let _ = running.chunks.send(std::mem::take(&mut running.chunk));
            #[cfg(test)]
            {
                running.sent += 1;
            }
        }
        running.granted.write(frame);
        for event in running.heard.try_iter() {
            match &event {
                Report::Open(at) => running.granted.open(at * DECIMATION as i64),
                Report::Close(at) => running.granted.close(at * DECIMATION as i64),
                Report::Score { .. } => eprintln!("speaker {event:?}"),
                Report::Error(error) if !running.failed => {
                    eprintln!("speaker filter off, hearing everyone: {error}");
                    running.failed = true;
                }
                _ => {}
            }
            if !matches!(event, Report::Open(_) | Report::Close(_) | Report::Score { .. }) {
                hearing.reports.push(event);
            }
        }
        match mode {
            Mode::Filter(_) if running.failed => {}
            Mode::Filter(_) => running.granted.read(frame),
            _ => frame.fill(0.0),
        }
    }

    /// Blocks until the worker has finished every chunk sent so far, or has stopped.
    #[cfg(test)]
    pub fn settle(&self) {
        if let Some((_, _, running)) = &self.running {
            while running.digested.load(Ordering::Acquire) < running.sent && Arc::strong_count(&running.digested) > 1 {
                std::thread::yield_now();
            }
        }
    }
}

/// The voiceprint as the page stores it, under the model it was learned with.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Stored {
    pub model: String,
    pub print: Vec<f32>,
    #[serde(default)]
    pub off: bool,
}

impl Stored {
    pub fn read(path: &std::path::Path) -> Option<Stored> {
        serde_json::from_slice::<Stored>(&std::fs::read(path).ok()?).ok().filter(|stored| stored.model == page().sha256)
    }

    pub fn write(stored: Option<&Stored>, path: &std::path::Path) {
        let result = match stored {
            Some(stored) => {
                let temporary = path.with_extension("tmp");
                std::fs::write(&temporary, serde_json::to_vec(stored).unwrap()).and_then(|()| std::fs::rename(&temporary, path))
            }
            None => std::fs::remove_file(path).or_else(|error| if error.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(error) }),
        };
        if let Err(error) = result {
            eprintln!("voiceprint not saved: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const OWNER_HZ: f64 = 300.0;
    const OTHER_HZ: f64 = 2500.0;

    fn rate() -> f64 {
        page().rate as f64
    }

    fn tone(hz: f64, seconds: f64) -> Vec<f32> {
        (0..(seconds * rate()).round() as usize).map(|index| (0.3 * (2.0 * std::f64::consts::PI * hz * index as f64 / rate()).sin()) as f32).collect()
    }

    fn softly(samples: Vec<f32>) -> Vec<f32> {
        samples.into_iter().enumerate().map(|(index, value)| (value as f64 * 10f64.powf((index as f64 / (0.2 * rate()) - 1.0) * 3.5).min(1.0)) as f32).collect()
    }

    /// The page tests' quiet room: noise far below speech.
    fn silence(seconds: f64, rate: f64) -> Vec<f32> {
        (0..(seconds * rate).round() as usize).map(|index| (1e-4 * (index as f64 * 12.9898).sin() * (index as f64 * 78.233).cos()) as f32).collect()
    }

    fn quiet(seconds: f64) -> Vec<f32> {
        silence(seconds, rate())
    }

    fn join(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    /// Where `samples` sits in `out`, sample for sample; None when it was never forwarded whole.
    fn located(out: &[f32], samples: &[f32]) -> Option<usize> {
        (0..=out.len().saturating_sub(samples.len())).find(|&offset| out[offset] == samples[0] && (0..samples.len()).step_by(97).all(|index| out[offset + index] == samples[index]))
    }

    #[test]
    fn the_constants_are_the_page_s() {
        let page = page();
        assert_eq!((page.rate, page.hop, page.chunk, page.offset, page.onset, page.windows), (16000, 160, 1280, 60, 4, 12));
        assert_eq!((page.frame, page.fft, page.bins, page.ring), (400, 512, 80, 600));
        assert_eq!((page.threshold, page.preroll, page.tail, page.shortest, page.every, page.window, page.longest), (0.5, 25, 15, 40, 100, 150, 300));
        assert!(SOURCE.contains(&format!("sha256: '{}'", page.sha256)) && page.sha256.len() == 64);
        assert_eq!(DECIMATION * page.rate, crate::audio::RATE as usize);
    }

    /// The fake model the golden was written with: the mean energy of the low bins against that of the middle bins.
    fn bands(features: &[f32], frames: usize) -> Result<Vec<f32>, String> {
        let band = |from: usize, to: usize| (0..frames).map(|frame| (from..to).map(|bin| (features[frame * 80 + bin] as f64).powi(2)).sum::<f64>()).sum::<f64>() / frames as f64;
        Ok(vec![band(0, 12) as f32, band(35, 65) as f32])
    }

    #[test]
    fn features_detections_and_gate_decisions_match_the_page_s_golden() {
        let golden: Value = serde_json::from_str(include_str!("../golden/speaker.json")).unwrap();
        let audio = join(&[quiet(1.0), tone(OTHER_HZ, 1.6), quiet(1.0), softly(tone(OWNER_HZ, 1.6)), quiet(1.5), tone(OWNER_HZ, 2.0), tone(OTHER_HZ, 3.0), quiet(1.5)]);
        let stride = golden["stride"].as_u64().unwrap() as usize;
        let features = fbank(&audio);
        let expected: Vec<f64> = golden["features"].as_array().unwrap().iter().map(|value| value.as_f64().unwrap()).collect();
        let ours: Vec<f32> = features.iter().copied().step_by(stride).collect();
        assert_eq!(ours.len(), expected.len());
        let worst = ours.iter().zip(&expected).map(|(&ours, &expected)| (ours as f64 - expected).abs()).fold(0.0, f64::max);
        assert!(worst < 1e-3, "largest feature difference {worst}");

        let mut detector = SpeechDetector::new();
        let detections: Vec<Value> = SpeakerFrames::new()
            .push(&audio)
            .into_iter()
            .map(|energy| detector.frame(energy))
            .enumerate()
            .filter(|(_, detection)| detection.start.is_some() || detection.end.is_some())
            .map(|(frame, detection)| {
                let mut value = serde_json::json!({ "frame": frame, "speech": detection.speech });
                if let Some(start) = detection.start {
                    value["start"] = start.into();
                }
                if let Some(end) = detection.end {
                    value["end"] = end.into();
                }
                value
            })
            .collect();
        assert_eq!(Value::from(detections), golden["detections"]);

        let mut gate = Gate::new(vec![1.0, 0.0]);
        let mut events = Vec::new();
        for chunk in audio.chunks(page().chunk) {
            let mut heard = Vec::new();
            gate.push(chunk, &mut bands, &mut |event| heard.push(event)).unwrap();
            events.extend(heard.into_iter().map(|event| (event, gate.frames.count)));
        }
        let expected = golden["events"].as_array().unwrap();
        assert_eq!(events.len(), expected.len(), "{events:?}");
        for ((event, at), expected) in events.iter().zip(expected) {
            assert_eq!(*at, expected["at"].as_i64().unwrap(), "{event:?} against {expected}");
            match event {
                Report::Open(from) => assert_eq!(Some(*from), expected["open"].as_i64()),
                Report::Close(to) => assert_eq!(Some(*to), expected["close"].as_i64()),
                Report::Score { score, from, to, .. } => {
                    assert!((*score as f64 - expected["score"].as_f64().unwrap()).abs() <= 2e-3, "{event:?} against {expected}");
                    assert_eq!((Some(*from), Some(*to)), (expected["from"].as_i64(), expected["to"].as_i64()));
                }
                _ => unreachable!(),
            }
        }

        let mut enrolled = Vec::new();
        let mut enrollment = Enrollment::new(4);
        let mut embed = |features: &[f32], frames: usize| {
            enrolled.push((frames, features[0] as f64, features[features.len() - 1] as f64));
            Ok(vec![1.0, 0.0])
        };
        for chunk in audio.chunks(page().chunk) {
            enrollment.push(chunk, &mut embed).unwrap();
        }
        let expected = golden["enrolled"].as_array().unwrap();
        assert!(!expected.is_empty());
        assert_eq!(enrolled.len(), expected.len());
        for (&(frames, first, last), expected) in enrolled.iter().zip(expected) {
            assert_eq!(frames as u64, expected[0].as_u64().unwrap());
            assert!((first - expected[1].as_f64().unwrap()).abs() < 1e-3 && (last - expected[2].as_f64().unwrap()).abs() < 1e-3, "window {first} {last} against {expected}");
        }
    }

    #[test]
    fn streamed_frames_give_the_same_features_as_the_whole_clip() {
        let audio = join(&[quiet(0.3), tone(440.0, 1.0), quiet(0.2)]);
        let mut frames = SpeakerFrames::new();
        for chunk in audio.chunks(1000) {
            frames.push(chunk);
        }
        let (features, count) = frames.features(10, 90);
        assert_eq!(count, 80);
        assert_eq!(features, fbank(&audio[10 * 160..89 * 160 + 400]));
    }

    /// The page's `runGate`: the clip through the gate in chunks, the granted audio read back as the worklet does.
    fn run_gate(audio: &[f32]) -> (Vec<Report>, Vec<f32>) {
        let mut gate = Gate::new(vec![1.0, 0.0]);
        let mut granted = GrantedAudio::new(audio.len() + page().rate);
        let mut events = Vec::new();
        let mut out = Vec::new();
        let mut padded = audio.to_vec();
        padded.extend(vec![0.0; 5 * page().rate]);
        for (index, chunk) in padded.chunks(page().chunk).enumerate() {
            granted.write(chunk);
            if index * page().chunk < audio.len() {
                let mut heard = Vec::new();
                gate.push(chunk, &mut bands, &mut |event| heard.push(event)).unwrap();
                for event in &heard {
                    match event {
                        Report::Open(at) => granted.open(*at),
                        Report::Close(at) => granted.close(*at),
                        _ => {}
                    }
                }
                events.extend(heard);
            }
            let mut block = vec![0.0; chunk.len()];
            granted.read(&mut block);
            out.extend(block);
        }
        (events, out)
    }

    #[test]
    fn the_owner_passes_whole_and_another_voice_never_does() {
        let owner = softly(tone(OWNER_HZ, 1.6));
        let other = tone(OTHER_HZ, 1.6);
        let audio = join(&[quiet(1.0), other.clone(), quiet(1.0), owner.clone(), quiet(1.5)]);
        let spoken = 16000 + other.len() + 16000;
        let (events, out) = run_gate(&audio);
        let gates: Vec<&Report> = events.iter().filter(|event| matches!(event, Report::Open(_) | Report::Close(_))).collect();
        assert!(matches!(gates[..], [Report::Open(_), Report::Close(_)]), "{events:?}");
        assert_eq!(located(&out, &other[4000..12000]), None);
        let at = located(&out, &owner).expect("the whole owner utterance is forwarded");
        let lag = (at - spoken) as f64 / 16000.0;
        assert!(lag > 0.5 && lag < 1.4, "forwarded {lag:.2} s after it was spoken");
    }

    #[test]
    fn granted_audio_is_replayed_in_order_skipping_what_was_never_granted() {
        let mut granted = GrantedAudio::new(1000);
        granted.write(&(1..=600).map(|value| value as f32).collect::<Vec<_>>());
        granted.open(100);
        granted.close(150);
        granted.open(400);
        let mut out = vec![0.0; 80];
        granted.read(&mut out);
        assert_eq!(out[..50], (101..151).map(|value| value as f32).collect::<Vec<_>>()[..]);
        assert_eq!(out[50..], (401..431).map(|value| value as f32).collect::<Vec<_>>()[..]);
        granted.close(460);
        granted.close(440);
        granted.read(&mut out);
        assert_eq!(out[..10], (431..441).map(|value| value as f32).collect::<Vec<_>>()[..]);
        assert!(out[10..].iter().all(|&value| value == 0.0));
        granted.write(&[0.0; 2000]);
        granted.open(0);
        granted.read(&mut out);
        assert_eq!(granted.cursor, 2600 - 1000 + 80, "audio already overwritten is skipped");
    }

    #[test]
    fn enrollment_averages_only_windows_that_are_mostly_speech() {
        let mut seen = Vec::new();
        let mut embed = |_: &[f32], frames: usize| {
            seen.push(frames);
            Ok(vec![1.0, 0.0])
        };
        let mut enrollment = Enrollment::new(3);
        let mut result = (0.0, None);
        for part in [quiet(3.0), tone(OWNER_HZ, 0.6), quiet(3.0)] {
            for chunk in part.chunks(page().chunk) {
                result = enrollment.push(chunk, &mut embed).unwrap();
            }
        }
        assert_eq!(result, (0.0, None));
        for chunk in tone(OWNER_HZ, 4.0).chunks(page().chunk) {
            result = enrollment.push(chunk, &mut embed).unwrap();
        }
        assert_eq!(result, (1.0, Some(vec![1.0, 0.0])));
        assert_eq!(seen, [150, 150, 150]);
    }

    #[test]
    fn decimation_keeps_speech_and_drops_what_the_filter_rate_cannot_hold() {
        let level = |hz: f64| {
            let input: Vec<f32> = (0..48000).map(|index| (0.5 * (2.0 * std::f64::consts::PI * hz * index as f64 / 48000.0).sin()) as f32).collect();
            let mut output = Vec::new();
            Decimator::new().push(&input, &mut output);
            assert_eq!(output.len(), 16000);
            (output[1000..].iter().map(|&sample| sample * sample).sum::<f32>() / 15000.0).sqrt() / (0.5 / 2f32.sqrt())
        };
        for hz in [100.0, 300.0, 1000.0, 3000.0, 6000.0] {
            assert!((level(hz) - 1.0).abs() < 0.02, "{hz} Hz at {}", level(hz));
        }
        for hz in [9000.0, 12000.0, 20000.0] {
            assert!(level(hz) < 0.01, "{hz} Hz folds back at {}", level(hz));
        }
    }

    #[test]
    fn the_stored_voiceprint_is_the_page_s_value_and_another_model_s_is_ignored() {
        let directory = std::env::temp_dir().join(format!("voice-vr-print-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("voiceprint.json");
        std::fs::write(&path, format!(r#"{{"model":"{}","print":[0.6,0.8]}}"#, page().sha256)).unwrap();
        assert_eq!(Stored::read(&path), Some(Stored { model: page().sha256.into(), print: vec![0.6, 0.8], off: false }));
        Stored::write(Some(&Stored { model: page().sha256.into(), print: vec![1.0], off: true }), &path);
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value, serde_json::json!({ "model": page().sha256, "print": [1.0], "off": true }));
        std::fs::write(&path, r#"{"model":"another","print":[1]}"#).unwrap();
        assert_eq!(Stored::read(&path), None);
        Stored::write(None, &path);
        assert!(!path.exists());
        Stored::write(None, &path);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_model_file_other_than_the_pinned_one_is_refused() {
        let path = std::env::temp_dir().join(format!("voice-vr-model-{}.onnx", std::process::id()));
        std::fs::write(&path, b"not the model").unwrap();
        let error = Model::load(&path).err().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("integrity"), "{error}");
    }

    /// Recorded speech at the capture rate, from the build's fixtures.
    fn recording(name: &str) -> Vec<f32> {
        let directory = std::env::var("VOICE_VR_SPEECH").expect("VOICE_VR_SPEECH names the recorded speech; the Nix check sets it");
        std::fs::read(format!("{directory}/{name}.f32")).unwrap().chunks_exact(4).map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap())).collect()
    }

    /// The session's path: 20 ms frames through the ear, each after the worker has finished what the last one sent, as an idle host keeps up.
    fn through(ear: &mut Ear, audio: &[f32]) -> Vec<f32> {
        let frame = crate::audio::RATE as usize / 50;
        let mut out = Vec::with_capacity(audio.len());
        for chunk in audio.chunks(frame) {
            let mut samples = chunk.to_vec();
            ear.hear(&mut samples);
            ear.settle();
            out.extend(samples);
        }
        out
    }

    /// The share of `clip`'s quarter-second spans that reached Live sample for sample.
    fn forwarded(out: &[f32], clip: &[f32]) -> f64 {
        let span = 12000;
        let spans: Vec<&[f32]> = clip.chunks_exact(span).filter(|span| span.iter().any(|&sample| sample.abs() > 1e-3)).collect();
        spans.iter().filter(|span| located(out, span).is_some()).count() as f64 / spans.len() as f64
    }

    #[test]
    fn recorded_speech_from_the_enrolled_speaker_passes_and_others_are_filtered_with_voice_id_on_and_off() {
        let capture = crate::audio::RATE as f64;
        let hearing = Arc::new(Mutex::new(Hearing::default()));
        let mut ear = Ear::new(hearing.clone());

        hearing.lock().unwrap().set(Mode::Learn);
        let mut enrollment = Vec::new();
        for name in ["leijun-sr-1", "leijun-sr-2", "leijun-test-sr-1"] {
            enrollment.extend(recording(name));
            enrollment.extend(silence(0.5, capture));
        }
        let mut heard = through(&mut ear, &enrollment);
        heard.extend(through(&mut ear, &silence(0.1, capture)));
        let learned = hearing.lock().unwrap().reports.iter().find_map(|report| if let Report::Learned(print) = report { Some(print.clone()) } else { None });
        let print = learned.unwrap_or_else(|| panic!("no voiceprint: {:?}", hearing.lock().unwrap().reports));
        assert!(heard.iter().all(|&sample| sample == 0.0), "Live hears nothing while the voice is learned");
        let progress: Vec<f32> = hearing.lock().unwrap().reports.iter().filter_map(|report| if let Report::Progress(progress) = report { Some(*progress) } else { None }).collect();
        assert!(progress.windows(2).all(|pair| pair[0] < pair[1]) && progress.len() == page().windows - 1, "{progress:?}");

        let (enrolled, first, second) = (recording("leijun-test-sr-2"), recording("fangjun-test-sr-1"), recording("speaker2_a_en"));
        let conversation = join(&[silence(1.0, capture), first.clone(), silence(1.5, capture), enrolled.clone(), silence(1.5, capture), second.clone(), silence(3.0, capture)]);

        hearing.lock().unwrap().set(Mode::Filter(print.as_slice().into()));
        let filtered = through(&mut ear, &conversation);
        let shares = [forwarded(&filtered, &enrolled), forwarded(&filtered, &first), forwarded(&filtered, &second)];
        eprintln!("voice ID on: forwarded shares: enrolled {:.2}, another {:.2}, a third {:.2}", shares[0], shares[1], shares[2]);
        assert!(shares[0] >= 0.9, "the enrolled speaker reaches Live: {shares:?}");
        assert_eq!(shares[1..], [0.0, 0.0], "other speakers do not");

        hearing.lock().unwrap().set(Mode::Open);
        let open = through(&mut ear, &conversation);
        assert_eq!(open, conversation, "voice ID off: Live hears everyone, untouched");
    }
}
