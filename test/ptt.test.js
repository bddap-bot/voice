import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createPtt, wavSink, labeller, encodeWav } from '../docs/ptt.js';

const tick = (ms = 10) => new Promise((r) => setTimeout(r, ms));

function fakeMic() {
  const track = { kind: 'audio', readyState: 'live', enabled: true, muted: false, stopped: false, stop() { this.stopped = true; this.readyState = 'ended'; } };
  return { track, stream: { getTracks: () => [track], getAudioTracks: () => [track] } };
}

function wavRig({ chunk = new Float32Array([0.1, -0.1, 0.5]), gated = false, refuse = false } = {}) {
  const log = { mics: [], uploads: [], states: [], closed: 0 };
  let emit = null;
  let gate = null;
  const sink = wavSink({
    capture: () => ({ rate: 16000, onChunk: (cb) => { emit = cb; }, close: () => { log.closed++; } }),
    upload: async (wav) => { log.uploads.push(wav); },
    onState: (s) => log.states.push(s),
  });
  const ptt = createPtt({
    mediaDevices: {
      getUserMedia: async () => {
        if (refuse) throw new Error('denied');
        if (gated) await new Promise((r) => { gate = r; });
        const m = fakeMic(); log.mics.push(m); return m.stream;
      },
    },
    sink,
    onState: (s) => log.states.push(s),
  });
  return { ptt, log, emit: (c = chunk) => emit && emit(c), openGate: () => gate && gate() };
}

test('hold → release: exactly one upload of what was captured, mic stopped, capture closed', async () => {
  const r = wavRig();
  await r.ptt.hold();
  r.emit(); r.emit();
  await r.ptt.release();
  assert.equal(r.log.uploads.length, 1);
  assert.equal(r.log.mics[0].track.stopped, true, 'track.stop() called on release');
  assert.equal(r.log.closed, 1);
  const wav = r.log.uploads[0];
  assert.equal(String.fromCharCode(...wav.subarray(0, 4)), 'RIFF');
  assert.equal(wav.length, 44 + 6 * 2, 'six samples of 16-bit PCM');
  assert.deepEqual(r.log.states, ['opening mic', 'listening', 'sending'], 'sent is the server\'s word, not the page\'s');
});

test('zero bytes leave outside a hold: chunks after release are dropped, no second upload', async () => {
  const r = wavRig();
  await r.ptt.hold();
  r.emit();
  await r.ptt.release();
  r.emit(); r.emit();
  assert.equal(r.log.uploads.length, 1);
  await r.ptt.hold();
  await r.ptt.release();
  assert.equal(r.log.uploads.length, 1, 'an empty hold uploads nothing');
  assert.ok(r.log.states.includes('nothing heard'));
  assert.equal(r.log.mics[1].track.stopped, true, 'the mic is still stopped after an empty hold');
});

test('release before the mic opened stops the track and uploads nothing', async () => {
  const r = wavRig({ gated: true });
  const holding = r.ptt.hold();
  await tick();
  const releasing = r.ptt.release();
  assert.equal(r.ptt.held, false, 'released at once, even while the mic is still opening');
  r.openGate();
  await holding;
  await releasing;
  assert.equal(r.log.mics[0].track.stopped, true);
  assert.equal(r.log.uploads.length, 0);
  assert.equal(r.log.closed, 0, 'the sink was never opened');
  assert.equal(r.ptt.held, false);
});

test('a refused mic leaves the button unheld with no upload', async () => {
  const r = wavRig({ refuse: true });
  await r.ptt.hold();
  assert.equal(r.ptt.held, false);
  assert.equal(r.log.uploads.length, 0);
  assert.ok(r.log.states.some((s) => s.startsWith('mic refused: denied')));
});

test('a failed upload is reported and leaves the button unheld', async () => {
  const states = [];
  const sink = wavSink({ capture: () => ({ rate: 16000, onChunk: (cb) => cb(new Float32Array([0.2])), close() {} }), upload: async () => { throw new Error('stream gone'); }, onState: (s) => states.push(s) });
  const ptt = createPtt({ mediaDevices: { getUserMedia: async () => fakeMic().stream }, sink, onState: (s) => states.push(s) });
  await ptt.hold();
  await ptt.release();
  assert.ok(states.includes('send failed: stream gone'));
  assert.equal(ptt.held, false);
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

test('a second hold while the first is still opening waits for it; the first mic is stopped, not leaked', async () => {
  const r = wavRig({ gated: true });
  const first = r.ptt.hold();
  await tick();
  const releasing = r.ptt.release();
  const second = r.ptt.hold();
  await tick();
  assert.equal(r.log.mics.length, 0, 'the second hold has not asked for a mic yet');
  r.openGate();
  await first; await releasing;
  await tick();
  assert.equal(r.log.mics.length, 1);
  r.openGate();
  await second;
  assert.equal(r.log.mics.length, 2);
  assert.equal(r.log.mics[0].track.stopped, true, 'the first mic was stopped');
  assert.equal(r.log.mics[1].track.stopped, false, 'the second hold is live');
  assert.equal(r.ptt.held, true);
  await r.ptt.release();
  assert.equal(r.log.mics[1].track.stopped, true);
});

function labelRig() {
  const shown = [];
  const label = labeller((t) => shown.push(t));
  return { label, shown, last: () => shown.at(-1) };
}

test('label: idle → listening… → sending… → thinking… → hold to talk, one label per step', () => {
  const r = labelRig();
  assert.equal(r.last(), 'hold to talk');
  r.label.listening();
  r.label.sending();
  r.label.sent();
  r.label.reply();
  assert.deepEqual(r.shown, ['hold to talk', 'listening…', 'sending…', 'thinking…', 'hold to talk']);
});

test('label: a reply while held keeps listening…; a hold during thinking shows listening…, and thinking resumes after', () => {
  const r = labelRig();
  r.label.listening(); r.label.sending(); r.label.sent();
  assert.equal(r.last(), 'thinking…');
  r.label.listening();
  assert.equal(r.last(), 'listening…');
  r.label.reply();
  assert.equal(r.last(), 'listening…');
  r.label.sending();
  assert.equal(r.last(), 'sending…');
  r.label.sent();
  assert.equal(r.last(), 'thinking…');
});

test('label: idle clears a busy word but not an unanswered note', () => {
  const r = labelRig();
  r.label.listening(); r.label.sending();
  r.label.idle();
  assert.equal(r.last(), 'hold to talk');
  r.label.listening(); r.label.sending(); r.label.sent();
  r.label.idle();
  assert.equal(r.last(), 'thinking…', 'the reply is re-served after a reconnect, so thinking stands');
});
