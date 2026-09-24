import assert from 'node:assert/strict';
import test from 'node:test';
import { MISS_SCORE, SpeechSegmenter, WAKE_SCORE, wakeScore } from '../docs/wake.js';

const phrase = 'Hey Corvus, wake up.';
const rate = 16000;

function tone(seconds, amplitude = 0.2) {
  return Float32Array.from({ length: Math.round(seconds * rate) }, (_, index) => amplitude * Math.sin(index / 5));
}

function silence(seconds) {
  return new Float32Array(Math.round(seconds * rate));
}

function segments(...parts) {
  const segmenter = new SpeechSegmenter(rate);
  return parts.flatMap((part) => segmenter.push(part));
}

test('the phrase wakes even when the name is misheard or the phrase sits inside other speech', () => {
  for (const heard of [' Hey Corvus, wake up.', ' Hey, Coravus, wake up!', ' Hey Caravus, wake up!', 'Hey chorus, wake up', ' Okay so, hey Corvus, wake up please.']) {
    assert.ok(wakeScore(heard, phrase).score >= WAKE_SCORE, heard);
  }
  assert.deepEqual(wakeScore(' Okay so, hey Corvus, wake up please.', phrase), { score: 1, heard: 'hey corvus wake up' });
});

test('ordinary speech, the bare name and a bare wake up stay below a logged miss', () => {
  for (const heard of [' I think we should get pizza tonight.', ' Corvus is a genus of birds.', ' It is time to wake up, everyone.', ' Wake up.', ' Hey Corvus.', ' Thanks for watching!', ' [BLANK_AUDIO]', '']) {
    assert.ok(wakeScore(heard, phrase).score < MISS_SCORE, heard);
  }
});

test('a near miss scores between a logged miss and a wake and names the words that resembled the phrase', () => {
  for (const heard of [' Hey carameless, wake up!', ' Hey Google, wake up.']) {
    const { score } = wakeScore(heard, phrase);
    assert.ok(score >= MISS_SCORE && score < WAKE_SCORE, `${heard} ${score}`);
  }
  assert.equal(wakeScore(' So then, hey Google, wake up, he said.', phrase).heard, 'hey google wake up');
});

test('silence yields no speech segments', () => {
  assert.deepEqual(segments(silence(5)), []);
});

test('an utterance becomes one segment with its lead-in and trailing pause', () => {
  const found = segments(silence(1), tone(1.2), silence(2));
  assert.equal(found.length, 1);
  assert.ok(Math.abs(found[0].length / rate - 2.2) < 0.05, `${found[0].length / rate}`);
  assert.equal(found[0][0], 0);
  assert.ok(Math.max(...found[0].subarray(0.3 * rate, 1.5 * rate).map(Math.abs)) > 0.19);
});

test('a click too short to be speech yields nothing', () => {
  assert.deepEqual(segments(silence(1), tone(0.1), silence(2)), []);
});

test('long speech is cut into bounded segments that overlap across each cut', () => {
  const found = segments(silence(1), tone(20), silence(2));
  assert.ok(found.length >= 3);
  for (const segment of found) assert.ok(segment.length <= 8 * rate);
  for (let index = 1; index < found.length; index++) {
    assert.deepEqual(found[index].subarray(0, 2 * rate), found[index - 1].subarray(-2 * rate));
  }
});

test('steady background noise stops producing segments while louder speech still does', () => {
  const segmenter = new SpeechSegmenter(rate);
  assert.ok(segmenter.push(Float32Array.from([...silence(1), ...tone(40, 0.02)])).length > 0);
  assert.deepEqual(segmenter.push(tone(20, 0.02)), []);
  assert.equal(segmenter.push(Float32Array.from([...tone(1.2, 0.3), ...tone(2, 0.02)])).length, 1);
});
