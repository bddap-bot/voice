import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { PuppetTools } from '../docs/puppet-tools.js';

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function recordingRuntime(calls) {
  return {
    pose: (...args) => calls.push(['pose', ...args]),
    gesture: (...args) => calls.push(['gesture', ...args]),
    look: (...args) => calls.push(['look', ...args]),
    mood: (...args) => calls.push(['mood', ...args]),
  };
}

test('a performance answers once its first step plays, then plays the rest in order a step apart, a gesture taking the place of a pose', async () => {
  const calls = [];
  const pauses = [];
  const waiting = [];
  const tools = new PuppetTools(recordingRuntime(calls), () => [], () => '', async () => {}, (ms) => { pauses.push([ms, calls.length]); return new Promise((resolve) => waiting.push(resolve)); });
  const steps = [{ pose: 'listen' }, { pose: 'stand', gesture: 'point', target: 'panel', mood: 'skeptical' }, { look: 'away' }];
  assert.deepEqual(await tools.execute('perform', JSON.stringify({ steps })), { ok: true });
  assert.deepEqual(calls, [['pose', 'listen']]);
  waiting.shift()();
  await tick();
  assert.deepEqual(calls, [['pose', 'listen'], ['gesture', 'point', 'panel'], ['mood', 'skeptical']]);
  waiting.shift()();
  await tick();
  assert.deepEqual(calls, [['pose', 'listen'], ['gesture', 'point', 'panel'], ['mood', 'skeptical'], ['look', 'away']]);
  assert.deepEqual(pauses, [[1500, 1], [1500, 3]]);
  assert.equal(waiting.length, 0);
});

test('a newer performance or a closed conversation stops the one still playing', async () => {
  const calls = [];
  const waiting = [];
  let current = true;
  const tools = new PuppetTools(recordingRuntime(calls), () => [], () => '', async () => {}, () => new Promise((resolve) => waiting.push(resolve)));
  await tools.execute('perform', JSON.stringify({ steps: [{ gesture: 'nod' }, { gesture: 'wave' }, { gesture: 'clap' }] }));
  await tools.execute('perform', JSON.stringify({ steps: [{ gesture: 'shrug' }, { mood: 'amused' }] }), () => current);
  waiting.shift()();
  await tick();
  current = false;
  waiting.shift()();
  await tick();
  assert.deepEqual(calls, [['gesture', 'nod', undefined], ['gesture', 'shrug', undefined]]);
  assert.equal(waiting.length, 0);
});

test('an empty step plays nothing for one step', async () => {
  const calls = [];
  const pauses = [];
  const tools = new PuppetTools(recordingRuntime(calls), () => [], () => '', async () => {}, async (ms) => { pauses.push([ms, calls.length]); });
  await tools.execute('perform', JSON.stringify({ steps: [{ gesture: 'nod' }, {}, { target: 'panel' }, { gesture: 'wave' }] }));
  await tick();
  assert.deepEqual(calls, [['gesture', 'nod', undefined], ['gesture', 'wave', undefined]]);
  assert.deepEqual(pauses, [[1500, 1], [1500, 1], [1500, 1]]);
});

test('randomization picks another puppet without exposing an id', async () => {
  let active = 'a';
  const selected = [];
  let receivedGuard;
  const tools = new PuppetTools({}, () => [{ id: 'a' }, { id: 'b' }, { id: 'c' }], () => active, async (id, valid) => { selected.push(id); receivedGuard = valid; active = id; }, async () => {});
  const guard = () => false;
  const result = await tools.execute('randomize_appearance', '{"seed":"stable"}', guard);
  assert.deepEqual(result, { ok: true, changed: true });
  assert.equal(selected.length, 1);
  assert.notEqual(selected[0], 'a');
  assert.equal(receivedGuard, guard);
  assert.equal('id' in result, false);
});

test('malformed tool calls fail closed without stopping the performance already playing', async () => {
  const calls = [];
  const waiting = [];
  const tools = new PuppetTools(recordingRuntime(calls), () => [], () => '', async () => {}, () => new Promise((resolve) => waiting.push(resolve)));
  await tools.execute('perform', JSON.stringify({ steps: [{ gesture: 'nod' }, { gesture: 'wave' }] }));
  await assert.rejects(tools.execute('perform', '{'), /invalid tool arguments/);
  for (const steps of [[], ['nod'], [null], [[]], [{ gesture: 'nod', speed: 'fast' }], [{ mood: 3 }], Array(33).fill({ gesture: 'nod' })]) await assert.rejects(tools.execute('perform', JSON.stringify({ steps })), /invalid performance steps/);
  await assert.rejects(tools.execute('pose', '{"name":"sit"}'), /unknown puppet tool/);
  await assert.rejects(tools.execute('randomize_appearance', '{"seed":""}'), /invalid appearance seed/);
  waiting.shift()();
  await tick();
  assert.deepEqual(calls, [['gesture', 'nod', undefined], ['gesture', 'wave', undefined]]);
});

test('one performance plays every gesture and mood, and the scripted lines name none of them', async () => {
  const proof = JSON.parse(await readFile(new URL('../docs/proofs/issue-45-action-transcript.json', import.meta.url)));
  const calls = [];
  const tools = new PuppetTools({ gesture: (name) => calls.push(['gesture', name]), mood: (name) => calls.push(['mood', name]) }, () => [], () => '', async () => {}, async () => {});
  const requested = proof.events.filter((event) => event.type === 'tool_call');
  assert.deepEqual(await tools.execute('perform', JSON.stringify({ steps: requested.map((call) => ({ [call.name]: call.arguments.name })) })), { ok: true });
  await tick();
  assert.deepEqual(calls, requested.map((call) => [call.name, call.arguments.name]));
  const actions = requested.map((call) => call.arguments.name);
  assert.deepEqual(actions, ['nod', 'shrug', 'think', 'point', 'wave', 'no', 'laugh', 'clap', 'bow', 'thumbs-up', 'stretch', 'look-around', 'apologetic', 'surprised', 'amused', 'pleased', 'sad', 'angry', 'puzzled', 'skeptical', 'thinking', 'alert', 'sleepy', 'relaxed', 'curious']);
  const speech = proof.events.filter((event) => event.type === 'spoken').map((event) => event.text).join(' ');
  for (const action of actions) assert.equal(new RegExp(`\\b${action.replace('-', '[ -]')}\\b`, 'i').test(speech), false);
});
