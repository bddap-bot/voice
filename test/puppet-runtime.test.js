import assert from 'node:assert/strict';
import test from 'node:test';
import { audioVisemes } from '../src/puppet.js';

function waveform(amplitude) {
  return Uint8Array.from({ length: 256 }, (_, index) => 128 + Math.round(Math.sin(index / 3) * amplitude));
}

test('silence closes every viseme', () => {
  const values = audioVisemes(waveform(0), new Uint8Array(128).fill(255), 48000, 256);
  assert.deepEqual(values, { aa: 0, ih: 0, ou: 0, ee: 0, oh: 0 });
});

test('spectral bands choose different VRM visemes', () => {
  const low = new Uint8Array(128);
  low[2] = 255;
  const high = new Uint8Array(128);
  high[15] = 255;
  const lowValues = audioVisemes(waveform(32), low, 48000, 256);
  const highValues = audioVisemes(waveform(32), high, 48000, 256);
  assert.equal(Object.entries(lowValues).sort((left, right) => right[1] - left[1])[0][0], 'ou');
  assert.equal(Object.entries(highValues).sort((left, right) => right[1] - left[1])[0][0], 'ih');
  assert.ok(lowValues.ou > 0.7);
  assert.ok(highValues.ih > 0.7);
});
