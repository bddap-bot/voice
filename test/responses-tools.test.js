import assert from 'node:assert/strict';
import test from 'node:test';
import { ResponsesTools } from '../docs/responses-tools.js';

const envelope = (event, delegation_id = 'delegation') => ({ type: 'response.event', delegation_id, event });
const created = (id = 'r1') => ({ type: 'response.created', response: { id, output: [] } });
const completed = (id = 'r1') => ({ type: 'response.completed', response: { id, output: [] } });
const call = (call_id, name = 'gesture') => ({ type: 'response.output_item.done', item: { type: 'function_call', call_id, name, arguments: '{"name":"nod"}' } });

test('wrapped completed calls execute once and all results precede one continuation', async () => {
  const sent = [], executed = [];
  let finish;
  const loop = new ResponsesTools((item, delegation) => {
    executed.push([item.call_id, delegation]);
    return item.call_id === 'a' ? new Promise((resolve) => { finish = resolve; }) : { ok: true };
  }, (event) => sent.push(event));
  loop.handle(envelope(created()));
  loop.handle(call('unwrapped'));
  loop.handle(envelope({ type: 'response.function_call_arguments.done', call_id: 'args', name: 'gesture' }));
  loop.handle(envelope(call('a')));
  loop.handle(envelope(call('a')));
  loop.handle(envelope(call('b')));
  const pending = loop.handle(envelope(completed()));
  await Promise.resolve();
  assert.deepEqual(executed, [['a', 'delegation'], ['b', 'delegation']]);
  assert.deepEqual(sent, []);
  finish({ ok: true });
  await pending;
  assert.deepEqual(sent, [
    { type: 'response.item.create', item: { type: 'function_call_output', call_id: 'a', output: '{"ok":true}' } },
    { type: 'response.item.create', item: { type: 'function_call_output', call_id: 'b', output: '{"ok":true}' } },
    { type: 'response.create' },
  ]);
  await loop.handle(envelope(completed()));
  assert.equal(sent.length, 3);
});

test('failures return errors, empty responses do not loop, ended sessions do not send', async () => {
  const sent = [];
  let valid = true;
  const loop = new ResponsesTools(() => { throw Error('private detail'); }, (event) => sent.push(event), () => valid);
  loop.handle(envelope(created()));
  await loop.handle(envelope(completed()));
  assert.deepEqual(sent, []);
  loop.handle(envelope(created('r2')));
  loop.handle(envelope(call('a')));
  await loop.handle(envelope(completed('r2')));
  assert.deepEqual(JSON.parse(sent[0].item.output), { ok: false, error: 'tool execution failed' });
  loop.handle(envelope(created('r3')));
  loop.handle(envelope(call('b')));
  valid = false;
  await loop.handle(envelope(completed('r3')));
  assert.equal(sent.length, 2);
});

test('interleaved delegations retain their response and call association', async () => {
  const sent = [];
  const loop = new ResponsesTools(() => ({ ok: true }), (event) => sent.push(event));
  loop.handle(envelope(created('r1'), 'd1'));
  loop.handle(envelope(created('r2'), 'd2'));
  loop.handle(envelope(call('a'), 'd1'));
  loop.handle(envelope(call('b'), 'd2'));
  await loop.handle(envelope(completed('r2'), 'd1'));
  assert.deepEqual(sent, []);
  await loop.handle(envelope(completed('r2'), 'd2'));
  await loop.handle(envelope(completed('r1'), 'd1'));
  assert.deepEqual(sent.filter((event) => event.item).map((event) => event.item.call_id), ['b', 'a']);
});

test('closing a pending call prevents continuation even while the channel stays valid', async () => {
  const sent = [];
  let resolve;
  const loop = new ResponsesTools(() => new Promise((done) => { resolve = done; }), (event) => sent.push(event), () => true);
  loop.handle(envelope(created()));
  loop.handle(envelope(call('a', 'hub')));
  const completion = loop.handle(envelope(completed()));
  loop.close();
  resolve({ ok: false, error: 'session ended' });
  await completion;
  assert.deepEqual(sent, []);
});
