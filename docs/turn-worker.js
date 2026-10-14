import * as ort from './lib/ort.js';
import { FRAMES, MELS, TURN, WINDOW, features } from './turn.js';

const heard = new Float32Array(WINDOW);
let filled = 0;
let audio;
let loading;
let queue = Promise.resolve();

async function modelBytes() {
  const cache = await caches.open('voice-turn-model');
  const response = await cache.match(TURN.url) ?? await fetch(TURN.url);
  if (!response.ok) throw new Error(`turn model download failed: ${response.status}`);
  const bytes = await response.clone().arrayBuffer();
  const digest = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  if (digest !== TURN.sha256) {
    await cache.delete(TURN.url);
    throw new Error('turn model failed its integrity check');
  }
  await cache.put(TURN.url, response).catch(() => {});
  return bytes;
}

function session() {
  loading ??= modelBytes().then((bytes) => ort.InferenceSession.create(bytes));
  loading.catch(() => { loading = undefined; });
  return loading;
}

function hear(samples) {
  if (samples.length >= WINDOW) {
    heard.set(samples.subarray(samples.length - WINDOW));
  } else {
    heard.copyWithin(0, samples.length);
    heard.set(samples, WINDOW - samples.length);
  }
  filled = Math.min(WINDOW, filled + samples.length);
}

async function judge(ask) {
  const model = await session();
  const started = performance.now();
  const input = new ort.Tensor('float32', features(heard.subarray(WINDOW - filled)), [1, MELS, FRAMES]);
  const output = await model.run({ [model.inputNames[0]]: input });
  postMessage({ ask, probability: output[model.outputNames[0]].data[0], ms: Math.round(performance.now() - started) });
}

onmessage = ({ data }) => {
  if (data.load) session().catch(() => {});
  else if (data.audio) {
    audio?.close();
    audio = data.audio;
    filled = 0;
    audio.onmessage = ({ data: samples }) => hear(samples);
  } else if (data.ask) queue = queue.then(() => judge(data.ask)).catch((error) => postMessage({ ask: data.ask, error: String(error?.stack ?? error) }));
};
