import assert from 'node:assert/strict';
import test from 'node:test';
import { StageTrace } from '../docs/stage-trace.js';

function recorder() {
  let clock = 1000;
  const sent = [];
  const stages = new StageTrace((span) => sent.push(span), () => clock);
  const at = (ms) => { clock = ms; return stages; };
  const round = (delegation_id, event) => ({ type: 'response.event', delegation_id, event });
  const view = () => sent.map((span) => [span.name, Number(BigInt(span.startTimeUnixNano) / 1000000n), Number(BigInt(span.endTimeUnixNano) / 1000000n)]);
  const attributes = (span) => Object.fromEntries(span.attributes.map(({ key, value }) => [key, value.stringValue]));
  return { stages, at, round, sent, view, attributes };
}

test('an expression turn becomes one trace with a span per stage', async () => {
  const { at, round, sent, view, attributes } = recorder();
  at(1000).inputTranscript();
  at(1800).inputTranscript();
  at(1900).delegationCreated('d1');
  at(1900).responseEvent(round('d1', { type: 'response.created', response: { id: 'r1' } }));
  at(2600).responseEvent(round('d1', { type: 'response.output_item.added', item: { type: 'reasoning' } }));
  at(3600).responseEvent(round('d1', { type: 'response.output_item.done', item: { type: 'reasoning' } }));
  at(3600).responseEvent(round('d1', { type: 'response.output_item.done', item: { type: 'function_call', name: 'pose', call_id: 'c1' } }));
  const result = await at(3600).tool('d1', { name: 'pose', call_id: 'c1', arguments: '{"name":"stand"}' }, async (traceparent) => { at(3601); return { ok: true, traceparent }; });
  at(3900).responseEvent(round('d1', { type: 'response.completed', response: { id: 'r1', output: [] } }));
  at(4200).responseEvent(round('d1', { type: 'response.created', response: { id: 'r2' } }));
  at(4900).responseEvent(round('d1', { type: 'response.output_item.done', item: { type: 'message' } }));
  at(5200).responseEvent(round('d1', { type: 'response.completed', response: { id: 'r2', output: [] } }));
  at(5210).delegationSettled('d1');
  assert.ok(!sent.some((span) => span.name === 'turn'));
  at(5300).outputTranscript(40);
  at(5400).outputTranscript();
  assert.deepEqual(view(), [
    ['hear', 1000, 1800],
    ['decide', 1800, 1900],
    ['think', 2600, 3600],
    ['tool', 3600, 3601],
    ['respond', 1900, 3900],
    ['resume', 3900, 4200],
    ['respond', 4200, 5200],
    ['await-speech', 5210, 5340],
    ['turn', 1000, 5340],
  ]);
  const root = sent.at(-1);
  assert.equal(root.parentSpanId, undefined);
  assert.ok(sent.every((span) => span.traceId === root.traceId && /^[0-9a-f]{32}$/.test(span.traceId) && /^[0-9a-f]{16}$/.test(span.spanId)));
  assert.ok(sent.filter((span) => span.name !== 'turn' && span.name !== 'think').every((span) => span.parentSpanId === root.spanId));
  assert.equal(sent.find((span) => span.name === 'think').parentSpanId, sent.find((span) => span.name === 'respond').spanId);
  assert.deepEqual(sent.filter((span) => span.name === 'respond').map((span) => attributes(span).output), ['reasoning,pose', 'message']);
  const tool = sent.find((span) => span.name === 'tool');
  assert.deepEqual(attributes(tool), { 'tool.name': 'pose', 'tool.arguments': '{"name":"stand"}' });
  assert.equal(tool.status, undefined);
  assert.equal(result.traceparent, `00-${root.traceId}-${tool.spanId}-01`);
});

test('a hub reply resumes the delegation when it lands, and a failed call carries an error status', async () => {
  const { at, round, sent, view } = recorder();
  at(1000).inputTranscript();
  at(1100).delegationCreated('d1');
  at(1100).responseEvent(round('d1', { type: 'response.created', response: { id: 'r1' } }));
  let reply;
  const pending = at(2000).tool('d1', { name: 'hub', call_id: 'c1', arguments: '{}' }, () => new Promise((resolve) => { reply = resolve; }));
  at(2300).responseEvent(round('d1', { type: 'response.completed', response: { id: 'r1', output: [] } }));
  at(2400).outputTranscript();
  at(20000);
  reply({ ok: false, error: 'hub reply timed out' });
  await pending;
  at(20300).responseEvent(round('d1', { type: 'response.created', response: { id: 'r2' } }));
  at(21000).responseEvent(round('d1', { type: 'response.completed', response: { id: 'r2', output: [] } }));
  at(21000).delegationSettled('d1');
  at(21500).outputTranscript();
  assert.deepEqual(view().filter(([name]) => ['tool', 'resume', 'await-speech'].includes(name)), [['tool', 2000, 20000], ['resume', 20000, 20300], ['await-speech', 21000, 21500]]);
  assert.deepEqual(sent.find((span) => span.name === 'tool').status, { code: 2, message: 'failed' });
});

test('an unknown delegation runs its tool untraced', async () => {
  const { stages, sent } = recorder();
  assert.deepEqual(await stages.tool('missing', { name: 'hub', call_id: 'c1' }, async (traceparent) => ({ traceparent })), { traceparent: null });
  assert.deepEqual(sent, []);
});

test('a model reply, a pause, or the listener speaking again each close the turn before it', () => {
  const { at, round, view } = recorder();
  at(500).inputTranscript();
  at(1000).outputTranscript();
  at(1500).inputTranscript();
  at(1550).outputTranscript();
  at(1600).delegationCreated('d0');
  assert.deepEqual(view(), [['hear', 1500, 1500], ['decide', 1500, 1600]]);
  at(1700).delegationSettled('d0');
  at(1800).outputTranscript(Number.NaN);
  assert.deepEqual(view().slice(2), [['await-speech', 1700, 1800], ['turn', 1500, 1800]]);
  at(3000).inputTranscript();
  at(9000).inputTranscript();
  at(9500).delegationCreated('d1');
  at(9500).responseEvent(round('d1', { type: 'response.created', response: { id: 'r1' } }));
  at(10000).responseEvent(round('d1', { type: 'response.completed', response: { id: 'r1', output: [] } }));
  at(10000).delegationSettled('d1');
  at(12000).inputTranscript();
  at(12500).outputTranscript();
  assert.deepEqual(view().slice(4), [['hear', 9000, 9000], ['decide', 9000, 9500], ['respond', 9500, 10000], ['turn', 9000, 10000]]);
});

test('ending the session flushes open turns and cancels their tools', async () => {
  const { stages, at, round, sent, view } = recorder();
  at(1000).delegationCreated('d1');
  at(1000).responseEvent(round('d1', { type: 'response.created', response: { id: 'r1' } }));
  at(1200).responseEvent(round('d1', { type: 'response.output_item.added', item: { type: 'reasoning' } }));
  let finish;
  const pending = at(1500).tool('d1', { name: 'hub', call_id: 'c1', arguments: '{}' }, () => new Promise((resolve) => { finish = resolve; }));
  at(9000).sessionEnded();
  assert.deepEqual(view(), [['tool', 1500, 9000], ['think', 1200, 9000], ['respond', 1000, 9000], ['turn', 1000, 9000]]);
  assert.deepEqual(sent[0].status, { code: 2, message: 'cancelled' });
  finish({ ok: false });
  await pending;
  at(9600).sessionEnded();
  assert.equal(sent.length, 4);
});
