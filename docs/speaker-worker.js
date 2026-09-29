import * as ort from 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.22.0/dist/ort.wasm.min.mjs';
import { MODEL, SpeakerEnrollment, SpeakerGate } from './speaker.js';

ort.env.wasm.wasmPaths = 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.22.0/dist/';
let loading;
let current;
let listener;
let queue = Promise.resolve();

async function modelBytes() {
  const cache = await caches.open('voice-speaker-model');
  const response = await cache.match(MODEL.url) ?? await fetch(MODEL.url);
  if (!response.ok) throw new Error(`speaker model download failed: ${response.status}`);
  const bytes = await response.clone().arrayBuffer();
  const digest = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  if (digest !== MODEL.sha256) {
    await cache.delete(MODEL.url);
    throw new Error('speaker model failed its integrity check');
  }
  await cache.put(MODEL.url, response).catch(() => {});
  return bytes;
}

function session() {
  loading ??= modelBytes().then((bytes) => ort.InferenceSession.create(bytes));
  loading.catch(() => { loading = undefined; });
  return loading;
}

async function start({ id, voiceprint, enroll }) {
  const model = await session();
  let spent = 0;
  const embed = async (features, frames) => {
    const started = performance.now();
    const output = await model.run({ [model.inputNames[0]]: new ort.Tensor('float32', features, [1, frames, 80]) });
    spent = Math.round(performance.now() - started);
    return output[model.outputNames[0]].data;
  };
  const post = (event) => postMessage({ id, ...event });
  current = id;
  if (enroll) {
    const enrollment = new SpeakerEnrollment({ embed });
    let reported = 0;
    listener = async (chunk) => {
      if (reported === 1) return;
      const { progress, voiceprint: learned } = await enrollment.push(chunk);
      if (learned) post({ voiceprint: learned });
      else if (progress > reported) post({ progress });
      reported = progress;
    };
  } else {
    const gate = new SpeakerGate({ voiceprint, embed, send: (event) => post('score' in event ? { ...event, ms: spent } : event) });
    listener = (chunk) => gate.push(chunk);
  }
}

onmessage = ({ data }) => {
  if (data.load) session().catch(() => {});
  else if (data.stop) queue = queue.then(() => { if (current === data.id) current = listener = undefined; });
  else if (data.chunk) queue = queue.then(() => current === data.id && listener(data.chunk)).catch((error) => postMessage({ id: data.id, error: String(error?.stack ?? error) }));
  else queue = queue.then(() => start(data)).catch((error) => postMessage({ id: data.id, error: String(error?.stack ?? error) }));
};
