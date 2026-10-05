use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use pulseaudio::protocol;

pub const RATE: u32 = 48_000;
const SILENT: f32 = 0.001;
/// The page's jitter delay before playback starts.
const DELAY: usize = (RATE as usize * 15) / 100;
const CAPACITY: usize = RATE as usize * 180;
/// The page's analyser window for the mouth.
const WINDOW: usize = 256;
/// Server-side buffering: small, so the mouth moves with what is heard.
const LATENCY_MS: u32 = 40;

/// The page's `PlaybackBuffer`: a jitter delay, catching up across silence, and a run of silence that says speech has ended.
/// Packets arrive only while the far side sends, so time pulled with nothing queued also counts as silence, and the tail plays out once packets stop.
pub struct PlaybackBuffer {
    samples: VecDeque<f32>,
    silent_run: usize,
    since_push: usize,
    level: f32,
}

impl Default for PlaybackBuffer {
    fn default() -> Self {
        PlaybackBuffer { samples: VecDeque::new(), silent_run: 0, since_push: 0, level: 0.0 }
    }
}

impl PlaybackBuffer {
    pub fn push(&mut self, input: &[f32]) {
        for &sample in input {
            if self.samples.len() == CAPACITY {
                self.samples.pop_front();
            }
            self.samples.push_back(sample);
            self.silent_run = if sample.abs() > SILENT { 0 } else { self.silent_run + 1 };
        }
        self.since_push = 0;
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// True once `samples` of silence have run and nothing louder is still queued.
    pub fn quiet(&self, samples: usize) -> bool {
        self.silent_run >= samples.max(self.samples.len())
    }

    /// Restarts the silence count, as the page does when it starts waiting for quiet.
    pub fn listen_for_quiet(&mut self) {
        self.silent_run = 0;
    }

    pub fn pull(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let delay = if self.since_push >= DELAY { 0 } else { DELAY };
        while self.samples.len() > delay + output.len() && self.samples.iter().take(output.len()).all(|sample| sample.abs() <= SILENT) {
            self.samples.drain(..output.len());
        }
        let mut written = 0;
        while written < output.len() && self.samples.len() > delay {
            output[written] = self.samples.pop_front().unwrap();
            written += 1;
        }
        if self.samples.is_empty() {
            self.silent_run += output.len() - written;
        }
        self.since_push += output.len();
        let window = &output[output.len().saturating_sub(WINDOW)..];
        self.level = (window.iter().map(|sample| sample * sample).sum::<f32>() / window.len().max(1) as f32).sqrt();
    }

    /// The root-mean-square of the latest output, which the mouth follows.
    pub fn level(&self) -> f32 {
        self.level
    }
}

/// The page's `loudnessViseme` amount: the mouth opening for an output energy.
pub fn mouth(energy: f32) -> f32 {
    let t = ((energy - 0.018) / (0.16 - 0.018)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) * 0.82
}

fn floats(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes.chunks_exact(4).map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
}

fn spec() -> protocol::SampleSpec {
    protocol::SampleSpec { format: protocol::SampleFormat::Float32Le, channels: 1, sample_rate: RATE }
}

fn bytes_for(milliseconds: u32) -> u32 {
    RATE / 1000 * milliseconds * 4
}

/// One client of the default sound server, holding the reply's playback stream and, while unmuted, the microphone.
pub struct Audio {
    client: pulseaudio::Client,
    playback: Option<pulseaudio::PlaybackStream>,
    capture: Option<pulseaudio::RecordStream>,
    pub captured: Arc<Mutex<VecDeque<f32>>>,
}

impl Audio {
    pub fn open(played: Arc<Mutex<PlaybackBuffer>>) -> Result<Audio, String> {
        let client = pulseaudio::Client::from_env(c"voice-vr").map_err(|error| format!("sound server: {error}"))?;
        let source = played;
        let fill = move |data: &mut [u8]| {
            let mut samples = vec![0.0; data.len() / 4];
            source.lock().unwrap().pull(&mut samples);
            for (chunk, sample) in data.chunks_exact_mut(4).zip(samples) {
                chunk.copy_from_slice(&sample.to_le_bytes());
            }
            data.len() / 4 * 4
        };
        let params = protocol::PlaybackStreamParams {
            sample_spec: spec(),
            channel_map: protocol::ChannelMap::mono(),
            cvolume: Some(protocol::ChannelVolume::norm(1)),
            sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
            buffer_attr: protocol::stream::BufferAttr { target_length: bytes_for(LATENCY_MS), minimum_request_length: bytes_for(LATENCY_MS / 4), pre_buffering: bytes_for(LATENCY_MS / 4), ..Default::default() },
            flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
            ..Default::default()
        };
        let playback = futures::executor::block_on(client.create_playback_stream(params, pulseaudio::AsPlaybackSource::as_playback_source(fill))).map_err(|error| format!("playback: {error}"))?;
        Ok(Audio { client, playback: Some(playback), capture: None, captured: Arc::new(Mutex::new(VecDeque::new())) })
    }

    /// Opens the default capture source, or releases it so the device is free.
    pub fn capture(&mut self, on: bool) -> Result<(), String> {
        if !on {
            self.captured.lock().unwrap().clear();
            if let Some(stream) = self.capture.take() {
                futures::executor::block_on(stream.delete()).map_err(|error| format!("microphone release: {error}"))?;
            }
            return Ok(());
        }
        if self.capture.is_some() {
            return Ok(());
        }
        let sink = self.captured.clone();
        let write = move |data: &[u8]| {
            sink.lock().unwrap().extend(floats(data));
        };
        let params = protocol::RecordStreamParams {
            sample_spec: spec(),
            channel_map: protocol::ChannelMap::mono(),
            source_name: Some(protocol::DEFAULT_SOURCE.to_owned()),
            buffer_attr: protocol::stream::BufferAttr { fragment_size: bytes_for(10), ..Default::default() },
            flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
            ..Default::default()
        };
        self.capture = Some(futures::executor::block_on(self.client.create_record_stream(params, write)).map_err(|error| format!("microphone: {error}"))?);
        Ok(())
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        if let Err(error) = self.capture(false) {
            eprintln!("{error}");
        }
        if let Some(stream) = self.playback.take() {
            if let Err(error) = futures::executor::block_on(stream.delete()) {
                eprintln!("playback close: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(length: usize) -> Vec<f32> {
        (0..length).map(|i| 0.3 * (i as f32 * 0.05).sin() + 0.01).collect()
    }

    #[test]
    fn playback_waits_for_the_jitter_delay_while_packets_keep_coming() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&tone(DELAY));
        let mut output = vec![1.0; 128];
        buffer.pull(&mut output);
        assert!(output.iter().all(|&sample| sample == 0.0));
        buffer.push(&tone(960));
        buffer.pull(&mut output);
        assert!(output.iter().all(|&sample| sample != 0.0));
    }

    #[test]
    fn the_delay_returns_after_the_tail_has_played_out() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&tone(DELAY + 128));
        let mut output = vec![0.0; 128];
        for _ in 0..(3 * DELAY / 128) {
            buffer.pull(&mut output);
        }
        buffer.push(&tone(DELAY));
        buffer.pull(&mut output);
        assert!(output.iter().all(|&sample| sample == 0.0));
    }

    #[test]
    fn the_tail_plays_once_exactly_the_delay_has_passed_without_packets() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&tone(DELAY));
        buffer.pull(&mut vec![0.0; DELAY]);
        let mut output = vec![0.0; 128];
        buffer.pull(&mut output);
        assert!(output.iter().all(|&sample| sample != 0.0));
    }

    #[test]
    fn the_tail_plays_out_once_packets_stop() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&tone(DELAY + 128));
        let mut played = 0;
        for _ in 0..(3 * DELAY / 128) {
            let mut output = vec![0.0; 128];
            buffer.pull(&mut output);
            played += output.iter().filter(|&&sample| sample != 0.0).count();
        }
        assert_eq!(played, DELAY + 128);
    }

    #[test]
    fn silence_beyond_the_delay_is_skipped_to_catch_up() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&vec![0.0; 4 * DELAY]);
        buffer.push(&tone(DELAY + 256));
        let mut output = vec![0.0; 128];
        buffer.pull(&mut output);
        assert!(output.iter().all(|&sample| sample != 0.0), "the leading silence was dropped");
    }

    #[test]
    fn quiet_comes_a_run_after_the_last_sound_plays() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&tone(960));
        buffer.listen_for_quiet();
        let mut output = vec![0.0; 480];
        let mut pulled = 0;
        while !buffer.quiet(RATE as usize) {
            buffer.pull(&mut output);
            pulled += output.len();
            assert!(pulled < 3 * RATE as usize);
        }
        assert!((DELAY + 960 + RATE as usize..DELAY + 960 + RATE as usize + 2 * 480).contains(&pulled), "{pulled}");
        buffer.push(&[0.5]);
        assert!(!buffer.quiet(RATE as usize));
    }

    #[test]
    fn queued_sound_keeps_it_from_being_quiet() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&tone(960));
        buffer.push(&vec![0.0; RATE as usize]);
        assert!(!buffer.quiet(RATE as usize));
    }

    #[test]
    fn listening_for_quiet_restarts_the_run() {
        let mut buffer = PlaybackBuffer::default();
        buffer.pull(&mut vec![0.0; RATE as usize + 1]);
        assert!(buffer.quiet(RATE as usize));
        buffer.listen_for_quiet();
        assert!(!buffer.quiet(RATE as usize));
    }

    #[test]
    fn silence_held_by_the_delay_counts_once() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&tone(960));
        buffer.listen_for_quiet();
        let mut output = vec![0.0; 480];
        let mut pulled = 0;
        while !buffer.quiet(RATE as usize) {
            buffer.push(&[0.0; 480]);
            buffer.pull(&mut output);
            pulled += output.len();
            assert!(pulled < 3 * RATE as usize);
        }
        assert!(pulled >= RATE as usize, "silent packets still arriving: {pulled}");
    }

    #[test]
    fn the_level_follows_the_latest_output() {
        let mut buffer = PlaybackBuffer::default();
        buffer.push(&vec![0.5; DELAY + 1024]);
        let mut output = vec![0.0; 512];
        buffer.pull(&mut output);
        assert!((buffer.level() - 0.5).abs() < 1e-6);
        buffer.clear();
        buffer.push(&[0.5; 256]);
        buffer.push(&[0.002; 256]);
        buffer.push(&vec![0.5; DELAY]);
        buffer.pull(&mut output);
        assert!(buffer.level() < 0.01, "the latest window, not the first");
        buffer.clear();
        buffer.pull(&mut output);
        assert_eq!(buffer.level(), 0.0);
    }

    #[test]
    fn the_mouth_opens_with_energy_as_the_page_s_does() {
        assert_eq!(mouth(0.0), 0.0);
        assert_eq!(mouth(0.018), 0.0);
        assert!((mouth(0.089) - 0.41).abs() < 1e-3);
        assert!((mouth(0.0535) - 0.128).abs() < 1e-3, "smoothstep, not linear");
        assert!((mouth(0.16) - 0.82).abs() < 1e-6);
        assert!((mouth(1.0) - 0.82).abs() < 1e-6);
    }
}
