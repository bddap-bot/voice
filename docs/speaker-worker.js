import * as ort from 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.22.0/dist/ort.wasm.min.mjs';
import { MODEL, SpeakerEnrollment, SpeakerGate } from './speaker.js';

ort.env.wasm.wasmPaths = 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.22.0/dist/';
let listener;
let queue = Promise.resolve();
const fail = (error) => postMessage({ error: String(error?.stack ?? error) });

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
  await cache.put(MODEL.url, response);
  return bytes;
}

async function start(voiceprint) {
  const session = await ort.InferenceSession.create(await modelBytes());
  let spent = 0;
  const embed = async (features, frames) => {
    const started = performance.now();
    const output = await session.run({ [session.inputNames[0]]: new ort.Tensor('float32', features, [1, frames, 80]) });
    spent = Math.round(performance.now() - started);
    return output[session.outputNames[0]].data;
  };
  if (!voiceprint) {
    const enrollment = new SpeakerEnrollment({ embed });
    let reported = 0;
    listener = async (chunk) => {
      const { progress, voiceprint: learned } = await enrollment.push(chunk);
      if (learned) postMessage({ voiceprint: learned });
      else if (progress > reported) postMessage({ progress: reported = progress });
    };
  } else {
    const gate = new SpeakerGate({ voiceprint, embed, send: (event) => postMessage('score' in event ? { ...event, ms: spent } : event) });
    listener = (chunk) => gate.push(chunk);
  }
  postMessage({ ready: true });
}

onmessage = ({ data }) => {
  if (!(data instanceof Float32Array)) {
    queue = queue.then(() => start(data.voiceprint)).catch(fail);
    return;
  }
  queue = queue.then(() => listener?.(data)).catch(fail);
};
