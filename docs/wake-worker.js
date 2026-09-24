import { transformers } from './transformers.js';
import { WAKE_MODEL } from './wake.js';

const recognizer = transformers().then(({ pipeline }) => pipeline('automatic-speech-recognition', WAKE_MODEL.id, { dtype: WAKE_MODEL.dtype, device: 'wasm' }));
let next = null;
let running = false;

recognizer.then(() => postMessage({ ready: true }), (error) => postMessage({ error: String(error?.stack ?? error) }));

onmessage = ({ data }) => {
  next = data;
  if (!running) transcribe();
};

async function transcribe() {
  running = true;
  try {
    const recognize = await recognizer;
    while (next) {
      const audio = next;
      next = null;
      postMessage({ text: (await recognize(audio)).text });
    }
  } catch (error) {
    postMessage({ error: String(error?.stack ?? error) });
  }
  running = false;
}
