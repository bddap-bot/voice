import assert from 'node:assert/strict';
import test from 'node:test';
import { WhisperFeatureExtractor } from '@huggingface/transformers';
import { HandOffHold, RATE, TURN, WINDOW, features } from '../docs/turn.js';

test('features match transformers WhisperFeatureExtractor on the normalized, front-padded eight seconds', async () => {
  const extractor = new WhisperFeatureExtractor({ feature_size: 80, sampling_rate: RATE, hop_length: 160, chunk_length: 8, n_fft: 400, n_samples: WINDOW, nb_max_frames: 800, padding_value: 0, return_attention_mask: false });
  for (const seconds of [5, 9]) {
    const audio = Float32Array.from({ length: seconds * RATE }, (_, index) => 0.3 * Math.sin(index * 0.05) * Math.sin(index * 0.0007) + 0.05 * Math.sin(index * 12.9898) * Math.cos(index * 78.233));
    const padded = new Float64Array(WINDOW);
    const newest = audio.subarray(Math.max(0, audio.length - WINDOW));
    padded.set(newest, WINDOW - newest.length);
    const mean = padded.reduce((sum, value) => sum + value, 0) / WINDOW;
    const deviation = Math.sqrt(padded.reduce((sum, value) => sum + (value - mean) ** 2, 0) / WINDOW + 1e-7);
    const reference = (await extractor(Float32Array.from(padded, (value) => (value - mean) / deviation))).input_features.data;
    const ours = features(audio);
    assert.equal(ours.length, reference.length);
    let worst = 0;
    for (let index = 0; index < ours.length; index++) worst = Math.max(worst, Math.abs(ours[index] - reference[index]));
    assert.ok(worst < 1e-3, `${seconds} s: worst difference ${worst}`);
  }
});

function fixture() {
  const timers = new Set();
  const f = {
    now: 0,
    sent: [],
    asks: [],
    hold: null,
    wait(ms) {
      const end = f.now + ms;
      for (;;) {
        const next = [...timers].filter((timer) => timer.at <= end).sort((a, b) => a.at - b.at)[0];
        if (!next) break;
        timers.delete(next);
        f.now = next.at;
        next.run();
      }
      f.now = end;
    },
    judged(probability) { f.asks.pop()(probability); },
  };
  f.hold = new HandOffHold({
    handOff: (id) => { f.hold.clear(); f.sent.push(id); },
    judge: (verdict) => f.asks.push(verdict),
    now: () => f.now,
    later: (run, ms) => { const timer = { at: f.now + ms, run }; timers.add(timer); return timer; },
    cancel: (timer) => timers.delete(timer),
  });
  return f;
}

test('a finished thought goes once judged', () => {
  const f = fixture();
  f.hold.heard();
  f.hold.due('item_size', true);
  assert.deepEqual(f.sent, []);
  f.judged(0.9);
  assert.deepEqual(f.sent, ['item_size']);
});

test('an unfinished thought waits for more speech and goes under the newest delegation', () => {
  const f = fixture();
  f.hold.heard();
  f.hold.due('item_so', true);
  f.judged(0.1);
  f.wait(4000);
  f.hold.heard();
  f.hold.due('item_idea', true);
  f.wait(TURN.quietMs - 1);
  assert.equal(f.asks.length, 0, 'renewed speech is judged after a quiet');
  f.wait(1);
  f.judged(TURN.complete);
  assert.deepEqual(f.sent, ['item_idea']);
});

test('an unfinished thought goes after the hold of quiet', () => {
  const f = fixture();
  f.hold.heard();
  f.hold.due('item_figure', true);
  f.judged(0.2);
  f.wait(TURN.holdMs - 1);
  assert.deepEqual(f.sent, []);
  f.wait(1);
  assert.deepEqual(f.sent, ['item_figure']);
});

test('a judgement of speech that went on is dropped', () => {
  const f = fixture();
  f.hold.heard();
  f.hold.due('item_showing', true);
  const stale = f.asks.pop();
  f.hold.heard();
  stale(0.9);
  assert.deepEqual(f.sent, []);
  f.wait(TURN.quietMs);
  f.judged(0.9);
  assert.deepEqual(f.sent, ['item_showing']);
});

test('the page\'s own turn id never replaces a delegation id, and a failed judgement ends the thought', () => {
  const f = fixture();
  f.hold.due('item_a', true);
  f.hold.due('turn_b', false);
  f.judged(null);
  assert.deepEqual(f.sent, ['item_a']);
});

test('the hold counts from the last word heard before the hand-off came due', () => {
  const f = fixture();
  f.hold.heard();
  f.wait(3000);
  f.hold.due('turn_late', false);
  f.judged(0.1);
  f.wait(TURN.holdMs - 3000 - 1);
  assert.deepEqual(f.sent, []);
  f.wait(1);
  assert.deepEqual(f.sent, ['turn_late']);
});
