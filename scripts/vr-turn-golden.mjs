import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import * as ort from 'onnxruntime-web';
import { FRAMES, MELS, RATE, TURN, features } from '../docs/turn.js';

const out = new URL('../vr/golden/turn.json', import.meta.url).pathname;
const cache = new URL('../node_modules/.cache/voice-turn/', import.meta.url).pathname;
const nix = readFileSync(new URL('../vr/default.nix', import.meta.url), 'utf8');
const [, base] = nix.match(/url = "(https:\/\/raw\.githubusercontent\.com\/csukuangfj\/sr-data\/[0-9a-f]+\/)\$\{path\}"/);
const clip = (name) => {
  const [, path, sha256] = nix.match(new RegExp(`${name} = \\{ path = "([^"]+)"; sha256 = "([0-9a-f]+)"; \\};`));
  return { url: base + path, sha256 };
};
const REFERENCE = [['leijun-test-sr-1', 3.2, 0.9903], ['leijun-test-sr-1', 4.0, 0.0298], ['fangjun-test-sr-1', 2.5, 0.9881], ['fangjun-test-sr-1', 3.4, 0.0139]];

async function pinned({ url, sha256 }) {
  const file = cache + sha256;
  if (existsSync(file)) return readFileSync(file);
  const response = await fetch(url, { signal: AbortSignal.timeout(120000) });
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (createHash('sha256').update(bytes).digest('hex') !== sha256) throw new Error(`${url} failed its integrity check`);
  mkdirSync(cache, { recursive: true });
  writeFileSync(file, bytes);
  return bytes;
}

function samples(wav) {
  const view = new DataView(wav.buffer, wav.byteOffset, wav.byteLength);
  let at = 12;
  let format;
  while (at < wav.length) {
    const id = wav.toString('ascii', at, at + 4);
    const size = view.getUint32(at + 4, true);
    if (id === 'fmt ') format = { channels: view.getUint16(at + 10, true), rate: view.getUint32(at + 12, true), bits: view.getUint16(at + 22, true) };
    if (id === 'data') {
      if (format.channels !== 1 || format.rate !== RATE || format.bits !== 16) throw new Error(`unexpected clip format ${JSON.stringify(format)}`);
      return Float32Array.from({ length: size / 2 }, (_, index) => view.getInt16(at + 8 + 2 * index, true) / 32768);
    }
    at += 8 + size + (size & 1);
  }
  throw new Error('clip has no data');
}

const model = await ort.InferenceSession.create(await pinned(TURN));
const judgements = [];
for (const [name, seconds, reference] of REFERENCE) {
  const audio = samples(await pinned(clip(name)));
  const input = features(audio.subarray(0, Math.round(seconds * RATE)));
  const output = await model.run({ [model.inputNames[0]]: new ort.Tensor('float32', input, [1, MELS, FRAMES]) });
  const probability = output[model.outputNames[0]].data[0];
  console.log(`${name} at ${seconds} s: ${probability.toFixed(4)}, reference ${reference}`);
  if (Math.abs(probability - reference) >= 0.01) throw new Error(`${name} at ${seconds} s: ${probability} against the reference ${reference}`);
  judgements.push({ name, seconds, probability: +probability.toFixed(4) });
}
writeFileSync(out, JSON.stringify({
  about: 'docs/turn.js with onnxruntime-web on public speech clips: written by scripts/vr-turn-golden.mjs, compared by the overlay host build',
  judgements,
}, null, 1) + '\n');
