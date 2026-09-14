import assert from 'node:assert/strict';
import test from 'node:test';
import { EmbeddingActionClassifier, TranscriptActionDriver, keywordMood } from '../docs/puppet-drivers.js';

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
