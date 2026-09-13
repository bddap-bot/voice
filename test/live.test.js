import assert from 'node:assert/strict';
import test from 'node:test';
import { ConversationTrace, SessionClock, formatElapsed } from '../docs/live.js';

test('clock displays elapsed minutes and flips off at the configured cap', () => {
  let now = 1000;
  let tick;
  let label;
  let capped = 0;
  const clock = new SessionClock({ capSeconds: 30, now: () => now, every: (fn) => { tick = fn; return 42; }, cancel: () => {}, onTick: (seconds) => { label = formatElapsed(seconds); }, onCap: () => { capped++; } });
  clock.start();
  assert.equal(label, '00:00');
  now = 30999;
  tick();
  assert.equal(label, '00:29');
  assert.equal(capped, 0);
  now = 31000;
  tick();
  assert.equal(label, '00:30');
  assert.equal(capped, 1);
  tick();
  assert.equal(capped, 1);
});

test('timeline preserves every heard and spoken fragment around delegation', () => {
  const trace = new ConversationTrace();
  trace.heard('check the ', 100);
  trace.spoke('One moment.');
  trace.heard('deployment', 200);
  const delegation = trace.delegated('item_1', 500);
  trace.hub('item_1', 'running', 870);
  trace.spoke('It is running.');
  assert.equal(delegation.sent, 'check the\ndeployment');
  assert.equal(delegation.duration_ms, 400);
  assert.deepEqual(trace.entries, [
    { kind: 'heard', text: 'check the ' },
    { kind: 'spoken', source: 'model alone', text: 'One moment.' },
    { kind: 'heard', text: 'deployment' },
    { kind: 'delegation', id: 'item_1', sent: 'check the\ndeployment', context: [{ speaker: 'live', text: 'One moment.' }], reply: 'running', timing: 870, duration_ms: 400 },
    { kind: 'spoken', source: 'model via hub', text: 'It is running.' },
  ]);
});

test('direct response is visible without consuming later delegation context', () => {
  const trace = new ConversationTrace();
  trace.heard('first question');
  trace.spoke('Direct answer.');
  trace.completeSpeech();
  trace.heard('yes, do that');
  const entry = trace.delegated('item_2');
  assert.equal(entry.sent, 'yes, do that');
  assert.deepEqual(entry.context, [
    { speaker: 'owner', text: 'first question' },
    { speaker: 'live', text: 'Direct answer.' },
  ]);
  assert.equal(trace.entries[1].kind, 'spoken');
  assert.equal(trace.entries[1].source, 'model alone');
  assert.equal(trace.entries[1].text, 'Direct answer.');
});

test('hub results remain on their delegation when later speech occurs', () => {
  const trace = new ConversationTrace();
  trace.heard('deploy');
  trace.delegated('item_3');
  trace.heard('thanks');
  trace.spoke('Welcome.');
  trace.hub('item_3', 'deployed', 50);
  assert.equal(trace.entries[1].reply, 'deployed');
  assert.equal(trace.entries.at(-1).text, 'Welcome.');
});

test('failed and cancelled delegations remain visible', () => {
  const trace = new ConversationTrace();
  trace.heard('first');
  trace.delegated('item_4');
  trace.failed('item_4', 'hub reply timed out');
  trace.heard('second');
  trace.delegated('item_5');
  trace.cancel();
  assert.deepEqual(trace.entries.filter((entry) => entry.kind === 'delegation').map(({ reply, failed }) => ({ reply, failed })), [
    { reply: 'hub reply timed out', failed: true },
    { reply: 'cancelled', failed: true },
  ]);
});
