import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
import { StereoView, connectVrHost, eyeFrustum } from '../src/vr.js';

const HELLO = { type: 'hello', token: 'vr-token', eye: [4, 2], quad: [0.4, 0.4], height: 0.3, margin: 0.03 };

test('an eye frustum passes exactly through the window edges', () => {
  const rect = { left: -1, right: 1, bottom: 0, top: 2 };
  const eye = [0.3, 1.2, 5];
  const near = 0.05;
  const frustum = eyeFrustum(eye, rect, near);
  const camera = new THREE.PerspectiveCamera();
  camera.position.fromArray(eye);
  camera.updateMatrixWorld();
  camera.projectionMatrix.makePerspective(frustum.left, frustum.right, frustum.top, frustum.bottom, near, 100);
  for (const [x, y, ndcX, ndcY] of [[-1, 0, -1, -1], [1, 2, 1, 1], [0.3, 1.2, 0.3, 0.2]]) {
    const projected = new THREE.Vector3(x, y, 0).project(camera);
    assert.ok(Math.abs(projected.x - ndcX) < 1e-9, `x ${projected.x} ≠ ${ndcX}`);
    assert.ok(Math.abs(projected.y - ndcY) < 1e-9, `y ${projected.y} ≠ ${ndcY}`);
  }
});

test('a point on the window projects to the same place for both eyes, a point behind it does not', () => {
  const view = new StereoView(HELLO, () => {});
  view.pose({ eyes: [[-0.032, 0.1, 0.55], [0.032, 0.1, 0.55]], head: [0, 0.1, 0.55] });
  const project = (point) => view.eyes.map((eye) => {
    const frustum = eyeFrustum(eye, view.rect, 0.05);
    const camera = new THREE.PerspectiveCamera();
    camera.position.fromArray(eye);
    camera.updateMatrixWorld();
    camera.projectionMatrix.makePerspective(frustum.left, frustum.right, frustum.top, frustum.bottom, 0.05, 100);
    return new THREE.Vector3(...point).project(camera).x;
  });
  const [onLeft, onRight] = project(view.scene([0.05, 0.1, 0]));
  assert.ok(Math.abs(onLeft - onRight) < 1e-9);
  const [behindLeft, behindRight] = project(view.scene([0.05, 0.1, -0.1]));
  assert.ok(behindLeft < behindRight, 'uncrossed disparity for a point behind the window');
});

test('the puppet stands on the window floor at its configured height', () => {
  const view = new StereoView(HELLO, () => {});
  assert.deepEqual(view.scene([0, -0.2 + 0.03, 0]).map((value) => Math.round(value * 1e9) / 1e9), [0, 0, 0]);
  assert.ok(Math.abs(view.scene([0, -0.2 + 0.03 + 0.3, 0])[1] - 2.7) < 1e-9);
});

function fakeRenderer() {
  const calls = [];
  const gl = { RGBA: 1, UNSIGNED_BYTE: 2, readPixels: (...args) => calls.push(['readPixels', ...args.slice(0, 4)]) };
  return {
    calls,
    setPixelRatio: (ratio) => calls.push(['ratio', ratio]),
    setSize: (...args) => calls.push(['size', ...args]),
    setClearColor: () => {},
    setScissorTest: () => {},
    clear: () => calls.push(['clear']),
    setViewport: (...args) => calls.push(['viewport', ...args]),
    setScissor: () => {},
    render: (scene, camera) => calls.push(['render', camera.position.toArray()]),
    getContext: () => gl,
  };
}

test('a stereo frame renders each eye into its half and waits for the host before the next', () => {
  const sent = [];
  const view = new StereoView(HELLO, (pixels) => sent.push(pixels));
  const renderer = fakeRenderer();
  view.attach(renderer);
  assert.deepEqual(renderer.calls.splice(0), [['ratio', 1], ['size', 8, 2, false]]);
  view.render(renderer, null);
  assert.equal(renderer.calls.length, 0, 'no frame before the first pose');
  view.pose({ eyes: [[-0.03, 0, 0.5], [0.03, 0, 0.5]], head: [0, 0, 0.5] });
  view.render(renderer, null);
  assert.deepEqual(renderer.calls.filter(([name]) => name === 'viewport'), [['viewport', 0, 0, 4, 2], ['viewport', 4, 0, 4, 2]]);
  assert.deepEqual(renderer.calls.at(-1), ['readPixels', 0, 0, 8, 2]);
  assert.equal(sent.length, 1);
  assert.equal(sent[0].length, 8 * 2 * 4);
  renderer.calls.length = 0;
  view.render(renderer, null);
  assert.equal(renderer.calls.length, 0, 'unacknowledged frame holds the next');
  view.ready = true;
  view.pose({ eyes: [[-0.03, 0, -0.5], [0.03, 0, -0.5]], head: [0, 0, -0.5] });
  view.render(renderer, null);
  assert.equal(renderer.calls.length, 0, 'no frame from behind the window');
});

test('the vr host hands over its credential, poses, acknowledgements and taps', async () => {
  const sockets = [];
  class FakeSocket extends EventTarget {
    constructor(url) { super(); this.url = url; this.sent = []; sockets.push(this); queueMicrotask(() => this.dispatchEvent(new Event('open'))); }
    send(data) { this.sent.push(data); }
    deliver(message) { const event = new Event('message'); event.data = JSON.stringify(message); this.dispatchEvent(event); }
  }
  assert.throws(() => connectVrHost('nonsense', { WebSocket: FakeSocket }), /port/);
  let taps = 0;
  const pending = connectVrHost('4123.abc', { WebSocket: FakeSocket, onTap: () => taps++ });
  await new Promise((resolve) => setTimeout(resolve));
  const [socket] = sockets;
  assert.equal(socket.url, 'ws://127.0.0.1:4123/');
  assert.deepEqual(JSON.parse(socket.sent[0]), { type: 'hello', nonce: 'abc' });
  socket.deliver(HELLO);
  const { token, view } = await pending;
  assert.equal(token, 'vr-token');
  socket.deliver({ type: 'pose', eyes: [[0, 0, 1], [0.06, 0, 1]], head: [0.03, 0, 1] });
  assert.ok(view.viewer instanceof THREE.Vector3);
  view.ready = false;
  socket.deliver({ type: 'ready' });
  assert.equal(view.ready, true);
  socket.deliver({ type: 'tap' });
  assert.equal(taps, 1);
});
