import { writeFileSync } from 'node:fs';
import { CHUNK, RATE, SpeakerEnrollment, SpeakerFrames, SpeakerGate, SpeechDetector, fbank } from '../docs/speaker.js';

const out = new URL('../vr/golden/speaker.json', import.meta.url).pathname;
const tone = (hz, seconds, level = 0.3) => Float32Array.from({ length: Math.round(seconds * RATE) }, (_, index) => level * Math.sin((2 * Math.PI * hz * index) / RATE));
const softly = (samples) => Float32Array.from(samples, (value, index) => value * Math.min(1, 10 ** ((index / (0.2 * RATE) - 1) * 3.5)));
const silence = (seconds) => Float32Array.from({ length: Math.round(seconds * RATE) }, (_, index) => 1e-4 * Math.sin(index * 12.9898) * Math.cos(index * 78.233));
const parts = [silence(1), tone(2500, 1.6), silence(1), softly(tone(300, 1.6)), silence(1.5), tone(300, 2), tone(2500, 3), silence(1.5)];
const audio = Float32Array.from(parts.flatMap((part) => [...part]));

const embed = async (features, frames) => {
  const band = (from, to) => {
    let sum = 0;
    for (let frame = 0; frame < frames; frame++) for (let bin = from; bin < to; bin++) sum += features[frame * 80 + bin] ** 2;
    return sum / frames;
  };
  return [band(0, 12), band(35, 65)];
};
const events = [];
const gate = new SpeakerGate({ voiceprint: Float32Array.from([1, 0]), embed, send: (event) => events.push({ ...event, at: gate.frames.count }) });
for (let at = 0; at < audio.length; at += CHUNK) {
  gate.push(audio.subarray(at, at + CHUNK));
  while (gate.pending) await gate.idle();
}
const enrolled = [];
const enrollment = new SpeakerEnrollment({ embed: async (features, frames) => { enrolled.push([frames, +features[0].toFixed(5), +features[features.length - 1].toFixed(5)]); return [1, 0]; }, windows: 4 });
for (let at = 0; at < audio.length; at += CHUNK) await enrollment.push(audio.subarray(at, at + CHUNK));
const detector = new SpeechDetector();
const detections = new SpeakerFrames().push(audio).map((energy) => detector.frame(energy)).flatMap((event, frame) => (event.start !== undefined || event.end !== undefined ? [{ frame, ...event }] : []));
const features = fbank(audio);
const stride = 37;
writeFileSync(out, JSON.stringify({
  about: 'docs/speaker.js on a synthetic clip: written by scripts/vr-speaker-golden.mjs, compared by the overlay host build',
  stride,
  features: Array.from(features.filter((_, index) => index % stride === 0), (value) => +value.toFixed(5)),
  detections,
  events,
  enrolled,
}) + '\n');
