import assert from 'node:assert/strict';
import test from 'node:test';
import { ACTION_INSTRUCTIONS, EmbeddingActionClassifier, LABELS, TranscriptActionDriver, keywordMood } from '../docs/puppet-drivers.js';

const vectors = { yes: [1, 0], agree: [1, 0], sorry: [0, 1], mistake: [0, 1] };
const loadEmbedder = async () => async (texts) => texts.map((text) => {
  const words = text.toLowerCase().match(/[a-z]+/g) ?? [];
  return words.reduce((sum, word) => sum.map((value, i) => value + (vectors[word]?.[i] ?? 0)), [0.01, 0.01]);
});

test('the embedding classifier uses semantic centroids behind one action interface', async () => {
  const classifier = new EmbeddingActionClassifier(loadEmbedder);
  const nod = await classifier.classify('yes, I agree');
  assert.equal(nod.kind, 'gesture');
  assert.equal(nod.name, 'nod');
  const apology = await classifier.classify('sorry about my mistake');
  assert.equal(apology.kind, 'mood');
  assert.equal(apology.name, 'apologetic');
});

test('complete streamed sentences dispatch once and reset invalidates old work', async () => {
  const waiting = [];
  const classifier = { classify: (text) => new Promise((resolve) => waiting.push({ text, resolve })) };
  const applied = [];
  const driver = new TranscriptActionDriver((action) => applied.push(action), classifier);
  driver.push('First sen');
  driver.push('tence. Second');
  await Promise.resolve();
  assert.equal(waiting.length, 1);
  driver.reset();
  waiting[0].resolve({ kind: 'gesture', name: 'nod' });
  await Promise.resolve();
  assert.deepEqual(applied, []);
});

test('the keyword baseline only emits moods', () => {
  assert.deepEqual(keywordMood('This is an urgent warning'), { kind: 'mood', name: 'alert' });
  assert.equal(keywordMood('I concur with that conclusion'), null);
});

test('the spoken demonstration list reaches the intended actions through completed sentences', async () => {
  const expected = [
    ['Yes.', 'gesture', 'nod'],
    ['No idea.', 'gesture', 'shrug'],
    ['Hmm.', 'mood', 'thinking'],
    ['Happy.', 'mood', 'pleased'],
    ['Sad.', 'mood', 'sad'],
    ['Angry.', 'mood', 'angry'],
    ['Relaxed.', 'mood', 'relaxed'],
    ['Surprised.', 'mood', 'surprised'],
  ];
  const classifier = { classify: async (text) => {
    const [, kind, name] = expected.find(([sentence]) => sentence === text);
    return { kind, name };
  } };
  const applied = [];
  const driver = new TranscriptActionDriver((action) => applied.push([action.kind, action.name]), classifier, { minimumMs: 0, schedule: (apply) => apply() });
  for (const [sentence] of expected) driver.push(sentence);
  await driver.tail;
  assert.deepEqual(applied, expected.map(([, kind, name]) => [kind, name]));
});

test('applied actions include sentence and audio scheduling evidence', async () => {
  let now = 100;
  const applied = [];
  const classifier = { classify: async () => ({ kind: 'gesture', name: 'wave', score: 1 }) };
  const driver = new TranscriptActionDriver((action, timing) => applied.push({ action, timing }), classifier, { now: () => now, minimumMs: 0, schedule: (apply, delay) => { now += delay; apply(); } });
  driver.push('Waving hello.', 250);
  await driver.tail;
  assert.deepEqual(applied, [{ action: { kind: 'gesture', name: 'wave', score: 1, source: 'classifier' }, timing: { sentence: 'Waving hello.', playAtMinusNow: 150 } }]);
});

test('transcript-ahead actions wait for audio and retain a minimum spoken-order dwell', async () => {
  let now = 1000;
  const scheduled = [];
  const applied = [];
  const classifier = { classify: async (text) => ({ kind: 'mood', name: text.startsWith('First') ? 'pleased' : 'sad' }) };
  const driver = new TranscriptActionDriver((action) => applied.push(action.name), classifier, { now: () => now, minimumMs: 900, schedule: (apply, delay) => scheduled.push({ apply, delay }) });
  driver.push('First. Second.', 1500);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(scheduled[0].delay, 500);
  now = 1500;
  scheduled.shift().apply();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(scheduled[0].delay, 900);
  now = 2400;
  scheduled.shift().apply();
  await driver.tail;
  assert.deepEqual(applied, ['pleased', 'sad']);
});

test('bracketed tokens split across deltas play at their audio time, leave the spoken text, and replace the classifier for their sentence', async () => {
  const applied = [];
  const compared = [];
  const classified = [];
  const classifier = { classify: async (text) => { classified.push(text); return { kind: 'gesture', name: 'shrug', score: 0.5 }; } };
  const driver = new TranscriptActionDriver((action, timing) => applied.push({ action, timing }), classifier, { now: () => 0, minimumMs: 0, schedule: (apply) => apply() }, (comparison) => compared.push(comparison));
  const spoken = [driver.push('[no', 100), driver.push('d] Yes, [Thumbs Up] that', 200), driver.push(' works. Maybe.', 300)];
  await driver.tail;
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(spoken, ['', ' Yes, that', ' works. Maybe.']);
  assert.deepEqual(applied.map(({ action }) => [action.source, action.kind, action.name]), [['bracket', 'gesture', 'nod'], ['bracket', 'gesture', 'thumbs-up'], ['classifier', 'gesture', 'shrug']]);
  assert.deepEqual(applied.map(({ timing }) => timing.playAtMinusNow), [200, 200, 300]);
  assert.deepEqual(classified, ['Yes, that works.', 'Maybe.']);
  assert.deepEqual(compared, [{ sentence: 'Yes, that works.', annotations: [{ kind: 'gesture', name: 'nod' }, { kind: 'gesture', name: 'thumbs-up' }], classifier: { kind: 'gesture', name: 'shrug', score: 0.5 } }]);
});

test('unknown bracketed tokens are stripped and logged without displacing the classifier', async () => {
  const applied = [];
  const classifier = { classify: async () => ({ kind: 'mood', name: 'amused' }) };
  const driver = new TranscriptActionDriver((action, timing) => applied.push([action.source, action.kind, action.name, timing.token]), classifier, { minimumMs: 0, schedule: (apply) => apply() });
  assert.equal(driver.push('Ha [tongue click] fine.'), 'Ha fine.');
  await driver.tail;
  assert.deepEqual(applied, [['bracket', 'unknown', 'tongue-click', 'tongue click'], ['classifier', 'mood', 'amused', undefined]]);
});

test('the end of a turn compares its unpunctuated annotated tail and leaves nothing for the next turn', async () => {
  const applied = [];
  const compared = [];
  const classifier = { classify: async (text) => ({ kind: 'mood', name: text === 'Sure thing' ? 'pleased' : 'sad' }) };
  const driver = new TranscriptActionDriver((action) => applied.push([action.source, action.name]), classifier, { minimumMs: 0, schedule: (apply) => apply() }, (comparison) => compared.push([comparison.sentence, comparison.classifier.name]));
  driver.push('[nod] Sure thing');
  driver.endTurn();
  driver.push('Fine. [shrug] [wav');
  driver.endTurn();
  driver.push('Next one.');
  await driver.tail;
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(compared, [['Sure thing', 'pleased']]);
  assert.deepEqual(applied, [['bracket', 'nod'], ['classifier', 'sad'], ['bracket', 'shrug'], ['classifier', 'sad']]);
});

test('an unclosed bracket longer than any token is spoken text', () => {
  const driver = new TranscriptActionDriver(() => {}, { classify: async () => ({ kind: 'none', name: 'neutral' }) });
  assert.equal(driver.push('A [b'), 'A ');
  assert.equal(driver.push('x'.repeat(40)), '[b' + 'x'.repeat(40));
});

test('the action instructions name every gesture and mood token', () => {
  for (const kind of ['gesture', 'mood']) for (const name of Object.keys(LABELS[kind])) assert.ok(ACTION_INSTRUCTIONS.includes(`[${name}]`), name);
});

for (const outcome of ['missing', 'null', 'reject', 'throw', 'available']) {
  test('WebGPU probe explains CPU fallback once: ' + outcome, async (t) => {
    let probes = 0;
    const adapter = {};
    const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
    t.after(() => Object.defineProperty(globalThis, 'navigator', original));
    Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { gpu: outcome === 'missing' ? undefined : { requestAdapter() {
      probes++;
      if (outcome === 'throw') throw new Error('unavailable');
      if (outcome === 'reject') return Promise.reject(new Error('unavailable'));
      return Promise.resolve(outcome === 'available' ? adapter : null);
    } } } });
    const { webGpuAdapter } = await import('../docs/puppet-drivers.js?probe=' + outcome);
    const info = t.mock.method(console, 'info', () => {});
    assert.deepEqual(await Promise.all([webGpuAdapter(), webGpuAdapter()]), outcome === 'available' ? [adapter, adapter] : [null, null]);
    assert.equal(probes, outcome === 'missing' ? 0 : 1);
    assert.equal(info.mock.callCount(), outcome === 'available' ? 0 : 1);
    if (outcome !== 'available') assert.match(info.mock.calls[0].arguments[0], /WebAssembly CPU fallback/);
  });
}
