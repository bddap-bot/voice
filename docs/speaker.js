export const RATE = 16000;
export const HOP = 160;
export const CHUNK = 1280;
export const MODEL = {
  url: 'https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/0743f301363dec56491a490f6d6cbc9d67f9a3bf/3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx',
  sha256: 'c59158379255ad66e161679cca6af8d52d51e389e3224ab7d7a7baae295c2db5',
};
export const GATE = {
  threshold: 0.5,
  preroll: 25,
  tail: 15,
  onset: 4,
  offset: 60,
  first: 100,
  shortest: 40,
  retry: 100,
  check: 100,
  window: 150,
  longest: 200,
};
export const ENROLL_WINDOWS = 12;
const FRAME = 400;
const FFT = 512;
const BINS = 80;
const RING = 600;

const hamming = Float32Array.from({ length: FRAME }, (_, n) => 0.54 - 0.46 * Math.cos((2 * Math.PI * n) / (FRAME - 1)));
const mel = (hz) => 1127 * Math.log(1 + hz / 700);
const filters = Array.from({ length: BINS }, (_, bin) => {
  const low = mel(20);
  const step = (mel(RATE / 2) - low) / (BINS + 1);
  const [left, center, right] = [low + bin * step, low + (bin + 1) * step, low + (bin + 2) * step];
  const weights = Array.from({ length: FFT / 2 }, (_, index) => {
    const m = mel((index * RATE) / FFT);
    return m <= left || m >= right ? 0 : m <= center ? (m - left) / (center - left) : (right - m) / (right - center);
  });
  const first = weights.findIndex((weight) => weight > 0);
  return { first, weights: Float32Array.from(weights.slice(first, weights.findLastIndex((weight) => weight > 0) + 1)) };
});
const cosines = Float64Array.from({ length: FFT / 2 }, (_, index) => Math.cos((-2 * Math.PI * index) / FFT));
const sines = Float64Array.from({ length: FFT / 2 }, (_, index) => Math.sin((-2 * Math.PI * index) / FFT));
const reversed = Uint16Array.from({ length: FFT }, (_, index) => {
  let result = 0;
  for (let bit = 1, rest = index; bit < FFT; bit <<= 1, rest >>= 1) result = (result << 1) | (rest & 1);
  return result;
});
const frame = new Float64Array(FRAME);
const re = new Float64Array(FFT);
const im = new Float64Array(FFT);

function logMel(samples, into) {
  let mean = 0;
  for (let index = 0; index < FRAME; index++) mean += samples[index];
  mean /= FRAME;
  let energy = 0;
  for (let index = 0; index < FRAME; index++) {
    frame[index] = (samples[index] - mean) * 32768;
    energy += samples[index] * samples[index];
  }
  for (let index = FRAME - 1; index > 0; index--) frame[index] -= 0.97 * frame[index - 1];
  frame[0] -= 0.97 * frame[0];
  for (let index = 0; index < FFT; index++) {
    const source = reversed[index];
    re[index] = source < FRAME ? frame[source] * hamming[source] : 0;
    im[index] = 0;
  }
  for (let size = 2; size <= FFT; size <<= 1) {
    const half = size >> 1;
    const stride = FFT / size;
    for (let start = 0; start < FFT; start += size) {
      for (let k = 0; k < half; k++) {
        const cos = cosines[k * stride];
        const sin = sines[k * stride];
        const a = start + k;
        const b = a + half;
        const tr = re[b] * cos - im[b] * sin;
        const ti = re[b] * sin + im[b] * cos;
        re[b] = re[a] - tr;
        im[b] = im[a] - ti;
        re[a] += tr;
        im[a] += ti;
      }
    }
  }
  for (let bin = 0; bin < BINS; bin++) {
    const { first, weights } = filters[bin];
    let sum = 0;
    for (let index = 0; index < weights.length; index++) sum += weights[index] * (re[first + index] * re[first + index] + im[first + index] * im[first + index]);
    into[bin] = Math.log(Math.max(sum, 1.1920928955078125e-7));
  }
  return 10 * Math.log10(energy / FRAME + 1e-12);
}

export function fbank(samples) {
  const count = samples.length < FRAME ? 0 : 1 + Math.floor((samples.length - FRAME) / HOP);
  const features = new Float32Array(count * BINS);
  for (let frame = 0; frame < count; frame++) logMel(samples.subarray(frame * HOP, frame * HOP + FRAME), features.subarray(frame * BINS, (frame + 1) * BINS));
  return center(features, count);
}

function center(features, count) {
  for (let bin = 0; bin < BINS; bin++) {
    let mean = 0;
    for (let frame = 0; frame < count; frame++) mean += features[frame * BINS + bin];
    mean /= count || 1;
    for (let frame = 0; frame < count; frame++) features[frame * BINS + bin] -= mean;
  }
  return features;
}

export function normalized(vector) {
  let norm = 0;
  for (const value of vector) norm += value * value;
  norm = Math.sqrt(norm) || 1;
  return Float32Array.from(vector, (value) => value / norm);
}

export function similarity(a, b) {
  let sum = 0;
  for (let index = 0; index < a.length; index++) sum += a[index] * b[index];
  return sum;
}

export function voiceprint(embeddings) {
  const sum = new Float32Array(embeddings[0].length);
  for (const embedding of embeddings) for (let index = 0; index < sum.length; index++) sum[index] += embedding[index];
  return normalized(sum);
}

export class SpeakerFrames {
  constructor() {
    this.pending = new Float32Array(0);
    this.count = 0;
    this.mels = new Float32Array(RING * BINS);
    this.energies = new Float32Array(RING);
  }
  push(samples) {
    const joined = new Float32Array(this.pending.length + samples.length);
    joined.set(this.pending);
    joined.set(samples, this.pending.length);
    const energies = [];
    let offset = 0;
    for (; offset + FRAME <= joined.length; offset += HOP) {
      const slot = this.count % RING;
      this.energies[slot] = logMel(joined.subarray(offset, offset + FRAME), this.mels.subarray(slot * BINS, (slot + 1) * BINS));
      energies.push(this.energies[slot]);
      this.count++;
    }
    this.pending = joined.slice(offset);
    return energies;
  }
  features(from, to) {
    from = Math.max(from, this.count - RING);
    const count = Math.max(0, to - from);
    const features = new Float32Array(count * BINS);
    for (let frame = 0; frame < count; frame++) {
      const slot = (from + frame) % RING;
      features.set(this.mels.subarray(slot * BINS, (slot + 1) * BINS), frame * BINS);
    }
    return { features: center(features, count), frames: count };
  }
}

export class SpeechDetector {
  constructor({ onset = GATE.onset, offset = GATE.offset } = {}) {
    this.onset = onset;
    this.offset = offset;
    this.heard = [];
    this.floor = -Infinity;
    this.recent = [];
    this.silent = 0;
    this.speaking = false;
  }
  frame(energy) {
    if (energy > -100) {
      this.heard.push(energy);
      if (this.heard.length > 1000) this.heard.shift();
      if (this.heard.length % 10 === 1) this.floor = [...this.heard].sort((x, y) => x - y)[Math.floor(this.heard.length / 10)];
    }
    const speech = energy > Math.max(this.floor + 18, -70);
    this.recent.push(speech);
    if (this.recent.length > 6) this.recent.shift();
    this.silent = speech ? 0 : this.silent + 1;
    if (!this.speaking && this.recent.filter(Boolean).length >= this.onset) {
      this.speaking = true;
      return { speech, start: this.recent.length - this.recent.indexOf(true) };
    }
    if (this.speaking && this.silent >= this.offset) {
      this.speaking = false;
      return { speech, end: this.silent };
    }
    return { speech };
  }
}

export class SpeakerEnrollment {
  constructor({ embed, windows = ENROLL_WINDOWS, options = GATE }) {
    this.embed = embed;
    this.windows = windows;
    this.options = options;
    this.frames = new SpeakerFrames();
    this.detector = new SpeechDetector(options);
    this.speech = [];
    this.embeddings = [];
    this.next = options.window;
  }
  async push(samples) {
    for (const energy of this.frames.push(samples)) {
      this.speech.push(this.detector.frame(energy).speech);
      if (this.speech.length > this.options.window) this.speech.shift();
      const now = this.frames.count;
      if (now < this.next || this.embeddings.length >= this.windows) continue;
      if (this.speech.filter(Boolean).length < 0.6 * this.options.window) continue;
      const { features, frames } = this.frames.features(now - this.options.window, now);
      this.embeddings.push(normalized(await this.embed(features, frames)));
      this.next = now + this.options.window / 2;
    }
    return { progress: this.embeddings.length / this.windows, voiceprint: this.embeddings.length >= this.windows ? voiceprint(this.embeddings) : null };
  }
}

export class SpeakerGate {
  constructor({ voiceprint: print, embed, send, options = GATE }) {
    this.print = print;
    this.embed = embed;
    this.send = send;
    this.options = options;
    this.frames = new SpeakerFrames();
    this.detector = new SpeechDetector(options);
    this.segments = [];
    this.pending = null;
  }
  push(samples) {
    for (const energy of this.frames.push(samples)) this.step(this.detector.frame(energy));
    this.decide();
  }
  idle() {
    return this.pending ?? Promise.resolve();
  }
  step(event) {
    const { options } = this;
    const now = this.frames.count;
    if (event.start !== undefined) {
      const start = now - event.start - options.preroll;
      this.segments.push({ start, due: start + options.first, open: false, scored: false, end: null });
    }
    const segment = this.segments.at(-1);
    if (event.end === undefined || !segment || segment.end !== null) return;
    segment.end = now - event.end + options.tail;
    if (segment.open) this.send({ close: segment.end * HOP });
    segment.due = segment.open || segment.scored || segment.end - segment.start < options.shortest ? Infinity : now;
  }
  decide() {
    if (this.pending) return;
    this.segments = this.segments.filter((segment) => segment.end === null || segment.due !== Infinity);
    const now = this.frames.count;
    const segment = this.segments.find((item) => item.due <= now);
    if (!segment) return;
    const to = segment.end ?? now;
    const from = Math.max(segment.start, to - (segment.open ? this.options.window : this.options.longest));
    const { features, frames } = this.frames.features(from, to);
    this.pending = this.embed(features, frames).then((embedding) => {
      const score = similarity(this.print, normalized(embedding));
      this.send({ score: +score.toFixed(3), from: from * HOP, to: to * HOP });
      this.pending = null;
      this.verdict(segment, score >= this.options.threshold, from, to);
      this.decide();
    }, (error) => this.send({ error: String(error?.stack ?? error) }));
  }
  verdict(segment, match, from, to) {
    const { options } = this;
    const now = this.frames.count;
    segment.scored = true;
    if (segment.open && !match) {
      this.send({ close: Math.round((from + to) / 2) * HOP });
      Object.assign(segment, { open: false, scored: false, start: to, due: segment.end === null ? now + options.first : Infinity });
    } else if (!segment.open && match) {
      segment.open = true;
      this.send({ open: from * HOP });
      if (segment.end !== null) this.send({ close: segment.end * HOP });
      segment.due = segment.end === null ? now + options.check : Infinity;
    } else segment.due = segment.end !== null ? Infinity : now + (segment.open ? options.check : options.retry);
  }
}

export class GrantedAudio {
  constructor(capacity = 10 * RATE) {
    this.ring = new Float32Array(capacity);
    this.written = 0;
    this.cursor = 0;
    this.grants = [];
  }
  open(from) {
    this.grants.push({ from, to: Infinity });
  }
  close(to) {
    const last = this.grants.at(-1);
    if (last) last.to = Math.min(last.to, to);
  }
  write(samples) {
    for (const sample of samples) this.ring[this.written++ % this.ring.length] = sample;
  }
  read(out) {
    out.fill(0);
    let at = 0;
    while (at < out.length && this.grants.length) {
      const grant = this.grants[0];
      this.cursor = Math.max(this.cursor, grant.from, this.written - this.ring.length);
      if (this.cursor >= grant.to) {
        this.grants.shift();
        continue;
      }
      const count = Math.min(Math.min(grant.to, this.written) - this.cursor, out.length - at);
      if (count <= 0) break;
      for (let index = 0; index < count; index++) out[at + index] = this.ring[(this.cursor + index) % this.ring.length];
      this.cursor += count;
      at += count;
    }
  }
}

export async function startSpeakerGate(stream, voiceprint, heard) {
  const context = new AudioContext({ sampleRate: RATE });
  await context.audioWorklet.addModule(new URL('./speaker-worklet.js', import.meta.url));
  const worker = new Worker(new URL('./speaker-worker.js', import.meta.url), { type: 'module' });
  const node = new AudioWorkletNode(context, 'speaker-gate', { outputChannelCount: [1], channelCount: 1, channelCountMode: 'explicit' });
  const source = context.createMediaStreamSource(stream);
  const destination = context.createMediaStreamDestination();
  source.connect(node).connect(destination);
  node.port.onmessage = ({ data }) => worker.postMessage(data, [data.buffer]);
  worker.onmessage = ({ data }) => {
    if ('open' in data || 'close' in data) node.port.postMessage(data);
    heard(data);
  };
  worker.onerror = (event) => heard({ error: event.message || 'speaker worker failed' });
  worker.postMessage({ voiceprint });
  if (context.state === 'suspended') for (const type of ['pointerdown', 'keydown']) addEventListener(type, () => { if (context.state === 'suspended') context.resume(); }, { once: true });
  return {
    stream: destination.stream,
    close() {
      source.disconnect();
      worker.terminate();
      return context.close();
    },
  };
}
