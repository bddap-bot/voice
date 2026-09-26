import assert from 'node:assert/strict';
import test from 'node:test';
import { PlaybackBuffer } from '../docs/playback-buffer.js';
import { LivePlayback } from '../docs/live-playback.js';

test('audio plays in original order after its delay', () => {
  const buffer = new PlaybackBuffer(12, 2);
  const output = new Float32Array(4);
  buffer.process([1, 2, 3, 4], output);
  assert.deepEqual([...output], [1, 2, 0, 0]);
  buffer.process([5, 6, 7, 8], output);
  assert.deepEqual([...output], [3, 4, 5, 6]);
});

test('interruption discards old audio and overflow fails closed', () => {
  const buffer = new PlaybackBuffer(8, 2);
  const output = new Float32Array(4);
  buffer.process([1, 2, 3], output);
  assert.deepEqual([...output], [1, 0, 0, 0]);
  buffer.clear();
  buffer.process([7, 8, 9], output);
  assert.deepEqual([...output], [7, 0, 0, 0]);
  assert.throws(() => new PlaybackBuffer(4).process([1, 2, 3, 4, 5], output), /overflow/);
});

test('silence queued beyond the delay is skipped so speech keeps its latency', () => {
  const buffer = new PlaybackBuffer(24, 2);
  const output = new Float32Array(4);
  buffer.process(new Array(16).fill(0), output);
  assert.ok(buffer.size <= 6);
  buffer.process([1, 2, 3, 4], output);
  const spoken = [...output];
  buffer.process([0, 0, 0, 0], output);
  spoken.push(...output);
  assert.deepEqual(spoken.filter((value) => value !== 0), [1, 2, 3, 4]);
});

test('quiet waits for queued speech to play out and for a run of silence', async () => {
  const buffer = new PlaybackBuffer(64, 4);
  const output = new Float32Array(4);
  buffer.process([0.5, 0.5, 0.5, 0.5], output);
  assert.equal(buffer.quiet(4), false);
  buffer.process([0, 0, 0, 0], output);
  assert.deepEqual([...output], [0.5, 0.5, 0.5, 0.5]);
  assert.equal(buffer.quiet(4), true);
  assert.equal(buffer.quiet(8), false);
  buffer.process([0, 0, 0, 0], output);
  assert.equal(buffer.quiet(8), true);
  buffer.process([0, 0.5, 0, 0], output);
  assert.equal(buffer.quiet(4), false);
  await new LivePlayback(assert.fail).quiet();
});

test('closing playback releases quiet waiters so delayed replies can be discarded', async () => {
  const playback = new LivePlayback(assert.fail);
  playback.node = { port: { postMessage() {} }, disconnect() {} };
  const quiet = playback.quiet();
  await playback.close();
  await quiet;
  assert.equal(playback.quietWaiters.length, 0);
});
