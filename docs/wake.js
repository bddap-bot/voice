export const WAKE_MODEL = { id: 'onnx-community/whisper-base.en', dtype: 'q8' };
export const WAKE_SCORE = 0.8;
export const MISS_SCORE = 0.6;

const FRAMES_PER_SECOND = 50;
const PREROLL = 15;
const HANGOVER = 35;
const VOICED = 10;
const LONGEST = 400;
const OVERLAP = 100;

function words(text) {
  return text.toLowerCase().normalize('NFKD').replace(/[^a-z0-9]+/g, ' ').trim().split(' ').filter(Boolean);
}

function distance(a, b) {
  let previous = Array.from({ length: b.length + 1 }, (_, index) => index);
  for (let i = 1; i <= a.length; i++) {
    const current = [i];
    for (let j = 1; j <= b.length; j++) current[j] = Math.min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
    previous = current;
  }
  return previous[b.length];
}

export function wakeScore(heard, phrase) {
  const target = words(phrase);
  const goal = target.join(' ');
  const spoken = words(heard);
  let best = { score: 0, heard: '' };
  for (let length = 1; length <= target.length + 1; length++) {
    for (let start = 0; start + length <= spoken.length; start++) {
      const window = spoken.slice(start, start + length).join(' ');
      const score = 1 - distance(window, goal) / Math.max(window.length, goal.length);
      if (score > best.score) best = { score, heard: window };
    }
  }
  return best;
}

export class SpeechSegmenter {
  constructor(rate) {
    this.frame = new Float32Array(Math.round(rate / FRAMES_PER_SECOND));
    this.filled = 0;
    this.floor = Infinity;
    this.history = [];
    this.frames = null;
    this.voiced = 0;
    this.quiet = 0;
  }
  push(samples) {
    const segments = [];
    for (const sample of samples) {
      this.frame[this.filled++] = sample;
      if (this.filled < this.frame.length) continue;
      this.filled = 0;
      const segment = this.step(this.frame.slice());
      if (segment) segments.push(segment);
    }
    return segments;
  }
  step(frame) {
    const energy = Math.sqrt(frame.reduce((sum, sample) => sum + sample * sample, 0) / frame.length);
    this.floor = Math.min(energy, Math.max(this.floor, 1e-4) * 1.005);
    const loud = energy > Math.max(0.005, this.floor * 3);
    if (!this.frames) {
      if (!loud) {
        this.history = [...this.history, frame].slice(-PREROLL);
        return null;
      }
      this.frames = this.history;
      this.history = [];
    }
    this.frames.push(frame);
    if (loud) {
      this.voiced++;
      this.quiet = 0;
    } else this.quiet++;
    if (this.quiet < HANGOVER && this.frames.length < LONGEST) return null;
    const frames = this.frames;
    const voiced = this.voiced;
    this.frames = this.quiet < HANGOVER ? frames.slice(-OVERLAP) : null;
    this.voiced = 0;
    this.quiet = 0;
    if (voiced < VOICED) return null;
    const segment = new Float32Array(frames.length * frame.length);
    frames.forEach((item, index) => segment.set(item, index * frame.length));
    return segment;
  }
}

export async function startWakeSpotter(stream, heard, listening) {
  const context = new AudioContext({ sampleRate: 16000 });
  await context.audioWorklet.addModule(new URL('./wake-worklet.js', import.meta.url));
  const worker = new Worker(new URL('./wake-worker.js', import.meta.url), { type: 'module' });
  const capture = new AudioWorkletNode(context, 'wake-capture', { numberOfOutputs: 0 });
  const source = context.createMediaStreamSource(stream);
  source.connect(capture);
  capture.port.onmessage = ({ data }) => { if (listening()) worker.postMessage(data, [data.buffer]); };
  worker.onmessage = ({ data }) => heard(data);
  worker.onerror = (event) => heard({ error: event.message || 'wake worker failed' });
  if (context.state === 'suspended') for (const type of ['pointerdown', 'keydown']) addEventListener(type, () => context.resume(), { once: true });
  return {
    close() {
      source.disconnect();
      worker.terminate();
      return context.close();
    },
  };
}
