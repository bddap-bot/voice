import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
import { PuppetRuntime } from '../src/puppet.js';

test('the puppet stage preserves hips world XZ throughout an idle cycle', () => {
  Object.assign(globalThis, {
    devicePixelRatio: 1,
    ResizeObserver: class { observe() {} disconnect() {} },
    requestAnimationFrame: () => 1,
    cancelAnimationFrame: () => {},
  });
  const renderer = { setPixelRatio() {}, setSize() {}, dispose() {} };
  const runtime = new PuppetRuntime({ clientWidth: 320, clientHeight: 240 }, renderer);
  runtime.pause();
  const hips = new THREE.Object3D();
  hips.position.set(0.125, 1, -0.25);
  runtime.idleRoot.add(hips);
  const anchor = hips.getWorldPosition(new THREE.Vector3());
  for (let frame = 0; frame < 360; frame++) {
    runtime.updateBasePose(1 / 60);
    const point = hips.getWorldPosition(new THREE.Vector3());
    assert.ok(Math.hypot(point.x - anchor.x, point.z - anchor.z) < 1e-7, `hips world XZ drift at frame ${frame}`);
  }
  runtime.dispose();
});
