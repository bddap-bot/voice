import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createPtt, wavSink, trackSink, createNudge, describeTrack, encodeWav } from '../docs/ptt.js';

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
    sink: () => sink,
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
  assert.ok(r.log.states.includes('nothing captured'));
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
  const ptt = createPtt({ mediaDevices: { getUserMedia: async () => fakeMic().stream }, sink: () => sink, onState: (s) => states.push(s) });
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

test('live: track reports contain the capture state Firefox exposes', () => {
  assert.equal(describeTrack({ kind: 'audio', readyState: 'live', enabled: true, muted: false }), 'audio:live enabled=true muted=false');
  assert.equal(describeTrack(null), 'none');
});

function liveRig({ refuse = false, gated = false, slowSender = false, meter = null, refuseSender = false } = {}) {
  const mics = [];
  const sent = [];
  const installed = [];
  const states = [];
  const silence = { silence: true };
  let gate = null;
  let senderGate = null;
  const mediaDevices = {
    getUserMedia: async () => {
      if (refuse) throw new Error('denied');
      if (gated) await new Promise((r) => { gate = r; });
      const m = fakeMic(); mics.push(m); return m.stream;
    },
  };
  const sender = {
    track: silence,
    replaceTrack: async (t) => {
      if (refuseSender) throw new Error('incompatible track');
      if (slowSender && t !== silence) await new Promise((r) => { senderGate = r; });
      sender.track = t; installed.push(t);
    },
  };
  const sink = trackSink({ sender, silence, send: async (f) => sent.push(f), onState: (s) => states.push(s), makeMeter: meter ?? (() => null) });
  const live = createPtt({ mediaDevices, sink: () => sink, onState: (s) => states.push(s) });
  return { live, mics, sent, installed, states, sender, silence, openGate: () => gate && gate(), openSender: () => senderGate && senderGate() };
}

test('live: mic track only installed while held; release stops it and restores silence + sends release', async () => {
  const h = liveRig();
  await h.live.hold();
  assert.equal(h.sender.track, h.mics[0].track);
  assert.deepEqual(h.sent, ['hold']);
  assert.ok(h.states.includes('talking'));
  await h.live.release();
  assert.equal(h.mics[0].track.stopped, true, 'mic track ended on release');
  assert.equal(h.sender.track, h.silence, 'nothing but silence leaves the page outside a hold');
  assert.deepEqual(h.sent, ['hold', 'release']);
  assert.deepEqual(h.installed, [h.mics[0].track, h.silence]);
  assert.equal(h.states.at(-1), 'listening');
});

test('live: release reports the measured input peak', async () => {
  const h = liveRig({ meter: () => ({ read: () => 0.125, close() {} }) });
  await h.live.hold();
  await h.live.release();
  assert.ok(h.states.includes('input peak 0.1250'));
  assert.ok(h.states.some((s) => s.startsWith('mic granted: audio:')));
  assert.ok(h.states.some((s) => s.startsWith('sender hold: audio:')));
});

test('live: failed diagnostics do not break capture cleanup or release', async () => {
  const h = liveRig({ meter: () => { throw new Error('analyser unavailable'); } });
  await h.live.hold();
  await h.live.release();
  assert.equal(h.mics[0].track.stopped, true);
  assert.deepEqual(h.sent, ['hold', 'release']);
  assert.ok(h.states.includes('input meter failed: analyser unavailable'));
});

test('live: sender refusal stops the mic and sends no hold', async () => {
  const h = liveRig({ refuseSender: true });
  await h.live.hold();
  assert.equal(h.mics[0].track.stopped, true);
  assert.equal(h.live.held, false);
  assert.deepEqual(h.sent, []);
  assert.ok(h.states.includes('hold failed: incompatible track'));
});

test('live: release racing getUserMedia stops the track and never installs it', async () => {
  const h = liveRig({ gated: true });
  const holding = h.live.hold();
  await tick();
  const releasing = h.live.release();
  h.openGate();
  await holding;
  await releasing;
  assert.equal(h.mics[0].track.stopped, true);
  assert.equal(h.sender.track, h.silence);
  assert.deepEqual(h.sent, [], 'no hold frame for a hold that never went live');
});

test('live: release racing replaceTrack still sends hold then release, in that order', async () => {
  const h = liveRig({ slowSender: true });
  const holding = h.live.hold();
  await tick();
  const releasing = h.live.release();
  await tick();
  assert.deepEqual(h.sent, [], 'nothing sent while the sender is still installing');
  h.openSender();
  await holding;
  await releasing;
  assert.deepEqual(h.sent, ['hold', 'release']);
  assert.equal(h.sender.track, h.silence);
  assert.equal(h.mics[0].track.stopped, true);
  assert.equal(h.live.held, false);
});

test('live: mic refusal leaves it unheld and sends nothing', async () => {
  const h = liveRig({ refuse: true });
  await h.live.hold();
  assert.equal(h.live.held, false);
  assert.deepEqual(h.sent, []);
  assert.equal(h.sender.track, h.silence);
});

test('live: a second hold while held is a no-op (one mic per hold)', async () => {
  const h = liveRig();
  await h.live.hold();
  await h.live.hold();
  assert.equal(h.mics.length, 1);
  await h.live.release();
  await h.live.release();
  assert.deepEqual(h.sent, ['hold', 'release']);
});

test('the sink chosen at hold time is the one closed at release, even if the mode switched meanwhile', async () => {
  const closed = [];
  const mk = (name) => ({ open: async () => name, close: async () => { closed.push(name); } });
  let current = mk('a');
  const ptt = createPtt({ mediaDevices: { getUserMedia: async () => fakeMic().stream }, sink: () => current });
  await ptt.hold();
  current = mk('b');
  await ptt.release();
  assert.deepEqual(closed, ['a']);
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
