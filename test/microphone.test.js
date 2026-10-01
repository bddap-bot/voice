import assert from 'node:assert/strict';
import test from 'node:test';
import { Microphone } from '../docs/microphone.js';

function fixture() {
  const track = () => Object.assign(new EventTarget(), { readyState: 'live', stop() { this.readyState = 'ended'; } });
  const output = track();
  const requests = [], sources = [];
  globalThis.AudioContext = class {
    state = 'running';
    createMediaStreamDestination() { return { stream: { getTracks: () => [output] } }; }
    createMediaStreamSource(stream) {
      const source = { stream, connected: false, connect() { this.connected = true; }, disconnect() { this.connected = false; } };
      sources.push(source);
      return source;
    }
    async resume() {}
    async close() { this.state = 'closed'; }
  };
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { mediaDevices: {
    getUserMedia: () => new Promise((resolve, reject) => {
      const input = track();
      requests.push({ input, reject, resolve: () => resolve({ getTracks: () => [input], getAudioTracks: () => [input] }) });
    }),
  } } });
  const ended = [];
  const microphone = new Microphone({ track() {}, ended: stream => ended.push(stream) });
  return { microphone, requests, output, sources, ended };
}

test('mute stops capture while the same session stream survives repeated unmute', async () => {
  const { microphone, requests, output, sources } = fixture();
  let stream;
  for (let cycle = 0; cycle < 3; cycle++) {
    const opening = microphone.open(() => true);
    assert.equal(requests.length, cycle + 1);
    requests.at(-1).resolve();
    const next = await opening;
    stream ??= next;
    assert.equal(next, stream);
    assert.equal(sources.at(-1).connected, true);
    microphone.release();
    assert.equal(requests.at(-1).input.readyState, 'ended');
    assert.equal(sources.at(-1).connected, false);
    assert.equal(await microphone.open(() => false), stream);
    assert.equal(requests.length, cycle + 1);
    assert.equal(output.readyState, 'live');
  }
  microphone.close();
  assert.equal(output.readyState, 'ended');
});

test('late permission grants cannot recapture after mute or replace a newer unmute', async () => {
  const { microphone, requests, sources } = fixture();
  const first = microphone.open(() => true);
  microphone.release();
  const second = microphone.open(() => true);
  requests[1].resolve();
  await second;
  requests[0].resolve();
  await first;
  assert.equal(requests[0].input.readyState, 'ended');
  assert.equal(requests[1].input.readyState, 'live');
  assert.equal(sources.length, 1);
  microphone.close();
});

test('muting while permission is pending stops the eventual track without attaching it', async () => {
  const { microphone, requests, sources } = fixture();
  const opening = microphone.open(() => true);
  microphone.release();
  requests[0].resolve();
  await opening;
  assert.equal(requests[0].input.readyState, 'ended');
  assert.equal(sources.length, 0);
  microphone.close();
});

test('permission refusal can be retried and natural device loss reaches the page', async () => {
  const { microphone, requests, ended } = fixture();
  const first = microphone.open(() => true);
  requests[0].reject(new DOMException('Denied', 'NotAllowedError'));
  await assert.rejects(first, { name: 'NotAllowedError' });
  const second = microphone.open(() => true);
  requests[1].resolve();
  const stream = await second;
  requests[1].input.dispatchEvent(new Event('ended'));
  assert.deepEqual(ended, [stream]);
  microphone.close();
});
