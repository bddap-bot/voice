import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
import { StereoView, connectVrHost, eyeFrustum, puppetBounds } from '../src/vr.js';

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
  const gl = { RGBA: 1, UNSIGNED_BYTE: 2, readPixels: (...args) => calls.push(['readPixels', ...args.slice(0, 4), args[7]]) };
  return {
    calls,
    setPixelRatio: (ratio) => calls.push(['ratio', ratio]),
    setSize: (...args) => calls.push(['size', ...args]),
    setClearColor: () => {},
    setScissorTest: () => {},
    clear: () => calls.push(['clear']),
    setViewport: (...args) => calls.push(['viewport', ...args]),
    setScissor: () => {},
    render: (scene, camera) => {
      scene.updateMatrixWorld();
      scene.traverse((object) => object.skeleton?.update());
      calls.push(['render', camera.position.toArray()]);
    },
    getContext: () => gl,
  };
}

function header(frame) {
  const view = new DataView(frame.buffer, frame.byteOffset, 8);
  return [0, 1, 2, 3].map((index) => view.getUint16(index * 2, true));
}

function box(size, at) {
  const mesh = new THREE.Mesh(new THREE.BoxGeometry(size, size, size), new THREE.MeshBasicMaterial());
  mesh.position.fromArray(at);
  const scene = new THREE.Scene();
  scene.add(mesh);
  return scene;
}

test('each host pose requests one stereo frame, each eye rendered into its half', () => {
  const sent = [];
  const view = new StereoView({ ...HELLO, eye: [64, 64] }, (frame) => sent.push(frame));
  const renderer = fakeRenderer();
  view.attach(renderer);
  assert.deepEqual(renderer.calls.splice(0), [['ratio', 1], ['size', 128, 64, false]]);
  const scene = box(0.5, [0, 1.35, 0]);
  view.render(renderer, scene);
  assert.equal(renderer.calls.length, 0, 'no frame before the first pose');
  view.onPose = () => view.render(renderer, scene);
  view.pose({ eyes: [[-0.03, 0, 0.5], [0.03, 0, 0.5]], head: [0, 0, 0.5] });
  assert.deepEqual(renderer.calls.filter(([name]) => name === 'viewport'), [['viewport', 0, 0, 64, 64], ['viewport', 64, 0, 64, 64]]);
  assert.equal(sent.length, 1);
  const [x, y, w, h] = header(sent[0]);
  assert.ok(w > 0 && h > 0 && w < 32 && h < 32, `a small box crops to a small rect, got ${w}×${h}`);
  assert.deepEqual(renderer.calls.filter(([name]) => name === 'readPixels'), [['readPixels', x, y, w, h, 8], ['readPixels', 64 + x, y, w, h, 8 + w * h * 4]]);
  assert.equal(sent[0].length, 8 + 2 * w * h * 4);
  renderer.calls.length = 0;
  view.pose({ eyes: [[-0.03, 0, -0.5], [0.03, 0, -0.5]], head: [0, 0, -0.5] });
  assert.equal(renderer.calls.length, 0, 'no frame from behind the window');
});

test('an empty scene sends an empty rect and reads no pixels', () => {
  const sent = [];
  const view = new StereoView(HELLO, (frame) => sent.push(frame));
  const renderer = fakeRenderer();
  view.attach(renderer);
  view.pose({ eyes: [[-0.03, 0, 0.5], [0.03, 0, 0.5]], head: [0, 0, 0.5] });
  view.render(renderer, new THREE.Scene());
  assert.deepEqual(header(sent[0]), [0, 0, 0, 0]);
  assert.equal(sent[0].length, 8);
  assert.equal(renderer.calls.filter(([name]) => name === 'readPixels').length, 0);
});

test('a box straddling an eye sends the whole frame', () => {
  const sent = [];
  const view = new StereoView(HELLO, (frame) => sent.push(frame));
  const renderer = fakeRenderer();
  view.attach(renderer);
  view.pose({ eyes: [[-0.03, 0, 0.5], [0.03, 0, 0.5]], head: [0, 0, 0.5] });
  view.render(renderer, box(20, [0, 1.35, 0]));
  assert.deepEqual(header(sent[0]), [0, 0, 4, 2]);
});

function skinnedPuppet() {
  const geometry = new THREE.CylinderGeometry(0.2, 0.2, 2, 8, 8);
  geometry.translate(0, 1, 0);
  const position = geometry.attributes.position;
  const indices = [];
  const weights = [];
  const morph = [];
  for (let vertex = 0; vertex < position.count; vertex++) {
    const upper = THREE.MathUtils.clamp(position.getY(vertex) - 0.5, 0, 1);
    indices.push(0, 1, 0, 0);
    weights.push(1 - upper, upper, 0, 0);
    morph.push(position.getX(vertex) * 1.5, 0, position.getZ(vertex));
  }
  geometry.setAttribute('skinIndex', new THREE.Uint16BufferAttribute(indices, 4));
  geometry.setAttribute('skinWeight', new THREE.Float32BufferAttribute(weights, 4));
  geometry.morphAttributes.position = [new THREE.Float32BufferAttribute(morph, 3)];
  geometry.morphTargetsRelative = true;
  const mesh = new THREE.SkinnedMesh(geometry, new THREE.MeshBasicMaterial());
  const root = new THREE.Bone();
  const arm = new THREE.Bone();
  arm.position.y = 0.5;
  root.add(arm);
  mesh.add(root);
  mesh.bind(new THREE.Skeleton([root, arm]));
  mesh.updateMorphTargets();
  const scene = new THREE.Scene();
  const holder = new THREE.Group();
  holder.scale.setScalar(1.1);
  holder.position.set(0.3, 0.2, 0);
  holder.add(mesh);
  scene.add(holder);
  arm.rotation.z = 1.2;
  mesh.morphTargetInfluences[0] = 1;
  return { scene, mesh };
}

test('the crop holds every skinned, morphed vertex in both eyes', () => {
  const sent = [];
  const view = new StereoView({ ...HELLO, eye: [256, 256] }, (frame) => sent.push(frame));
  const renderer = fakeRenderer();
  view.attach(renderer);
  const cameras = [];
  const render = renderer.render;
  renderer.render = (scene, camera) => {
    render(scene, camera);
    cameras.push(camera.clone());
  };
  const { scene, mesh } = skinnedPuppet();
  view.pose({ eyes: [[-0.032, 0.1, 0.55], [0.032, 0.1, 0.55]], head: [0, 0.1, 0.55] });
  view.render(renderer, scene);
  const [x, y, w, h] = header(sent[0]);
  assert.ok(w * h < 256 * 256 / 2, `crop ${w}×${h} is well under the eye`);
  const point = new THREE.Vector3();
  let seen = 0;
  for (let vertex = 0; vertex < mesh.geometry.attributes.position.count; vertex++) {
    mesh.getVertexPosition(vertex, point);
    point.applyMatrix4(mesh.matrixWorld);
    for (const camera of cameras) {
      const ndc = point.clone().project(camera);
      const px = (ndc.x + 1) / 2 * 256;
      const py = (ndc.y + 1) / 2 * 256;
      assert.ok(px >= x && px <= x + w && py >= y && py <= y + h, `vertex ${vertex} at ${px.toFixed(1)},${py.toFixed(1)} outside ${[x, y, w, h]}`);
      seen++;
    }
  }
  assert.ok(seen > 100);
  const tight = new THREE.Box3();
  for (let vertex = 0; vertex < mesh.geometry.attributes.position.count; vertex++) tight.expandByPoint(mesh.getVertexPosition(vertex, point).applyMatrix4(mesh.matrixWorld));
  const loose = puppetBounds(scene).box;
  assert.ok(loose.expandByScalar(1e-6).containsBox(tight));
});

test('the vr host hands over its credential, poses and taps', async () => {
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
  socket.deliver({ type: 'tap' });
  assert.equal(taps, 1);
});
