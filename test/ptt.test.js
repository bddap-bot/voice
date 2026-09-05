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

import { createLivePtt, createNudge, describeTrack } from '../docs/ptt.js';

test('live: track reports contain the capture state Firefox exposes', () => {
  assert.equal(describeTrack({ kind: 'audio', readyState: 'live', enabled: true, muted: false }), 'audio:live enabled=true muted=false');
  assert.equal(describeTrack(null), 'none');
});

function fakeMic() {
  const track = { kind: 'audio', readyState: 'live', enabled: true, muted: false, stopped: false, stop() { this.stopped = true; this.readyState = 'ended'; } };
  return { track, stream: { getAudioTracks: () => [track] } };
}
function liveHarness(opts = {}) {
  const mics = [];
  const sent = [];
  const installed = [];
  const silence = { silence: true };
  let gate = null;
  const mediaDevices = {
    getUserMedia: async () => {
      if (opts.refuse) throw new Error('denied');
      if (opts.gated) await new Promise((r) => { gate = r; });
      const m = fakeMic(); mics.push(m); return m.stream;
    },
  };
  const sender = { track: silence, replaceTrack: async (t) => { sender.track = t; installed.push(t); } };
  const live = createLivePtt({ mediaDevices, sender, silence, send: async (f) => sent.push(f) });
  return { live, mics, sent, installed, sender, silence, openGate: () => gate && gate() };
}

test('live: mic track only installed while held; release stops it and restores silence + sends release', async () => {
  const h = liveHarness();
  await h.live.hold();
  assert.equal(h.sender.track, h.mics[0].track);
  assert.deepEqual(h.sent, ['hold']);
  await h.live.release();
  assert.equal(h.mics[0].track.stopped, true, 'mic track ended on release');
  assert.equal(h.sender.track, h.silence, 'nothing but silence leaves the page outside a hold');
  assert.deepEqual(h.sent, ['hold', 'release']);
  assert.deepEqual(h.installed, [h.mics[0].track, h.silence]);
});

test('live: release reports the measured input peak', async () => {
  const h = liveHarness();
  const states = [];
  const live = createLivePtt({
    mediaDevices: { getUserMedia: async () => { const m = fakeMic(); h.mics.push(m); return m.stream; } },
    sender: h.sender, silence: h.silence, send: async () => {}, onState: (s) => states.push(s),
    makeMeter: () => ({ read: () => 0.125, close() {} }),
  });
  await live.hold();
  await live.release();
  assert.ok(states.includes('input peak 0.1250'));
  assert.ok(states.some((s) => s.startsWith('mic granted: audio:')));
  assert.ok(states.some((s) => s.startsWith('sender hold: audio:')));
});

test('live: failed diagnostics do not break capture cleanup or release', async () => {
  const h = liveHarness();
  const states = [];
  const live = createLivePtt({
    mediaDevices: { getUserMedia: async () => { const m = fakeMic(); h.mics.push(m); return m.stream; } },
    sender: h.sender, silence: h.silence, send: async (f) => h.sent.push(f), onState: (s) => states.push(s),
    makeMeter: () => { throw new Error('analyser unavailable'); },
  });
  await live.hold();
  await live.release();
  assert.equal(h.mics[0].track.stopped, true);
  assert.deepEqual(h.sent, ['hold', 'release']);
  assert.ok(states.includes('input meter failed: analyser unavailable'));
});

test('live: sender refusal stops the mic and sends no hold', async () => {
  const mic = fakeMic();
  const states = [];
  const live = createLivePtt({
    mediaDevices: { getUserMedia: async () => mic.stream },
    sender: { replaceTrack: async () => { throw new Error('incompatible track'); } }, silence: {},
    send: async () => assert.fail('hold must not be sent'), onState: (s) => states.push(s),
  });
  await live.hold();
  assert.equal(mic.track.stopped, true);
  assert.equal(live.held, false);
  assert.ok(states.includes('sender hold failed: incompatible track'));
});

test('live: release racing getUserMedia stops the track and never installs it', async () => {
  const h = liveHarness({ gated: true });
  const holding = h.live.hold();
  await new Promise((r) => setTimeout(r, 10));
  await h.live.release();
  h.openGate();
  await holding;
  assert.equal(h.mics[0].track.stopped, true);
  assert.equal(h.sender.track, h.silence);
  assert.deepEqual(h.sent, [], 'no hold frame for a hold that never went live');
});

test('live: mic refusal leaves it unheld and sends nothing', async () => {
  const h = liveHarness({ refuse: true });
  await h.live.hold();
  assert.equal(h.live.held, false);
  assert.deepEqual(h.sent, []);
  assert.equal(h.sender.track, h.silence);
});

test('live: a second hold while held is a no-op (one mic per hold)', async () => {
  const h = liveHarness();
  await h.live.hold();
  await h.live.hold();
  assert.equal(h.mics.length, 1);
  await h.live.release();
  await h.live.release();
  assert.deepEqual(h.sent, ['hold', 'release']);
});

function nudgeRig() {
  const log = { nudges: 0, clears: 0 };
  const timers = []; const cleared = [];
  const nudge = createNudge({
    onNudge: () => { log.nudges++; },
    onClear: () => { log.clears++; },
    setTimer: (fn, ms) => { timers.push({ fn, ms }); return timers.length; },
    clearTimer: (id) => { cleared.push(id); timers[id - 1].fn = null; },
  });
  return { nudge, log, timers, cleared, fire: () => { for (const tm of timers) { const fn = tm.fn; tm.fn = null; if (fn) fn(); } } };
}

test('nudge: media connected with no hold fires once after the window', () => {
  const r = nudgeRig();
  r.nudge.connected();
  assert.equal(r.timers.length, 1);
  assert.equal(r.timers[0].ms, 8000);
  assert.equal(r.log.nudges, 0);
  r.fire();
  assert.equal(r.log.nudges, 1);
});

test('nudge: a hold inside the window clears the timer; the recorded callback is inert afterwards', () => {
  const r = nudgeRig();
  r.nudge.connected();
  const { fn } = r.timers[0];
  r.nudge.hold();
  assert.deepEqual(r.cleared, [1]);
  assert.equal(r.log.clears, 1);
  fn();
  assert.equal(r.log.nudges, 0);
});

test('nudge: stop clears a pending timer and calls onClear', () => {
  const r = nudgeRig();
  r.nudge.connected();
  r.nudge.stop();
  assert.deepEqual(r.cleared, [1]);
  assert.equal(r.log.clears, 1);
  r.fire();
  assert.equal(r.log.nudges, 0);
});

test('nudge: a second connected() while pending arms no second timer', () => {
  const r = nudgeRig();
  r.nudge.connected();
  r.nudge.connected();
  assert.equal(r.timers.length, 1);
});

test('nudge: after a hold, connected() arms nothing', () => {
  const r = nudgeRig();
  r.nudge.hold();
  r.nudge.connected();
  assert.equal(r.timers.length, 0);
  assert.equal(r.log.nudges, 0);
});

test('nudge: once fired, a reconnect arms nothing again', () => {
  const r = nudgeRig();
  r.nudge.connected();
  r.fire();
  r.nudge.connected();
  assert.equal(r.timers.length, 1);
  assert.equal(r.log.nudges, 1);
});

test('nudge: a fired timer is forgotten; stop does not clear it again', () => {
  const r = nudgeRig();
  r.nudge.connected();
  r.fire();
  r.nudge.stop();
  assert.deepEqual(r.cleared, []);
  assert.equal(r.log.clears, 1);
});
