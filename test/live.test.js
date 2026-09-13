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

test('delegated turn preserves heard text, hub request, reply, timing, and speech', () => {
  const trace = new ConversationTrace();
  trace.heard('what is ', 100);
  trace.heard('running?', 200);
  const entry = trace.delegated('deleg_1', 500);
  assert.equal(entry.heard, 'what is running?');
  assert.equal(entry.sent, 'what is running?');
  assert.equal(entry.duration_ms, 400);
  assert.equal(trace.hub('deleg_1', 'job 3728 is running', 870), true);
  trace.spoke('Job 3728 ');
  trace.spoke('is running.');
  assert.deepEqual(trace.entries[0], { id: 'deleg_1', heard: 'what is running?', decision: 'delegated', sent: 'what is running?', reply: 'job 3728 is running', timing: 870, said: 'Job 3728 is running.', duration_ms: 400 });
});

test('direct response is visibly tagged model alone', () => {
  const trace = new ConversationTrace();
  trace.heard('hello');
  trace.spoke('Hi.');
  assert.equal(trace.entries[0].heard, 'hello');
  assert.equal(trace.entries[0].decision, 'model alone');
  assert.equal(trace.entries[0].said, 'Hi.');
});
