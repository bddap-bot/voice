import * as ort from 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.22.0/dist/ort.wasm.min.mjs';
import { WakeDecision, headScore, loadHead, wakeFeatures } from './wake.js';

ort.env.wasm.wasmPaths = 'https://cdn.jsdelivr.net/npm/onnxruntime-web@1.22.0/dist/';
let detect;
let queue = Promise.resolve();
const fail = (error) => postMessage({ error: String(error?.stack ?? error) });

async function start(model) {
  const features = (await wakeFeatures(ort, (name) => ort.InferenceSession.create(new URL(`./wake/${name}.onnx`, import.meta.url).href)))();
  const head = loadHead(model);
  const decision = new WakeDecision(head);
  detect = async (chunk) => {
    const window = await features.push(chunk);
    return window && decision.decide(headScore(head, window));
  };
  postMessage({ ready: true });
}

onmessage = ({ data }) => {
  if (data.model) return void start(data.model).catch(fail);
  queue = queue.then(async () => {
    const event = detect && await detect(data);
    if (event) postMessage(event);
  }).catch(fail);
};
