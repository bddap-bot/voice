import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { PuppetTools, completedToolCall } from '../docs/puppet-tools.js';

test('completed function items are the only executable tool events', () => {
  const item = { type: 'function_call', call_id: 'call_1', name: 'mood', arguments: '{"name":"amused"}' };
  assert.equal(completedToolCall({ type: 'response.output_item.done', item }), item);
  assert.equal(completedToolCall({ type: 'response.function_call_arguments.done', ...item }), null);
  assert.equal(completedToolCall({ type: 'response.output_item.done', item: { type: 'message' } }), null);
});

test('puppet tools dispatch locally and randomization never exposes an id', async () => {
  const calls = [];
  const runtime = {
    pose: (...args) => calls.push(['pose', ...args]),
    gesture: (...args) => calls.push(['gesture', ...args]),
    look: (...args) => calls.push(['look', ...args]),
    mood: (...args) => calls.push(['mood', ...args]),
  };
  let active = 'a';
  const selected = [];
  let receivedGuard;
  const tools = new PuppetTools(runtime, () => [{ id: 'a' }, { id: 'b' }, { id: 'c' }], () => active, async (id, valid) => { selected.push(id); receivedGuard = valid; active = id; });
  assert.deepEqual(await tools.execute('pose', '{"name":"listen"}'), { ok: true });
  assert.deepEqual(await tools.execute('gesture', '{"name":"point_at","target":"panel"}'), { ok: true });
  assert.deepEqual(await tools.execute('look', '{"direction":"away"}'), { ok: true });
  assert.deepEqual(await tools.execute('mood', '{"name":"skeptical"}'), { ok: true });
  assert.deepEqual(calls, [['pose', 'listen'], ['gesture', 'point_at', 'panel'], ['look', 'away'], ['mood', 'skeptical']]);
  const guard = () => false;
  const result = await tools.execute('randomize_appearance', '{"seed":"stable"}', guard);
  assert.deepEqual(result, { ok: true, changed: true });
  assert.equal(selected.length, 1);
  assert.notEqual(selected[0], 'a');
  assert.equal(receivedGuard, guard);
  assert.equal('id' in result, false);
});

test('invalid tool names and arguments fail closed', async () => {
  const tools = new PuppetTools({}, () => [], () => '', async () => {});
  await assert.rejects(tools.execute('pose', '{'), /invalid tool arguments/);
  await assert.rejects(tools.execute('unknown', '{}'), /unknown puppet tool/);
  await assert.rejects(tools.execute('randomize_appearance', '{"seed":""}'), /invalid appearance seed/);
});

test('the scripted expression run calls every action before action-free speech', async () => {
  const proof = JSON.parse(await readFile(new URL('../docs/proofs/issue-45-action-transcript.json', import.meta.url)));
  const calls = [];
  const tools = new PuppetTools({ gesture: (name) => calls.push(['gesture', name]), mood: (name) => calls.push(['mood', name]) }, () => [], () => '', async () => {});
  const actions = [];
  for (let index = 0; index < proof.events.length; index += 2) {
    const call = proof.events[index];
    const spoken = proof.events[index + 1];
    assert.equal(call.type, 'tool_call');
    assert.equal(spoken.type, 'spoken');
    await tools.execute(call.name, JSON.stringify(call.arguments));
    actions.push(call.arguments.name);
    assert.equal(new RegExp(`\\b${call.arguments.name.replace('-', '[ -]')}\\b`, 'i').test(spoken.text), false);
  }
  assert.deepEqual(calls, proof.events.filter((event) => event.type === 'tool_call').map((event) => [event.name, event.arguments.name]));
  assert.deepEqual(actions, ['nod', 'shrug', 'think', 'point', 'wave', 'no', 'laugh', 'clap', 'bow', 'thumbs-up', 'stretch', 'look-around', 'apologetic', 'surprised', 'amused', 'pleased', 'sad', 'angry', 'puzzled', 'skeptical', 'thinking', 'alert', 'sleepy', 'relaxed', 'curious']);
  const speech = proof.events.filter((event) => event.type === 'spoken').map((event) => event.text).join(' ');
  for (const action of actions) assert.equal(new RegExp(`\\b${action.replace('-', '[ -]')}\\b`, 'i').test(speech), false);
});
