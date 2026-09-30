import test from 'node:test';
import assert from 'node:assert/strict';
import { AudioDiagnostics, AUDIO_DIAGNOSTICS_KEY } from '../docs/audio-diagnostics.js';

function memory() {
  const values = new Map();
  return { getItem: key => values.get(key), setItem: (key, value) => values.set(key, value) };
}

test('diagnostics retain bounded lifecycle evidence without speech, tokens, or addresses', () => {
  const storage = memory();
  const recorder = new AudioDiagnostics(storage, () => 12);
  for (let i = 0; i < 305; i++) recorder.record('peer', { state: 'connected', token: 'secret', address: 'private', text: 'speech' });
  const rows = JSON.parse(storage.getItem(AUDIO_DIAGNOSTICS_KEY));
  assert.equal(rows.length, 300);
  assert.deepEqual(rows[0], { at: 12, kind: 'peer', state: 'connected' });
  assert.equal(new AudioDiagnostics(storage).rows.length, 300);
});

test('track mute and end are distinguished from AudioContext suspension and transport activity', async () => {
  const recorder = new AudioDiagnostics(memory());
  const track = Object.assign(new EventTarget(), { readyState: 'live', enabled: true, muted: false });
  recorder.track(track);
  track.muted = true;
  track.dispatchEvent(new Event('mute'));
  track.readyState = 'ended';
  track.dispatchEvent(new Event('ended'));
  const context = Object.assign(new EventTarget(), { state: 'running', currentTime: 2 });
  recorder.context(context, 'playback-context');
  context.state = 'suspended';
  context.dispatchEvent(new Event('statechange'));
  await recorder.stats({ getStats: async () => new Map([
    ['audio', { type: 'outbound-rtp', kind: 'audio', packetsSent: 50, bytesSent: 800, address: 'private' }],
    ['video', { type: 'outbound-rtp', kind: 'video', packetsSent: 90 }],
    ['source', { type: 'media-source', kind: 'audio', totalAudioEnergy: 2, totalSamplesDuration: 10 }],
  ]) });
  assert.deepEqual(recorder.rows.map(row => row.state ?? row.kind), ['live', 'live', 'ended', 'running', 'suspended', 'outbound-rtp', 'media-source']);
  assert.equal(recorder.rows[1].muted, true);
  assert.equal(recorder.rows[5].packetsSent, 50);
  assert.equal(recorder.rows[6].totalSamplesDuration, 10);
  assert.ok(!JSON.stringify(recorder.rows).includes('private'));
});

test('unavailable diagnostic storage and failed statistics never interrupt a call', async () => {
  const recorder = new AudioDiagnostics({ getItem() { throw Error(); }, setItem() { throw Error(); } });
  recorder.record('freeze');
  await recorder.stats({ getStats: async () => { throw Error(); } });
  assert.equal(recorder.rows.length, 1);
});
