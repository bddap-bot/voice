import assert from 'node:assert/strict';
import test from 'node:test';
import { EmbeddingActionClassifier, ListeningReactionDriver, TranscriptActionDriver, keywordMood } from '../docs/puppet-drivers.js';

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
    ['Pointing at the panel.', 'gesture', 'point'],
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
  assert.deepEqual(applied, [{ action: { kind: 'gesture', name: 'wave', score: 1 }, timing: { sentence: 'Waving hello.', playAtMinusNow: 150 } }]);
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

test('input activity leans in, adds small head motion, then returns to idle after a beat', () => {
  let now = 0;
  const scheduled = [];
  const frames = [];
  const driver = new ListeningReactionDriver((frame) => frames.push({ now, ...frame }), { now: () => now, schedule: (apply) => scheduled.push(apply) });
  const advance = (at) => {
    now = at;
    scheduled.shift()();
  };
  driver.activity(true);
  advance(240);
  advance(1100);
  advance(2300);
  driver.activity(false);
  advance(2500);
  advance(2720);
  assert.equal(frames[1].amount, 1);
  assert.equal(frames[1].lean, -0.055);
  assert.ok(Math.abs(frames[2].nod) > 0.02);
  assert.ok(Math.abs(frames[3].tilt) > 0.01);
  assert.ok(frames[4].amount > 0 && frames[4].amount < 1);
  assert.deepEqual(frames.at(-1), { now: 2720, amount: 0, lean: 0, nod: 0, tilt: 0 });
});
