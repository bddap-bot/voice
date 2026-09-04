import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createPtt, encodeWav } from '../docs/ptt.js';

function rig({ chunk = new Float32Array([0.1, -0.1, 0.5]) } = {}) {
  const log = { stops: 0, uploads: [], states: [], closed: 0, getUserMedia: 0 };
  let emit = null;
  const track = { stop: () => { log.stops++; } };
  const stream = { getTracks: () => [track] };
  const ptt = createPtt({
    mediaDevices: { getUserMedia: async () => { log.getUserMedia++; return stream; } },
    capture: () => ({ rate: 16000, onChunk: (cb) => { emit = cb; }, close: () => { log.closed++; } }),
    upload: async (wav) => { log.uploads.push(wav); },
    onState: (s) => log.states.push(s),
  });
  return { ptt, log, chunk, emit: (c = chunk) => emit && emit(c) };
}

test('hold → release: exactly one upload of what was captured, mic stopped', async () => {
  const r = rig();
  await r.ptt.hold();
  r.emit(); r.emit();
  await r.ptt.release();
  assert.equal(r.log.uploads.length, 1);
  assert.equal(r.log.stops, 1, 'track.stop() called on release');
  assert.equal(r.log.closed, 1);
  const wav = r.log.uploads[0];
  assert.equal(String.fromCharCode(...wav.subarray(0, 4)), 'RIFF');
  assert.equal(wav.length, 44 + 6 * 2, 'six samples of 16-bit PCM');
});

test('zero bytes leave outside a hold: chunks after release are dropped, no second upload', async () => {
  const r = rig();
  await r.ptt.hold();
  r.emit();
  await r.ptt.release();
  r.emit(); r.emit();
  assert.equal(r.log.uploads.length, 1);
  await r.ptt.hold();
  await r.ptt.release();
  assert.equal(r.log.uploads.length, 1, 'an empty hold uploads nothing');
  assert.equal(r.log.stops, 2, 'the mic is still stopped after an empty hold');
});

test('release before the mic opened stops the track and uploads nothing', async () => {
  const r = rig();
  const holding = r.ptt.hold();
  await r.ptt.release();
  await holding;
  assert.equal(r.log.stops, 1);
  assert.equal(r.log.uploads.length, 0);
  assert.equal(r.ptt.held, false);
});

test('a refused mic leaves the button unheld with no upload', async () => {
  const log = [];
  const ptt = createPtt({
    mediaDevices: { getUserMedia: async () => { throw new Error('denied'); } },
    capture: () => { throw new Error('unreachable'); },
    upload: async () => { throw new Error('unreachable'); },
    onState: (s) => log.push(s),
  });
  await ptt.hold();
  assert.equal(ptt.held, false);
  assert.ok(log.some((s) => s.startsWith('mic refused')));
});

test('encodeWav resamples to 16 kHz mono 16-bit', () => {
  const wav = encodeWav(new Float32Array(48000).fill(0.25), 48000);
  const v = new DataView(wav.buffer);
  assert.equal(v.getUint32(24, true), 16000);
  assert.equal(v.getUint16(22, true), 1);
  assert.equal(v.getUint16(34, true), 16);
  assert.equal(v.getUint32(40, true), 16000 * 2);
  assert.equal(v.getInt16(44, true), Math.trunc(0.25 * 0x7fff));
});
