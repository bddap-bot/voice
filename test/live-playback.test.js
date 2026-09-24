import assert from 'node:assert/strict';
import test from 'node:test';
import { PlaybackBuffer } from '../docs/playback-buffer.js';
import { LivePlayback } from '../docs/live-playback.js';

test('held audio is silent and releases every sample in original order', () => {
  const buffer = new PlaybackBuffer(12, 2);
  const output = new Float32Array(4);
  buffer.held = true;
  buffer.process([1, 2, 3, 4], output);
  assert.deepEqual([...output], [0, 0, 0, 0]);
  buffer.process([5, 6, 7, 8], output);
  assert.deepEqual([...output], [0, 0, 0, 0]);
  buffer.held = false;
  buffer.process([], output);
  assert.deepEqual([...output], [1, 2, 3, 4]);
  buffer.process([9, 10, 11, 12], output);
  assert.deepEqual([...output], [5, 6, 7, 8]);
  buffer.process([13, 14], output);
  assert.deepEqual([...output], [9, 10, 11, 12]);
});

test('interruption discards old audio and overflow fails closed', () => {
  const buffer = new PlaybackBuffer(4);
  const output = new Float32Array(4);
  buffer.held = true;
  buffer.process([1, 2, 3, 4], output);
  assert.throws(() => buffer.process([5], output), /overflow/);
  assert.deepEqual([...output], [0, 0, 0, 0]);
  buffer.clear();
  buffer.held = false;
  buffer.process([9, 10], output);
  assert.deepEqual([...output], [9, 10, 0, 0]);
});

test('interleaved delegations hold transcripts until all settle; shutdown discards pending speech', async () => {
  const presented = [];
  const playback = new LivePlayback((event) => presented.push(event), assert.fail);
  playback.hold('one'); playback.hold('two');
  playback.transcript('early speech');
  playback.release('one');
  assert.deepEqual(presented, []);
  playback.release('two');
  assert.deepEqual(presented, ['early speech']);
  playback.hold('three'); playback.transcript('interrupted'); playback.interrupt();
  playback.release('three');
  assert.deepEqual(presented, ['early speech']);
  playback.hold('four'); playback.transcript('cancelled'); await playback.close(); playback.release('four');
  assert.deepEqual(presented, ['early speech']);
});

test('long holds discard queued silence and catch up after buffered words', () => {
  const buffer = new PlaybackBuffer(24, 2);
  const output = new Float32Array(4);
  buffer.held = true;
  for (let i = 0; i < 100; i++) buffer.process([0, 0, 0, 0], output);
  assert.ok(buffer.size <= 6);
  buffer.process([1, 2, 3, 4], output);
  buffer.held = false;
  const spoken = [];
  for (let i = 0; i < 10; i++) {
    buffer.process([0, 0, 0, 0], output);
    spoken.push(...output.filter(value => value !== 0));
  }
  assert.deepEqual(spoken, [1, 2, 3, 4]);
  assert.ok(buffer.size <= 6);
});

test('quiet waits for release, for held speech to play out, and for a run of silence', async () => {
  const buffer = new PlaybackBuffer(64);
  const output = new Float32Array(4);
  const idle = new PlaybackBuffer(64);
  idle.held = true;
  idle.process([0, 0, 0, 0], output);
  idle.process([0, 0, 0, 0], output);
  assert.equal(idle.quiet(4), false);
  buffer.held = true;
  buffer.process([0.5, 0.5, 0.5, 0.5], output);
  buffer.process([0, 0, 0, 0], output);
  assert.equal(buffer.quiet(4), false);
  buffer.held = false;
  assert.equal(buffer.quiet(4), false);
  buffer.process([0, 0, 0, 0], output);
  assert.deepEqual([...output], [0.5, 0.5, 0.5, 0.5]);
  assert.equal(buffer.quiet(4), true);
  assert.equal(buffer.quiet(12), false);
  buffer.process([0, 0, 0, 0], output);
  assert.equal(buffer.quiet(12), true);
  buffer.process([0, 0.5, 0, 0], output);
  assert.equal(buffer.quiet(4), false);
  await new LivePlayback(assert.fail, assert.fail).quiet();
});
