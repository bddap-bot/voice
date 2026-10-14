use std::collections::VecDeque;
use std::f64::consts::PI;
use std::path::PathBuf;
use std::sync::{mpsc, OnceLock};
use std::time::Instant;

use tract_onnx::prelude::*;

use crate::speaker::Decimator;

const RATE: usize = 16_000;
/// Smart Turn reads the newest eight seconds, zero-padded in front.
const WINDOW: usize = 8 * RATE;
const FFT: usize = 400;
const HOP: usize = 160;
const BINS: usize = FFT / 2 + 1;
const MELS: usize = 80;
const FRAMES: usize = WINDOW / HOP;
/// The model's own decision line on its sigmoid output.
const COMPLETE: f32 = 0.5;

fn hz_to_mel(hz: f64) -> f64 {
    if hz < 1000.0 { 3.0 * hz / 200.0 } else { 15.0 + (hz / 1000.0).ln() * 27.0 / 6.4f64.ln() }
}

fn mel_to_hz(mel: f64) -> f64 {
    if mel < 15.0 { 200.0 * mel / 3.0 } else { 1000.0 * ((mel - 15.0) * 6.4f64.ln() / 27.0).exp() }
}

struct Tables {
    window: Vec<f64>,
    cosines: Vec<f32>,
    sines: Vec<f32>,
    filters: Vec<f32>,
}

/// Whisper's feature extractor: periodic Hann, 400-point frames every 160 samples, 80 Slaney mel filters to 8 kHz.
fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let edges: Vec<f64> = (0..MELS + 2).map(|index| mel_to_hz(hz_to_mel(8000.0) * index as f64 / (MELS + 1) as f64)).collect();
        let mut filters = vec![0.0f32; MELS * BINS];
        for mel in 0..MELS {
            let (left, center, right) = (edges[mel], edges[mel + 1], edges[mel + 2]);
            for bin in 0..BINS {
                let hz = bin as f64 * 8000.0 / (BINS - 1) as f64;
                let weight = ((hz - left) / (center - left)).min((right - hz) / (right - center)).max(0.0);
                filters[mel * BINS + bin] = (weight * 2.0 / (right - left)) as f32;
            }
        }
        let angle = |bin: usize, n: usize| -2.0 * PI * ((bin * n) % FFT) as f64 / FFT as f64;
        Tables {
            window: (0..FFT).map(|n| 0.5 - 0.5 * (2.0 * PI * n as f64 / FFT as f64).cos()).collect(),
            cosines: (0..BINS * FFT).map(|at| angle(at / FFT, at % FFT).cos() as f32).collect(),
            sines: (0..BINS * FFT).map(|at| angle(at / FFT, at % FFT).sin() as f32).collect(),
            filters,
        }
    })
}

/// The `input_features` Smart Turn's reference inference makes of the newest eight seconds: `[MELS, FRAMES]`, row-major.
pub fn features(samples: &[f32]) -> Vec<f32> {
    let tables = tables();
    let newest = &samples[samples.len().saturating_sub(WINDOW)..];
    let mut padded = vec![0.0f64; WINDOW - newest.len()];
    padded.extend(newest.iter().map(|&sample| sample as f64));
    let mean = padded.iter().sum::<f64>() / WINDOW as f64;
    let deviation = (padded.iter().map(|sample| (sample - mean).powi(2)).sum::<f64>() / WINDOW as f64 + 1e-7).sqrt();
    let reflect = |at: isize| -> f32 {
        let at = if at < 0 { -at } else if at >= WINDOW as isize { 2 * (WINDOW as isize - 1) - at } else { at };
        ((padded[at as usize] - mean) / deviation) as f32
    };
    let mut log_mel = vec![0.0f32; MELS * FRAMES];
    let mut frame = [0.0f32; FFT];
    let mut power = [0.0f32; BINS];
    for index in 0..FRAMES {
        for (n, slot) in frame.iter_mut().enumerate() {
            *slot = reflect((index * HOP + n) as isize - (FFT / 2) as isize) * tables.window[n] as f32;
        }
        for (bin, slot) in power.iter_mut().enumerate() {
            let (cosines, sines) = (&tables.cosines[bin * FFT..(bin + 1) * FFT], &tables.sines[bin * FFT..(bin + 1) * FFT]);
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for n in 0..FFT {
                re += frame[n] * cosines[n];
                im += frame[n] * sines[n];
            }
            *slot = re * re + im * im;
        }
        for mel in 0..MELS {
            let energy: f32 = tables.filters[mel * BINS..(mel + 1) * BINS].iter().zip(&power).map(|(weight, power)| weight * power).sum();
            log_mel[mel * FRAMES + index] = energy.max(1e-10).log10();
        }
    }
    let ceiling = log_mel.iter().copied().fold(f32::NEG_INFINITY, f32::max) - 8.0;
    log_mel.iter().map(|&value| (value.max(ceiling) + 4.0) / 4.0).collect()
}

/// Smart Turn v3: the probability that the speech so far ends a turn, from the audio alone.
pub struct Model {
    plan: TypedSimplePlan<TypedModel>,
}

impl Model {
    pub fn load(path: &std::path::Path) -> Result<Model, String> {
        let failed = |error: TractError| format!("turn model: {error}");
        let mut model = tract_onnx::onnx().model_for_path(path).map_err(failed)?;
        // The export names its batch `s6` in every intermediate shape too, so the batch is fixed only once the graph is typed.
        let batch = model.symbols.sym("s6");
        model.set_input_fact(0, f32::fact([batch.to_dim(), MELS.to_dim(), FRAMES.to_dim()]).into()).map_err(failed)?;
        let model = model.into_typed().map_err(failed)?.concretize_dims(&SymbolValues::default().with(&batch, 1)).map_err(failed)?;
        Ok(Model { plan: model.into_optimized().map_err(failed)?.into_runnable().map_err(failed)? })
    }

    pub fn complete(&self, samples: &[f32]) -> Result<f32, String> {
        let failed = |error: TractError| format!("turn model: {error}");
        let input = Tensor::from_shape(&[1, MELS, FRAMES], &features(samples)).map_err(failed)?;
        let output = self.plan.run(tvec!(input.into())).map_err(failed)?;
        Ok(output[0].as_slice::<f32>().map_err(failed)?[0])
    }
}

/// `voice-vr turn <audio> <seconds>...`: the judgement of a recording (mono 32-bit float at 16 kHz) cut at each time, and how long each took.
pub fn replay(args: &[String]) -> Result<(), String> {
    let [audio, cuts @ ..] = args else { return Err("usage: voice-vr turn <audio.f32> <seconds>...".into()) };
    let model = Model::load(&std::env::var_os("VOICE_VR_TURN_MODEL").map(PathBuf::from).ok_or("VOICE_VR_TURN_MODEL is not set")?)?;
    let bytes = std::fs::read(audio).map_err(|error| format!("{audio}: {error}"))?;
    let samples: Vec<f32> = bytes.chunks_exact(4).map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap())).collect();
    for cut in cuts {
        let seconds: f64 = cut.parse().map_err(|_| format!("{cut}: not a time in seconds"))?;
        let started = Instant::now();
        let probability = model.complete(&samples[..((seconds * RATE as f64) as usize).min(samples.len())])?;
        println!("{seconds} {probability:.4} {} ms", started.elapsed().as_millis());
    }
    Ok(())
}

/// What Live heard of the microphone, at the model's rate, newest eight seconds.
pub struct Tail {
    decimator: Decimator,
    decimated: Vec<f32>,
    samples: VecDeque<f32>,
}

impl Default for Tail {
    fn default() -> Tail {
        Tail { decimator: Decimator::new(), decimated: Vec::new(), samples: VecDeque::with_capacity(WINDOW) }
    }
}

impl Tail {
    pub fn push(&mut self, frame: &[f32]) {
        self.decimated.clear();
        self.decimator.push(frame, &mut self.decimated);
        self.samples.extend(&self.decimated);
        let excess = self.samples.len().saturating_sub(WINDOW);
        self.samples.drain(..excess);
    }

    pub fn snapshot(&self) -> Vec<f32> {
        self.samples.iter().copied().collect()
    }
}

/// Smart Turn on its own thread, so a verdict never holds up a rendered frame; each verdict answers the ask with its number.
pub struct Judge {
    asks: mpsc::Sender<(u64, Vec<f32>)>,
    verdicts: mpsc::Receiver<(u64, Result<f32, String>)>,
}

impl Judge {
    pub fn start() -> Judge {
        let (asks, asked) = mpsc::channel::<(u64, Vec<f32>)>();
        let (answer, verdicts) = mpsc::channel();
        std::thread::spawn(move || {
            let started = Instant::now();
            let model = std::env::var_os("VOICE_VR_TURN_MODEL").map(PathBuf::from).ok_or_else(|| "VOICE_VR_TURN_MODEL is not set".to_owned()).and_then(|path| Model::load(&path));
            match &model {
                Ok(_) => tracing::info!("turn model loaded in {:.1} s", started.elapsed().as_secs_f32()),
                Err(error) => tracing::warn!("{error}: every pause ends a thought"),
            }
            while let Ok((number, audio)) = asked.recv() {
                let started = Instant::now();
                let verdict = model.as_ref().map_err(Clone::clone).and_then(|model| model.complete(&audio));
                if let Ok(probability) = verdict {
                    tracing::info!("turn verdict {number}: {probability:.2} in {} ms", started.elapsed().as_millis());
                }
                if answer.send((number, verdict)).is_err() {
                    return;
                }
            }
        });
        Judge { asks, verdicts }
    }

    pub fn ask(&self, number: u64, audio: Vec<f32>) {
        let _ = self.asks.send((number, audio));
    }

    /// Each answered ask: whether its speech ends a thought; a model that cannot judge lets every pause end one.
    pub fn verdicts(&self) -> Vec<(u64, bool)> {
        self.verdicts.try_iter().map(|(number, verdict)| (number, verdict.map_or(true, |probability| probability >= COMPLETE))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech(name: &str) -> Vec<f32> {
        let directory = std::env::var("VOICE_VR_SPEECH").expect("VOICE_VR_SPEECH names the recorded speech");
        let bytes = std::fs::read(format!("{directory}/{name}.f32")).unwrap();
        let captured: Vec<f32> = bytes.chunks_exact(4).map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap())).collect();
        captured.iter().step_by(crate::speaker::DECIMATION).copied().collect()
    }

    /// Smart Turn's reference inference (onnxruntime and transformers' WhisperFeatureExtractor) on the same cuts of the same recordings.
    #[test]
    fn the_overlay_scores_speech_as_the_reference_inference_does() {
        let model = Model::load(std::path::Path::new(&std::env::var("VOICE_VR_TURN_MODEL").expect("VOICE_VR_TURN_MODEL names the model"))).unwrap();
        for (name, seconds, reference) in [("leijun-test-sr-1", 3.2, 0.9903), ("leijun-test-sr-1", 4.0, 0.0298), ("fangjun-test-sr-1", 2.5, 0.9881), ("fangjun-test-sr-1", 3.4, 0.0139)] {
            let clip = speech(name);
            let probability = model.complete(&clip[..(seconds * RATE as f64) as usize]).unwrap();
            eprintln!("{name} at {seconds} s: {probability:.4}, reference {reference:.4}");
            assert!((probability - reference).abs() < 0.03, "{name} at {seconds} s: {probability} against the reference {reference}");
        }
    }

    #[test]
    fn the_tail_keeps_the_newest_eight_seconds_at_the_model_rate() {
        let mut tail = Tail::default();
        for _ in 0..500 {
            tail.push(&[0.25; 960]);
        }
        let snapshot = tail.snapshot();
        assert_eq!(snapshot.len(), WINDOW);
        assert!((snapshot[WINDOW - 1] - 0.25).abs() < 1e-3);
    }
}
