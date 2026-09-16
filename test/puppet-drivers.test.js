import assert from 'node:assert/strict';
import test from 'node:test';
import { EmbeddingActionClassifier, ListeningReactionDriver, TranscriptActionDriver, bracketAction, keywordMood, stripBrackets } from '../docs/puppet-drivers.js';

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
  assert.deepEqual(applied, [{ action: { kind: 'gesture', name: 'wave', score: 1, source: 'classifier' }, timing: { sentence: 'Waving hello.', brackets: [], shadowed: false, playAtMinusNow: 150 } }]);
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

test('bracket tokens name puppet actions after normalization and unknown ones stay unknown', () => {
  assert.deepEqual(bracketAction('nod'), { kind: 'gesture', name: 'nod', source: 'bracket' });
  assert.deepEqual(bracketAction(' Thumbs up '), { kind: 'gesture', name: 'thumbs-up', source: 'bracket' });
  assert.deepEqual(bracketAction('amused'), { kind: 'mood', name: 'amused', source: 'bracket' });
  assert.deepEqual(bracketAction('tongue click'), { kind: 'unknown', name: 'tongue-click', source: 'bracket' });
});

test('stripping removes closed brackets and a trailing open one without touching other text', () => {
  assert.equal(stripBrackets('Yes [nod], sure [thumbs up].'), 'Yes, sure.');
  assert.equal(stripBrackets('Yes [no'), 'Yes');
  assert.equal(stripBrackets('[shrug] No idea.'), ' No idea.');
  assert.equal(stripBrackets('Plain speech.'), 'Plain speech.');
  assert.equal(stripBrackets('x [a] y [b'), 'x y');
});

test('bracket tokens dispatch at their audio time and shadow the classifier for their sentence', async () => {
  let now = 0;
  const applied = [];
  const classified = [];
  const classifier = { classify: async (text) => { classified.push(text); return { kind: 'gesture', name: 'shrug', score: 0.5 }; } };
  const driver = new TranscriptActionDriver((action, timing) => applied.push({ action, timing }), classifier, { now: () => now, minimumMs: 0, schedule: (apply, delay) => { now += delay; apply(); } });
  driver.push('Yes [no', 100);
  driver.push('d], that', 200);
  await driver.tail;
  assert.deepEqual(applied, [{ action: { kind: 'gesture', name: 'nod', source: 'bracket' }, timing: { token: '[nod]', playAtMinusNow: 200 } }]);
  driver.push(' landed [tongue click]. Next.', 300);
  await driver.tail;
  assert.deepEqual(classified, ['Yes, that landed.', 'Next.']);
  assert.deepEqual(applied.slice(1), [
    { action: { kind: 'unknown', name: 'tongue-click', source: 'bracket' }, timing: { token: '[tongue click]', playAtMinusNow: 100 } },
    { action: { kind: 'gesture', name: 'shrug', score: 0.5, source: 'classifier' }, timing: { sentence: 'Yes, that landed.', brackets: ['nod'], shadowed: true, playAtMinusNow: null } },
    { action: { kind: 'gesture', name: 'shrug', score: 0.5, source: 'classifier' }, timing: { sentence: 'Next.', brackets: [], shadowed: false, playAtMinusNow: 0 } },
  ]);
  driver.push('Great [wow!] news [laughs.] indeed. Fine.', 400);
  await driver.tail;
  assert.deepEqual(classified.slice(2), ['Great news indeed.', 'Fine.']);
  assert.deepEqual(applied.slice(4).map(({ action }) => action.name), ['wow!', 'laughs.', 'shrug', 'shrug']);
});

test('a flushed fragment and a reset both settle carried brackets', async () => {
  const applied = [];
  const classifier = { classify: async () => ({ kind: 'none', name: 'neutral' }) };
  const driver = new TranscriptActionDriver((action, timing) => applied.push([action.name, timing.brackets ?? timing.token]), classifier, { minimumMs: 0, schedule: (apply) => apply() });
  driver.push('[wave] bye');
  driver.flush();
  await driver.tail;
  assert.deepEqual(applied, [['wave', '[wave]'], ['neutral', ['wave']]]);
  driver.push('[bow]');
  driver.reset();
  driver.push('Done.');
  await driver.tail;
  assert.deepEqual(applied.at(-1), ['neutral', []]);
});

test('every delta split of a bracketed turn yields the same sentences and actions as one delta', async () => {
  const text = 'Great [wow!] news [laughs.] indeed. ... Ok [nod]. Fine.';
  const play = async (chunks) => {
    const classified = [];
    const applied = [];
    const classifier = { classify: async (sentence) => { classified.push(sentence); return { kind: 'gesture', name: 'shrug' }; } };
    const driver = new TranscriptActionDriver((action, timing) => applied.push(timing.token ?? timing.brackets), classifier, { minimumMs: 0, schedule: (apply) => apply() });
    for (const chunk of chunks) driver.push(chunk);
    driver.flush();
    await driver.tail;
    return { classified, applied };
  };
  const whole = await play([text]);
  assert.deepEqual(whole.classified, ['Great news indeed.', 'Ok.', 'Fine.']);
  assert.deepEqual(whole.applied, ['[wow!]', '[laughs.]', [], '[nod]', ['nod'], []]);
  for (let i = 1; i < text.length; i++) assert.deepEqual(await play([text.slice(0, i), text.slice(i)]), whole, `split at ${i}`);
});
