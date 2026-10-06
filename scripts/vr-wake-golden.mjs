import { existsSync } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
import ort from 'onnxruntime-node';
import { WAKE_PHRASE } from '../docs/identity.js';
import { CHUNK, RATE, WIDTH, WINDOW, WakeDecision, headScore, loadHead, wakeFeatures } from '../docs/wake.js';
import { HELD_OUT, synthesize } from './wake.mjs';

const out = new URL('../vr/golden/wake.json', import.meta.url).pathname;
const head = loadHead(JSON.parse(await readFile(new URL('./wake-demo.json', import.meta.url), 'utf8')));
async function spoken() {
  const [clip] = await synthesize([...HELD_OUT][0], 1, [WAKE_PHRASE]);
  const pad = new Float32Array(RATE);
  return Int16Array.from([...pad, ...clip, ...pad, ...pad], (sample) => Math.round(Math.max(-1, Math.min(1, sample)) * 32767));
}
const kept = existsSync(out) && process.argv[2] !== '--resynthesize' ? Buffer.from(JSON.parse(await readFile(out, 'utf8')).audio, 'base64') : null;
const pcm = kept ? new Int16Array(kept.buffer, kept.byteOffset, kept.byteLength / 2) : await spoken();
const audio = Float32Array.from(pcm, (sample) => sample / 32767);
const stream = (await wakeFeatures(ort, (name) => ort.InferenceSession.create(new URL(`../docs/wake/${name}.onnx`, import.meta.url).pathname)))();
const decision = new WakeDecision(head);
const embeddings = [];
const scores = [];
const events = [];
for (let chunk = 0; (chunk + 1) * CHUNK <= audio.length; chunk++) {
  const window = await stream.push(audio.subarray(chunk * CHUNK, (chunk + 1) * CHUNK));
  if (!window) continue;
  window.subarray((WINDOW - 1) * WIDTH).forEach((value, index) => { if (index % 7 === 0) embeddings.push(+value.toFixed(5)); });
  const score = headScore(head, window);
  scores.push(+score.toFixed(6));
  const event = decision.decide(score);
  if (event) events.push({ chunk, kind: event.wake ? 'wake' : 'miss' });
}
if (!events.some((event) => event.kind === 'wake')) throw new Error('the demo model did not wake on its phrase');
await writeFile(out, JSON.stringify({
  about: 'docs/wake.js and scripts/wake-demo.json on the page phrase in a held-out voice: written by scripts/vr-wake-golden.mjs, compared by the overlay host build',
  audio: Buffer.from(pcm.buffer).toString('base64'),
  embeddings,
  scores,
  events,
}) + '\n');
