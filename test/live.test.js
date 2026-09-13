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

test('direct response remains visible and later delegation cannot lose pending speech', () => {
  const trace = new ConversationTrace();
  trace.heard('first question');
  trace.spoke('Direct answer.');
  trace.heard('yes, do that');
  const entry = trace.delegated('item_2');
  assert.equal(entry.sent, 'first question\nyes, do that');
  assert.deepEqual(entry.context, [{ speaker: 'live', text: 'Direct answer.' }]);
  assert.equal(trace.entries[1].kind, 'spoken');
  assert.equal(trace.entries[1].source, 'model alone');
  assert.equal(trace.entries[1].text, 'Direct answer.');
});

test('delegation context drops oldest whole turns to fit the broker limit', () => {
  const trace = new ConversationTrace();
  for (let index = 0; index < 10; index++) {
    trace.heard(`question ${index} ${'q'.repeat(450)}`);
    trace.spoke(`answer ${index} ${'a'.repeat(450)}`);
  }
  trace.heard('check again');
  const entry = trace.delegated('item_context');
  assert.ok(new TextEncoder().encode(JSON.stringify(entry.context)).length <= 8192);
  assert.ok(entry.context.length < 20);
});

test('delegation request drops oldest complete fragments to fit the broker limit', () => {
  const trace = new ConversationTrace();
  for (let index = 0; index < 20; index++) {
    trace.heard(`question ${index} ${'q'.repeat(450)}`);
    trace.spoke('Still listening.');
  }
  const entry = trace.delegated('item_request');
  assert.ok(new TextEncoder().encode(entry.sent).length <= 8192);
  assert.match(entry.sent, /question 19/);
  assert.doesNotMatch(entry.sent, /question 0 /);
});

test('hub speech keeps its attribution when input interleaves', () => {
  const trace = new ConversationTrace();
  trace.heard('status');
  trace.delegated('item_overlap');
  trace.hub('item_overlap', 'running', 10);
  trace.spoke('It is ', 100, 200);
  trace.heard('sorry', 0, 150);
  trace.spoke('running.', 200, 300);
  assert.deepEqual(trace.entries.filter((entry) => entry.kind === 'spoken').map((entry) => entry.source), ['model via hub', 'model via hub']);
});

test('speech after a later owner turn is tagged model alone', () => {
  const trace = new ConversationTrace();
  trace.heard('status');
  trace.delegated('item_later');
  trace.hub('item_later', 'running', 10);
  trace.spoke('Running.', 100, 200);
  trace.heard('thanks', 0, 300);
  trace.spoke('You are welcome.', 320, 400);
  assert.equal(trace.entries.at(-1).source, 'model alone');
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
