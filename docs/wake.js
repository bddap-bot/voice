export const RATE = 16000;
export const CHUNK = 1280;
export const WINDOW = 16;
export const WIDTH = 96;
const CONTEXT = 480;
const BINS = 32;
const FRAMES = 76;
const REFRACTORY = 25;
const BATCH = 256;
const pcm = (sample) => sample * 32767;
const scaled = (value) => value / 10 + 2;

export class WakeFeatures {
  constructor(mel, embed) {
    this.mel = mel;
    this.embed = embed;
    this.audio = new Float32Array(CONTEXT + CHUNK);
    this.frames = new Float32Array(FRAMES * BINS);
    this.filled = 0;
    this.embeddings = new Float32Array(WINDOW * WIDTH);
    this.count = 0;
  }
  async push(chunk) {
    this.audio.copyWithin(0, CHUNK);
    for (let index = 0; index < CHUNK; index++) this.audio[CONTEXT + index] = pcm(chunk[index]);
    const raw = await this.mel(this.audio);
    const added = raw.length / BINS;
    this.frames.copyWithin(0, added * BINS);
    for (let index = 0; index < raw.length; index++) this.frames[(FRAMES - added) * BINS + index] = scaled(raw[index]);
    this.filled = Math.min(FRAMES, this.filled + added);
    if (this.filled < FRAMES) return null;
    this.embeddings.copyWithin(0, WIDTH);
    this.embeddings.set(await this.embed(this.frames), (WINDOW - 1) * WIDTH);
    this.count = Math.min(WINDOW, this.count + 1);
    return this.count < WINDOW ? null : this.embeddings;
  }
}

export async function wakeFeatures(ort, load) {
  const [mel, embed] = await Promise.all([load('melspectrogram'), load('embedding_model')]);
  const melRun = async (audio, count = 1) => (await mel.run({ input: new ort.Tensor('float32', audio, [count, CONTEXT + CHUNK]) })).output.data;
  const embedRun = async (frames, count = 1) => (await embed.run({ input_1: new ort.Tensor('float32', frames, [count, FRAMES, BINS, 1]) })).conv2d_19.data;
  const stream = () => new WakeFeatures(melRun, embedRun);
  stream.all = async (audio) => {
    const chunks = Math.floor(audio.length / CHUNK);
    let frames;
    let perChunk;
    for (let start = 0; start < chunks; start += BATCH) {
      const count = Math.min(BATCH, chunks - start);
      const input = new Float32Array(count * (CONTEXT + CHUNK));
      for (let item = 0; item < count; item++) {
        for (let index = 0; index < CONTEXT + CHUNK; index++) {
          const at = (start + item + 1) * CHUNK - CONTEXT - CHUNK + index;
          input[item * (CONTEXT + CHUNK) + index] = at < 0 ? 0 : pcm(audio[at]);
        }
      }
      const raw = await melRun(input, count);
      perChunk ??= raw.length / BINS / count;
      frames ??= new Float32Array(chunks * perChunk * BINS);
      for (let index = 0; index < raw.length; index++) frames[start * perChunk * BINS + index] = scaled(raw[index]);
    }
    if (!chunks) return { first: 0, embeddings: new Float32Array(0) };
    const first = Math.ceil(FRAMES / perChunk) - 1;
    const embeddings = new Float32Array(Math.max(0, chunks - first) * WIDTH);
    for (let start = first; start < chunks; start += BATCH) {
      const count = Math.min(BATCH, chunks - start);
      const input = new Float32Array(count * FRAMES * BINS);
      for (let item = 0; item < count; item++) {
        const end = (start + item + 1) * perChunk;
        input.set(frames.subarray((end - FRAMES) * BINS, end * BINS), item * FRAMES * BINS);
      }
      embeddings.set(await embedRun(input, count), (start - first) * WIDTH);
    }
    return { first, embeddings };
  };
  return stream;
}

function floats(encoded) {
  const bytes = Uint8Array.from(atob(encoded), (character) => character.charCodeAt(0));
  return new Float32Array(bytes.buffer);
}

export function loadHead(json) {
  return { ...json, mean: floats(json.mean), scale: floats(json.scale), w1: floats(json.w1), b1: floats(json.b1), w2: floats(json.w2) };
}

export function headScore(head, window) {
  let logit = head.b2;
  for (let unit = 0; unit < head.b1.length; unit++) {
    let sum = head.b1[unit];
    const row = unit * window.length;
    for (let index = 0; index < window.length; index++) sum += head.w1[row + index] * (window[index] - head.mean[index]) * head.scale[index];
    if (sum > 0) logit += head.w2[unit] * sum;
  }
  return 1 / (1 + Math.exp(-logit));
}

export class WakeDecision {
  constructor({ threshold, miss }) {
    this.threshold = threshold;
    this.miss = miss;
    this.rest = 0;
    this.peak = 0;
  }
  decide(score) {
    if (this.rest > 0) {
      this.rest--;
      return null;
    }
    if (score >= this.threshold) {
      this.rest = REFRACTORY;
      this.peak = 0;
      return { wake: score };
    }
    if (score >= this.miss) {
      this.peak = Math.max(this.peak, score);
      return null;
    }
    if (!this.peak) return null;
    const peak = this.peak;
    this.peak = 0;
    return { miss: peak };
  }
}

export async function startWakeSpotter(stream, heard, model) {
  const context = new AudioContext({ sampleRate: RATE });
  await context.audioWorklet.addModule(new URL('./wake-worklet.js', import.meta.url));
  const worker = new Worker(new URL('./wake-worker.js', import.meta.url), { type: 'module' });
  worker.postMessage({ model });
  const capture = new AudioWorkletNode(context, 'wake-capture', { numberOfOutputs: 0 });
  const source = context.createMediaStreamSource(stream);
  source.connect(capture);
  capture.port.onmessage = ({ data }) => worker.postMessage(data, [data.buffer]);
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
