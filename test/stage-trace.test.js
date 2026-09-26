import assert from 'node:assert/strict';
import test from 'node:test';
import { StageTrace } from '../docs/stage-trace.js';

function recorder() {
  let clock = 1000;
  const sent = [];
  const stages = new StageTrace((span) => sent.push(span), () => clock);
  const at = (ms) => { clock = ms; return stages; };
  const view = () => sent.map((span) => [span.name, Number(BigInt(span.startTimeUnixNano) / 1000000n), Number(BigInt(span.endTimeUnixNano) / 1000000n)]);
  return { stages, at, sent, view };
}

test('a delegated turn becomes one trace with a span per stage, and its traceparent names the delegate span', () => {
  const { at, sent, view } = recorder();
  at(1000).inputTranscript();
  at(1800).inputTranscript();
  const traceparent = at(1900).delegationCreated('d1');
  at(5200).hubReplied('d1');
  assert.ok(!sent.some((span) => span.name === 'turn'));
  at(5300).outputTranscript(40);
  at(5400).outputTranscript();
  assert.deepEqual(view(), [
    ['hear', 1000, 1800],
    ['decide', 1800, 1900],
    ['delegate', 1900, 5200],
    ['await-speech', 5200, 5340],
    ['turn', 1000, 5340],
  ]);
  const root = sent.at(-1);
  assert.equal(root.parentSpanId, undefined);
  assert.ok(sent.every((span) => span.traceId === root.traceId && /^[0-9a-f]{32}$/.test(span.traceId) && /^[0-9a-f]{16}$/.test(span.spanId)));
  assert.ok(sent.filter((span) => span.name !== 'turn').every((span) => span.parentSpanId === root.spanId && span.status === undefined));
  assert.equal(traceparent, `00-${root.traceId}-${sent.find((span) => span.name === 'delegate').spanId}-01`);
});

test('a failed hub request closes its turn with an error status and awaits no speech', () => {
  const { at, sent, view } = recorder();
  at(1000).inputTranscript();
  at(1100).delegationCreated('d1');
  at(2400).outputTranscript();
  at(20000).hubReplied('d1', 'failed');
  at(21500).outputTranscript();
  assert.deepEqual(view(), [['hear', 1000, 1000], ['decide', 1000, 1100], ['delegate', 1100, 20000], ['turn', 1000, 20000]]);
  assert.deepEqual(sent.find((span) => span.name === 'delegate').status, { code: 2, message: 'failed' });
});

test('a reply to an unknown delegation traces nothing', () => {
  const { stages, sent } = recorder();
  stages.hubReplied('missing');
  assert.deepEqual(sent, []);
});

test('a model reply, a pause, or the listener speaking again each close the turn before it', () => {
  const { at, view } = recorder();
  at(500).inputTranscript();
  at(1000).outputTranscript();
  at(1500).inputTranscript();
  at(1550).outputTranscript();
  at(1600).delegationCreated('d0');
  assert.deepEqual(view(), [['hear', 1500, 1500], ['decide', 1500, 1600]]);
  at(1700).hubReplied('d0');
  at(1800).outputTranscript(Number.NaN);
  assert.deepEqual(view().slice(2), [['delegate', 1600, 1700], ['await-speech', 1700, 1800], ['turn', 1500, 1800]]);
  at(3000).inputTranscript();
  at(9000).inputTranscript();
  at(9500).delegationCreated('d1');
  at(10000).hubReplied('d1');
  at(12000).inputTranscript();
  at(12500).outputTranscript();
  assert.deepEqual(view().slice(5), [['hear', 9000, 9000], ['decide', 9000, 9500], ['delegate', 9500, 10000], ['turn', 9000, 10000]]);
});

test('ending the session flushes open turns and cancels their delegations', () => {
  const { stages, at, sent, view } = recorder();
  at(1000).delegationCreated('d1');
  at(9000).sessionEnded();
  assert.deepEqual(view(), [['delegate', 1000, 9000], ['turn', 1000, 9000]]);
  assert.deepEqual(sent[0].status, { code: 2, message: 'cancelled' });
  stages.hubReplied('d1');
  at(9600).sessionEnded();
  assert.equal(sent.length, 2);
});
