export const TURN = {
  url: 'https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/f766f81d3cfdf7737ac64aad813d91bbfd56bf93/smart-turn-v3.2-gpu.onnx',
  sha256: 'ab8dc64b88713f90b571c15b714bd1330e6c883cad8763dacf65c9376dc539be',
  complete: 0.5,
  quietMs: 1000,
  // Past the longest mid-thought pause measured in real sessions, 5.6 s.
  holdMs: 6000,
};
export const RATE = 16000;
export const WINDOW = 8 * RATE;
const FFT = 400;
const HOP = 160;
const BINS = FFT / 2 + 1;
export const MELS = 80;
export const FRAMES = WINDOW / HOP;

const hzToMel = (hz) => (hz < 1000 ? (3 * hz) / 200 : 15 + (Math.log(hz / 1000) * 27) / Math.log(6.4));
const melToHz = (mel) => (mel < 15 ? (200 * mel) / 3 : 1000 * Math.exp(((mel - 15) * Math.log(6.4)) / 27));
let tables;

function whisperTables() {
  if (tables) return tables;
  const edges = Array.from({ length: MELS + 2 }, (_, index) => melToHz((hzToMel(8000) * index) / (MELS + 1)));
  const filters = new Float64Array(MELS * BINS);
  for (let mel = 0; mel < MELS; mel++) {
    const [left, center, right] = [edges[mel], edges[mel + 1], edges[mel + 2]];
    for (let bin = 0; bin < BINS; bin++) {
      const hz = (bin * 8000) / (BINS - 1);
      filters[mel * BINS + bin] = (Math.max(0, Math.min((hz - left) / (center - left), (right - hz) / (right - center))) * 2) / (right - left);
    }
  }
  const angle = (at) => (-2 * Math.PI * ((Math.floor(at / FFT) * (at % FFT)) % FFT)) / FFT;
  tables = {
    window: Float64Array.from({ length: FFT }, (_, n) => 0.5 - 0.5 * Math.cos((2 * Math.PI * n) / FFT)),
    cosines: Float64Array.from({ length: BINS * FFT }, (_, at) => Math.cos(angle(at))),
    sines: Float64Array.from({ length: BINS * FFT }, (_, at) => Math.sin(angle(at))),
    filters,
  };
  return tables;
}

export function features(samples) {
  const { window, cosines, sines, filters } = whisperTables();
  const newest = samples.subarray(Math.max(0, samples.length - WINDOW));
  const padded = new Float64Array(WINDOW);
  padded.set(newest, WINDOW - newest.length);
  let mean = 0;
  for (const sample of padded) mean += sample;
  mean /= WINDOW;
  let variance = 0;
  for (const sample of padded) variance += (sample - mean) ** 2;
  const deviation = Math.sqrt(variance / WINDOW + 1e-7);
  const reflect = (at) => (padded[at < 0 ? -at : at >= WINDOW ? 2 * (WINDOW - 1) - at : at] - mean) / deviation;
  const logMel = new Float32Array(MELS * FRAMES);
  const frame = new Float64Array(FFT);
  const power = new Float64Array(BINS);
  for (let index = 0; index < FRAMES; index++) {
    for (let n = 0; n < FFT; n++) frame[n] = reflect(index * HOP + n - FFT / 2) * window[n];
    for (let bin = 0; bin < BINS; bin++) {
      let re = 0;
      let im = 0;
      for (let n = 0, at = bin * FFT; n < FFT; n++, at++) {
        re += frame[n] * cosines[at];
        im += frame[n] * sines[at];
      }
      power[bin] = re * re + im * im;
    }
    for (let mel = 0; mel < MELS; mel++) {
      let energy = 0;
      for (let bin = 0, at = mel * BINS; bin < BINS; bin++, at++) energy += filters[at] * power[bin];
      logMel[mel * FRAMES + index] = Math.log10(Math.max(energy, 1e-10));
    }
  }
  let ceiling = -Infinity;
  for (const value of logMel) ceiling = Math.max(ceiling, value);
  ceiling -= 8;
  for (let at = 0; at < logMel.length; at++) logMel[at] = (Math.max(logMel[at], ceiling) + 4) / 4;
  return logMel;
}

export class HandOffHold {
  constructor({ handOff, judge, now = () => Date.now(), later = (...args) => globalThis.setTimeout(...args), cancel = (timer) => globalThis.clearTimeout(timer), log = () => {} }) {
    Object.assign(this, { handOff, judge, now, later, cancel, log });
    this.held = null;
    this.heardAt = null;
    this.asks = 0;
    this.timer = null;
  }
  due(id, owned) {
    if (this.held) {
      if (owned) this.held.id = id;
      return;
    }
    const now = this.now();
    this.held = { id, judging: null, judgeAt: now, until: (this.heardAt ?? now) + TURN.holdMs };
    this.step();
  }
  heard() {
    const now = this.heardAt = this.now();
    if (!this.held) return;
    Object.assign(this.held, { judging: null, judgeAt: now + TURN.quietMs, until: now + TURN.holdMs });
    this.step();
  }
  take() {
    const id = this.held?.id ?? null;
    this.clear();
    return id;
  }
  clear() {
    this.held = null;
    this.cancel(this.timer);
    this.timer = null;
  }
  step() {
    this.cancel(this.timer);
    this.timer = null;
    const held = this.held;
    if (!held) return;
    const now = this.now();
    if (now >= held.until) {
      this.log('released', { id: held.id });
      return this.handOff(held.id);
    }
    if (held.judging === null && held.judgeAt !== null && now >= held.judgeAt) {
      const number = held.judging = ++this.asks;
      held.judgeAt = null;
      this.judge((probability) => this.judged(number, probability));
      if (this.held !== held) return;
    }
    this.timer = this.later(() => this.step(), Math.max(0, Math.min(held.judgeAt ?? Infinity, held.until) - now));
  }
  judged(number, probability) {
    const held = this.held;
    if (held?.judging !== number) return;
    held.judging = null;
    this.log('judged', { id: held.id, probability });
    if (probability === null || probability >= TURN.complete) this.handOff(held.id);
  }
}

let worker;
let asks = 0;
const verdicts = new Map();

function turnWorker() {
  if (worker) return worker;
  const started = worker = new Worker(new URL('./turn-worker.js', import.meta.url), { type: 'module' });
  started.onmessage = ({ data }) => {
    const verdict = verdicts.get(data.ask);
    verdicts.delete(data.ask);
    verdict?.(data);
  };
  started.onerror = (event) => {
    if (worker !== started) return;
    started.terminate();
    worker = undefined;
    for (const [ask, verdict] of verdicts) verdict({ ask, error: event.message || 'turn worker failed' });
    verdicts.clear();
  };
  return started;
}

export function prepareTurn() {
  turnWorker().postMessage({ load: true });
}

export async function listenForTurns(stream) {
  const context = new AudioContext({ sampleRate: RATE });
  try {
    await context.audioWorklet.addModule(new URL('./turn-worklet.js', import.meta.url));
    const node = new AudioWorkletNode(context, 'turn-tap', { numberOfOutputs: 0, channelCount: 1, channelCountMode: 'explicit' });
    const { port1, port2 } = new MessageChannel();
    turnWorker().postMessage({ audio: port1 }, [port1]);
    node.port.postMessage({ audio: port2 }, [port2]);
    const source = context.createMediaStreamSource(stream);
    source.connect(node);
    if (context.state === 'suspended') for (const type of ['pointerdown', 'keydown']) addEventListener(type, () => { if (context.state === 'suspended') context.resume(); }, { once: true });
    return {
      close() {
        source.disconnect();
        node.port.postMessage({ close: true });
        return context.close();
      },
    };
  } catch (error) {
    context.close();
    throw error;
  }
}

export function judgeTurn(heard) {
  const ask = ++asks;
  verdicts.set(ask, heard);
  turnWorker().postMessage({ ask });
}
