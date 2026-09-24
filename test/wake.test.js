import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, utimes, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { WAKE_PHRASE } from '../docs/identity.js';
import { CHUNK, RATE, WIDTH, WINDOW, WakeDecision, WakeFeatures, headScore, loadHead, wakeFeatures } from '../docs/wake.js';
import { ingest, speechSpans } from '../scripts/wake.mjs';

function chunk(value) {
  return new Float32Array(CHUNK).fill(value);
}

test('streaming features pass int16-scaled audio with context and slide one embedding per chunk', async () => {
  const seen = [];
  const features = new WakeFeatures(
    async (audio) => {
      seen.push([audio.length, audio[0], audio.at(-1)]);
      return new Float32Array(8 * 32).fill(audio.at(-1) * 10);
    },
    async (frames) => new Float32Array(WIDTH).fill(frames.at(-1)),
  );
  const windows = [];
  for (let index = 0; index < 30; index++) windows.push(await features.push(chunk((index + 1) / 1000)));
  assert.deepEqual(seen[1], [480 + CHUNK, Math.fround(Math.fround(1 / 1000) * 32767), Math.fround(Math.fround(2 / 1000) * 32767)]);
  const first = windows.findIndex(Boolean);
  assert.equal(first, 9 + WINDOW - 1);
  const window = windows[29];
  assert.equal(window.length, WINDOW * WIDTH);
  const last = (index) => (index + 1) / 1000 * 32767 * 10 / 10 + 2;
  assert.ok(Math.abs(window[WINDOW * WIDTH - 1] - last(29)) < 1e-3);
  assert.ok(Math.abs(window[0] - last(29 - WINDOW + 1)) < 1e-3);
});

test('a head scores the standardized window through one hidden layer', () => {
  const encoded = (values) => Buffer.from(Float32Array.from(values).buffer).toString('base64');
  const inputs = WINDOW * WIDTH;
  const head = loadHead({
    mean: encoded(new Array(inputs).fill(1)),
    scale: encoded(new Array(inputs).fill(2)),
    w1: encoded([...new Array(inputs).fill(0.001), ...new Array(inputs).fill(-0.001)]),
    b1: encoded([0.5, 0.25]),
    w2: encoded([2, 3]),
    b2: -1,
    threshold: 0.5,
  });
  const window = new Float32Array(inputs).fill(1.5);
  const hidden = [0.5 + inputs * 0.001, Math.max(0, 0.25 - inputs * 0.001)];
  assert.ok(Math.abs(headScore(head, window) - 1 / (1 + Math.exp(-(-1 + 2 * hidden[0] + 3 * hidden[1])))) < 1e-5);
});

test('crossing the threshold wakes once and then rests for two seconds', () => {
  const decision = new WakeDecision({ threshold: 0.9 });
  const events = [0.1, 0.95, 0.99, ...new Array(24).fill(0.99), 0.98].map((score) => decision.decide(score));
  assert.deepEqual(events.filter(Boolean), [{ wake: 0.95 }, { wake: 0.98 }]);
  assert.equal(events.indexOf(events.find((event) => event?.wake === 0.98)), 27);
});

test('an episode that peaks between the miss level and the threshold is one logged miss', () => {
  const decision = new WakeDecision({ threshold: 0.9 });
  const events = [0.2, 0.5, 0.8, 0.6, 0.3, 0.1].map((score) => decision.decide(score)).filter(Boolean);
  assert.deepEqual(events, [{ miss: 0.8 }]);
});

test('the threshold and the miss level are inclusive and a wake ends its episode without a miss', () => {
  assert.deepEqual(new WakeDecision({ threshold: 0.9 }).decide(0.9), { wake: 0.9 });
  const decision = new WakeDecision({ threshold: 0.9 });
  const events = [0.45, 0.1, 0.6, 0.95, ...new Array(26).fill(0.1)].map((score) => decision.decide(score)).filter(Boolean);
  assert.deepEqual(events, [{ miss: 0.45 }, { wake: 0.95 }]);
});

test('repetitions separated by pauses become one span each and a pause inside a repetition does not split it', () => {
  const tone = (seconds) => Float32Array.from({ length: Math.round(seconds * RATE) }, (_, index) => 0.3 * Math.sin(index / 7));
  const quiet = (seconds) => new Float32Array(Math.round(seconds * RATE));
  const audio = Float32Array.from([...quiet(0.5), ...tone(0.6), ...quiet(0.3), ...tone(0.5), ...quiet(1.5), ...tone(1), ...quiet(1.5), ...tone(0.8), ...quiet(0.5)]);
  const spans = speechSpans(audio, 0.9);
  assert.equal(spans.length, 3);
  assert.equal(speechSpans(audio, 0.2).length, 4);
  assert.ok(Math.abs(spans[0].start / RATE - 0.5) < 0.03 && Math.abs(spans[0].end / RATE - 1.9) < 0.03);
});

test('ingestion splits clips into sessions by time and holds the latest session out for evaluation', async () => {
  const scratch = await mkdtemp(path.join(os.tmpdir(), 'wake-ingest-'));
  try {
    const clips = path.join(scratch, 'clips');
    const corpus = path.join(scratch, 'corpus');
    await mkdir(clips);
    const start = Date.parse('2030-01-01T00:00:00Z');
    for (const [name, minutes] of [['a.ogg', 0], ['b.ogg', 2], ['c.ogg', 90], ['d.ogg', 200], ['e.ogg', 203]]) {
      await writeFile(path.join(clips, name), name);
      await utimes(path.join(clips, name), new Date(start + minutes * 60000), new Date(start + minutes * 60000));
    }
    await ingest(clips, { corpus, phrase: WAKE_PHRASE, ordinary: false });
    const manifest = JSON.parse(await readFile(path.join(corpus, 'manifest.json'), 'utf8'));
    assert.deepEqual(manifest.clips.map(({ file, split }) => [path.basename(file), split]), [['a.ogg', 'train'], ['b.ogg', 'train'], ['c.ogg', 'train'], ['d.ogg', 'eval'], ['e.ogg', 'eval']]);
    assert.equal(new Set(manifest.clips.map(({ session }) => session)).size, 3);
    assert.ok(manifest.clips.every(({ phrase }) => phrase === WAKE_PHRASE));
    await assert.rejects(ingest(clips, { corpus: path.join(process.cwd(), 'corpus'), phrase: WAKE_PHRASE, ordinary: false }), /outside the repository/);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});

test('batched features equal the streaming features the browser computes, window for window', async () => {
  const ort = (await import('onnxruntime-node')).default;
  const create = await wakeFeatures(ort, (name) => ort.InferenceSession.create(fileURLToPath(new URL(`../docs/wake/${name}.onnx`, import.meta.url))));
  let seed = 7;
  const noise = () => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed / 2147483648 - 0.5; };
  const audio = Float32Array.from({ length: CHUNK * 60 + 300 }, (_, index) => 0.02 * noise() + (index > 20000 && index < 50000 ? 0.3 * Math.sin(index / 9) * Math.sin(index / 2000) : 0));
  const stream = create();
  const streamed = [];
  let first = -1;
  for (let chunk = 0; chunk < 60; chunk++) {
    await stream.push(audio.subarray(chunk * CHUNK, (chunk + 1) * CHUNK));
    if (!stream.count) continue;
    if (first < 0) first = chunk;
    streamed.push(...stream.embeddings.subarray((WINDOW - 1) * WIDTH));
  }
  const batched = await create.all(audio);
  assert.equal(batched.first, first);
  assert.equal(batched.embeddings.length, streamed.length);
  assert.ok(streamed.every((value, index) => Math.abs(value - batched.embeddings[index]) < 1e-4));
});

test('the demo model listens for the configured phrase, carries its measurements, and stays asleep in silence and noise', async () => {
  const model = JSON.parse(await readFile(new URL('../scripts/wake-demo.json', import.meta.url), 'utf8'));
  assert.equal(model.phrase, WAKE_PHRASE);
  const head = loadHead(model);
  assert.ok(Object.keys(model.metrics.recall).length && Object.keys(model.metrics.falseAccepts).length);
  const ort = (await import('onnxruntime-node')).default;
  const create = await wakeFeatures(ort, (name) => ort.InferenceSession.create(fileURLToPath(new URL(`../docs/wake/${name}.onnx`, import.meta.url))));
  let seed = 3;
  const noise = () => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed / 2147483648 - 0.5; };
  for (const audio of [new Float32Array(CHUNK * 80), Float32Array.from({ length: CHUNK * 80 }, () => 0.01 * noise()), Float32Array.from({ length: CHUNK * 80 }, () => 0.3 * noise())]) {
    const { embeddings } = await create.all(audio);
    const decision = new WakeDecision(head);
    for (let index = WINDOW - 1; index < embeddings.length / WIDTH; index++) assert.equal(decision.decide(headScore(head, embeddings.slice((index - WINDOW + 1) * WIDTH, (index + 1) * WIDTH)))?.wake, undefined);
  }
});

test('a model with missing or mismatched weights is refused', () => {
  const encoded = (length) => Buffer.from(new Float32Array(length).buffer).toString('base64');
  const inputs = WINDOW * WIDTH;
  const valid = { phrase: WAKE_PHRASE, threshold: 0.9, b2: 0, mean: encoded(inputs), scale: encoded(inputs), w1: encoded(inputs * 2), b1: encoded(2), w2: encoded(2) };
  assert.doesNotThrow(() => loadHead(valid));
  for (const broken of [{ ...valid, w1: encoded(inputs) }, { ...valid, w2: encoded(3) }, { ...valid, threshold: 1 }, { ...valid, b2: undefined }, { ...valid, mean: encoded(10) }, { ...valid, scale: undefined }, { ...valid, b1: 'not base64!' }]) assert.throws(() => loadHead(broken), /invalid wake model/);
});
