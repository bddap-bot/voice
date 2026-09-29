import assert from 'node:assert/strict';
import test from 'node:test';
import { WeSpeakerFeatureExtractor } from '@huggingface/transformers';
import { CHUNK, GATE, GrantedAudio, HOP, OFFSET, RATE, SpeakerEnrollment, SpeakerFrames, SpeakerGate, SpeechDetector, fbank } from '../docs/speaker.js';

const OWNER_HZ = 300;
const OTHER_HZ = 2500;

function tone(hz, seconds, level = 0.3) {
  return Float32Array.from({ length: Math.round(seconds * RATE) }, (_, index) => level * Math.sin((2 * Math.PI * hz * index) / RATE));
}

function softly(samples) {
  return Float32Array.from(samples, (value, index) => value * Math.min(1, 10 ** ((index / (0.2 * RATE) - 1) * 3.5)));
}

function silence(seconds) {
  return Float32Array.from({ length: Math.round(seconds * RATE) }, (_, index) => 1e-4 * Math.sin(index * 12.9898) * Math.cos(index * 78.233));
}

function join(...parts) {
  const out = new Float32Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

async function runGate(audio, owner, runChunks = 0) {
  const events = [];
  const running = [];
  let span;
  let most = 0;
  const embed = () => {
    const [from, to] = span;
    const own = Math.max(0, Math.min(to, owner[1]) - Math.max(from, owner[0]));
    const embedding = own * 2 > to - from ? [1, 0.1] : [0.1, 1];
    if (!runChunks) return Promise.resolve(embedding);
    return new Promise((resolve) => {
      running.push({ left: runChunks, finish: () => resolve(embedding) });
      most = Math.max(most, running.length);
    });
  };
  const gate = new SpeakerGate({ voiceprint: Float32Array.from([1, 0]), embed, send: (event) => events.push({ ...event, at: gate.frames.count * HOP }) });
  const features = gate.frames.features.bind(gate.frames);
  gate.frames.features = (from, to) => {
    span = [from * HOP, to * HOP];
    return features(from, to);
  };
  const granted = new GrantedAudio(audio.length + RATE);
  const out = new Float32Array(audio.length + 5 * RATE);
  const block = new Float32Array(128);
  let sent = 0;
  for (let at = 0; at < out.length; at += 128) {
    if (at < audio.length) {
      const input = audio.subarray(at, at + 128);
      granted.write(input);
      if ((at + 128) % CHUNK === 0 || at + 128 >= audio.length) {
        gate.push(audio.subarray(Math.floor(at / CHUNK) * CHUNK, at + 128));
        for (const job of running) if (--job.left <= 0) job.finish();
        running.splice(0, running.length, ...running.filter((job) => job.left > 0));
        if (!runChunks) await gate.idle();
        else await new Promise((resolve) => setImmediate(resolve));
      }
    } else granted.write(new Float32Array(128));
    for (; sent < events.length; sent++) {
      if ('open' in events[sent]) granted.open(events[sent].open);
      if ('close' in events[sent]) granted.close(events[sent].close);
    }
    granted.read(block);
    out.set(block.subarray(0, Math.min(128, out.length - at)), at);
  }
  return { events, out, most };
}

function located(out, samples) {
  for (let offset = 0; offset + samples.length <= out.length; offset++) {
    let match = true;
    for (let index = 0; index < samples.length && match; index += 97) match = out[offset + index] === samples[index];
    if (match) return offset;
  }
  return -1;
}

test('filter-bank features equal the WeSpeaker Kaldi-compatible extractor', async () => {
  const extractor = new WeSpeakerFeatureExtractor({ sampling_rate: RATE, num_mel_bins: 80, min_num_frames: 9 });
  const audio = join(tone(220, 0.7), silence(0.2), tone(1800, 0.6, 0.1));
  const expected = await extractor._extract_fbank_features(audio);
  const ours = fbank(audio);
  assert.equal(ours.length, expected.data.length);
  const centered = new Float32Array(expected.data);
  const frames = centered.length / 80;
  for (let bin = 0; bin < 80; bin++) {
    let mean = 0;
    for (let frame = 0; frame < frames; frame++) mean += centered[frame * 80 + bin];
    for (let frame = 0; frame < frames; frame++) centered[frame * 80 + bin] -= mean / frames;
  }
  let worst = 0;
  for (let index = 0; index < ours.length; index++) worst = Math.max(worst, Math.abs(ours[index] - centered[index]));
  assert.ok(worst < 1e-2, `largest difference ${worst}`);
});

test('streamed frames give the same features as the whole clip', () => {
  const audio = join(silence(0.3), tone(440, 1), silence(0.2));
  const frames = new SpeakerFrames();
  for (let at = 0; at < audio.length; at += 1000) frames.push(audio.subarray(at, at + 1000));
  const { features, frames: count } = frames.features(10, 90);
  assert.equal(count, 80);
  assert.deepEqual(features, fbank(audio.subarray(10 * HOP, 89 * HOP + 400)));
});

test('speech onset reports how far back it began, and silence ends it after the offset window', () => {
  const detector = new SpeechDetector();
  const frames = new SpeakerFrames();
  const events = [];
  for (const energy of frames.push(join(silence(1), tone(300, 0.5), silence(1)))) events.push(detector.frame(energy));
  const start = events.findIndex((event) => event.start !== undefined);
  const end = events.findIndex((event) => event.end !== undefined);
  assert.ok(Math.abs(start - events[start].start - 100) <= 3, `onset frame ${start - events[start].start}`);
  assert.equal(events[end].end, OFFSET);
  assert.equal(events.filter((event) => event.start !== undefined).length, 1);
});

test('a steady new noise stops counting as speech once it becomes the background', () => {
  const detector = new SpeechDetector();
  const frames = new SpeakerFrames();
  const hum = (seconds) => Float32Array.from(tone(120, seconds, 0.02), (value, index) => value + 0.01 * Math.sin(index * 0.37));
  const events = [];
  for (const part of [silence(12), hum(20)]) for (let at = 0; at < part.length; at += CHUNK) for (const energy of frames.push(part.subarray(at, at + CHUNK))) events.push(detector.frame(energy));
  const started = events.findIndex((event) => event.start !== undefined);
  const ended = events.findIndex((event) => event.end !== undefined);
  assert.ok(started >= 1200, 'the hum begins as speech');
  assert.ok(ended > started && ended < 3200, `the hum is background ${((ended - 1200) / 100).toFixed(1)} s after it began`);
});

test('the enrolled voice passes whole and delayed, while another voice never passes', async () => {
  const owner = softly(tone(OWNER_HZ, 1.6));
  const other = tone(OTHER_HZ, 1.6);
  const audio = join(silence(1), other, silence(1), owner, silence(1.5));
  const spokenAt = RATE + other.length + RATE;
  const { events, out } = await runGate(audio, [spokenAt, spokenAt + owner.length]);
  assert.deepEqual(events.filter((event) => 'open' in event || 'close' in event).map((event) => Object.keys(event)[0]), ['open', 'close']);
  assert.equal(located(out, other.subarray(4000, 12000)), -1);
  const at = located(out, owner);
  assert.ok(at > 0, 'the whole enrolled utterance, from its first sample, is forwarded');
  const lag = (at - spokenAt) / RATE;
  assert.ok(lag > 0.5 && lag < 1.2, `forwarded ${lag.toFixed(2)} s after it was spoken`);
  const detector = new SpeechDetector();
  const onsets = [];
  new SpeakerFrames().push(audio).forEach((energy, index) => {
    const event = detector.frame(energy);
    if (event.start !== undefined) onsets.push(index + 1 - event.start);
  });
  assert.equal(events.find((event) => 'open' in event).open, (onsets[1] - GATE.preroll) * HOP, 'forwarding starts a full preroll before the detected onset');
  const forwarded = out.reduce((count, value) => count + (value !== 0), 0);
  assert.ok(forwarded < owner.length + (GATE.preroll + GATE.tail + 4) * HOP, 'nothing beyond the utterance and its margins is forwarded');
});

test('a model slower than real time delays decisions without queueing them or clipping the voice', async () => {
  const owner = tone(OWNER_HZ, 3);
  const other = tone(OTHER_HZ, 2);
  const spokenAt = RATE + other.length + RATE;
  const { events, out, most } = await runGate(join(silence(1), other, silence(1), owner, silence(3)), [spokenAt, spokenAt + owner.length], 8);
  assert.equal(most, 1);
  const at = located(out, owner);
  assert.ok(at > 0, JSON.stringify(events));
  const lag = (at - spokenAt) / RATE;
  assert.ok(lag > 1.5 && lag < 2.2, `forwarded ${lag.toFixed(2)} s after it was spoken`);
  assert.equal(located(out, other.subarray(4000, 12000)), -1);
  assert.deepEqual(events.filter((event) => 'open' in event || 'close' in event).map((event) => Object.keys(event)[0]), ['open', 'close']);
});

test('a voice that starts while the model is still busy is scored and forwarded from its start', async () => {
  const other = tone(OTHER_HZ, 1.5);
  const owner = tone(OWNER_HZ, 3);
  const spokenAt = RATE + other.length + Math.round(0.7 * RATE);
  const { out } = await runGate(join(silence(1), other, silence(0.7), owner, silence(8)), [spokenAt, spokenAt + owner.length], 40);
  assert.ok(located(out, owner) > 0);
});

test('another voice taking over without a pause is cut off at the next check', async () => {
  const owner = tone(OWNER_HZ, 2);
  const other = tone(OTHER_HZ, 3);
  const { events, out } = await runGate(join(silence(1), owner, other, silence(1.5)), [RATE, RATE + owner.length]);
  assert.ok(located(out, owner) > 0);
  const close = events.find((event) => 'close' in event).close;
  const leaked = (close - (RATE + owner.length)) / RATE;
  assert.ok(leaked >= 0 && leaked <= (GATE.every + GATE.window) / 100, `other voice forwarded for ${leaked.toFixed(2)} s`);
  assert.equal(located(out, other.subarray(Math.round(1.6 * RATE), Math.round(2.4 * RATE))), -1);
});

test('granted audio is replayed in order, skipping what was never granted', () => {
  const granted = new GrantedAudio(1000);
  granted.write(Float32Array.from({ length: 600 }, (_, index) => index + 1));
  granted.open(100);
  granted.close(150);
  granted.open(400);
  const out = new Float32Array(80);
  granted.read(out);
  assert.deepEqual([...out.subarray(0, 50)], Array.from({ length: 50 }, (_, index) => index + 101));
  assert.deepEqual([...out.subarray(50)], Array.from({ length: 30 }, (_, index) => index + 401));
  granted.close(460);
  granted.close(440);
  granted.read(out);
  assert.deepEqual([...out.subarray(0, 10)], Array.from({ length: 10 }, (_, index) => index + 431));
  assert.ok(out.subarray(10).every((value) => value === 0));
  granted.write(new Float32Array(2000));
  granted.open(0);
  granted.read(out);
  assert.equal(granted.cursor, 2600 - 1000 + 80, 'audio already overwritten is skipped');
});

test('enrollment averages only windows that are mostly speech', async () => {
  const seen = [];
  const enrollment = new SpeakerEnrollment({ embed: async (features, frames) => { seen.push(frames); return [1, 0]; }, windows: 3 });
  let result;
  for (const part of [silence(3), tone(OWNER_HZ, 0.6), silence(3)]) for (let at = 0; at < part.length; at += CHUNK) result = await enrollment.push(part.subarray(at, at + CHUNK));
  assert.equal(seen.length, 0);
  assert.equal(result.voiceprint, null);
  const speech = tone(OWNER_HZ, 4);
  for (let at = 0; at < speech.length; at += CHUNK) result = await enrollment.push(speech.subarray(at, at + CHUNK));
  assert.equal(result.progress, 1);
  assert.deepEqual([...result.voiceprint], [1, 0]);
  assert.deepEqual(seen, [GATE.window, GATE.window, GATE.window]);
});
