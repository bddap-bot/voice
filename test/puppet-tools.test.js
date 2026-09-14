import assert from 'node:assert/strict';
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
