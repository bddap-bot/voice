import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
import { standingPose, anchorStandingIdle } from '../src/standing.js';
import { PuppetRuntime } from '../src/puppet.js';

function rig(version) {
  const scene = new THREE.Group();
  const nodes = Object.fromEntries(Object.keys(standingPose({ meta: { metaVersion: version } })).map(name => {
    const node = new THREE.Bone();
    node.name = name;
    scene.add(node);
    return [name, node];
  }));
  return { scene, meta: { metaVersion: version }, humanoid: { getNormalizedBoneNode: name => nodes[name] } };
}

test('Standing converts the facing convention and legacy thumb joints without changing the source', () => {
  const old = standingPose(rig('0'));
  const modern = standingPose(rig('1'));
  const turn = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0, 1, 0), Math.PI);
  for (const name of Object.keys(old)) {
    const expected = new THREE.Quaternion().fromArray(old[name].rotation).premultiply(turn).multiply(turn.clone().invert());
    assert.ok(expected.angleTo(new THREE.Quaternion().fromArray(modern[name].rotation)) < 1e-7, name);
  }
  assert.ok(old.leftThumbMetacarpal);
  assert.ok(old.leftThumbProximal);
  assert.equal(old.leftThumbIntermediate, undefined);
  assert.deepEqual(standingPose(rig('0')), old);
});

for (const version of ['0', '1']) test(`standing idle preserves motion and returns from a gesture continuously in VRM ${version}`, () => {
  const vrm = rig(version);
  const pose = standingPose(vrm);
  for (const [name, value] of Object.entries(pose)) vrm.humanoid.getNormalizedBoneNode(name).quaternion.fromArray(value.rotation);
  const first = new THREE.Quaternion().setFromEuler(new THREE.Euler(0.3, -0.4, 0.2));
  const delta = new THREE.Quaternion().setFromEuler(new THREE.Euler(0.02, 0.03, -0.01));
  const peak = first.clone().multiply(delta);
  const idle = anchorStandingIdle(new THREE.AnimationClip('idle', 2, [
    new THREE.QuaternionKeyframeTrack('head.quaternion', [0, 1, 2], [...first.toArray(), ...peak.toArray(), ...first.toArray()]),
    new THREE.VectorKeyframeTrack('hips.position', [0, 1, 2], [0.1, 0.9, 0.2, 0.1, 0.92, 0.2, 0.1, 0.9, 0.2]),
  ]), vrm);
  const headTrack = idle.tracks.find(track => track.name === 'head.quaternion');
  const target = new THREE.Quaternion().fromArray(pose.head.rotation);
  assert.ok(new THREE.Quaternion().fromArray(headTrack.values).normalize().angleTo(target) < 1e-6);
  assert.ok(new THREE.Quaternion().fromArray(headTrack.values, 4).normalize().angleTo(target.clone().multiply(delta)) < 1e-6);
  assert.equal(idle.tracks.filter(track => track.name.endsWith('.quaternion')).length, Object.keys(pose).length);
  const gesture = new THREE.AnimationClip('wave', 0.5, [new THREE.QuaternionKeyframeTrack('head.quaternion', [0, 0.5], [0, 0, 0, 1, 0.3, 0, 0, Math.sqrt(0.91)])]);
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm, mixer: new THREE.AnimationMixer(vrm.scene), clips: new Map([['idle', idle], ['wave', gesture]]),
    bones: new Map(), poseName: 'stand', idleClip: null, nextIdleAt: Infinity,
  });
  runtime.playIdle(0);
  runtime.mixer.update(0);
  runtime.gesture('wave');
  let previous = vrm.humanoid.getNormalizedBoneNode('head').quaternion.clone();
  let peakStep = 0;
  for (let frame = 0; frame < 150; frame++) {
    runtime.updateBasePose(1 / 60);
    runtime.updatePose(frame * 1000 / 60);
    const rotation = vrm.humanoid.getNormalizedBoneNode('head').quaternion;
    peakStep = Math.max(peakStep, previous.angleTo(rotation));
    previous.copy(rotation);
  }
  assert.ok(peakStep < 0.1, `peak frame rotation ${peakStep}`);
  assert.equal(runtime.clipGesture, null);
  for (const [name, value] of Object.entries(pose)) {
    const distance = vrm.humanoid.getNormalizedBoneNode(name).quaternion.angleTo(new THREE.Quaternion().fromArray(value.rotation));
    assert.ok(distance < 0.04, `${name}: ${distance}`);
  }
});

test('an avatar loads and renders Standing before any clip exists', async () => {
  const names = ['hips', 'spine', 'head', 'leftUpperLeg', 'leftLowerLeg', 'leftFoot', 'rightUpperLeg', 'rightLowerLeg', 'rightFoot', 'leftUpperArm', 'leftLowerArm', 'leftHand', 'rightUpperArm', 'rightLowerArm', 'rightHand'];
  const children = { hips: ['spine', 'leftUpperLeg', 'rightUpperLeg'], spine: ['head', 'leftUpperArm', 'rightUpperArm'], leftUpperLeg: ['leftLowerLeg'], leftLowerLeg: ['leftFoot'], rightUpperLeg: ['rightLowerLeg'], rightLowerLeg: ['rightFoot'], leftUpperArm: ['leftLowerArm'], leftLowerArm: ['leftHand'], rightUpperArm: ['rightLowerArm'], rightLowerArm: ['rightHand'] };
  const gltf = {
    asset: { version: '2.0' }, scene: 0, scenes: [{ nodes: [0] }],
    nodes: names.map(name => ({ name, translation: [0, name === 'hips' ? 1 : 0.1, 0], children: (children[name] ?? []).map(child => names.indexOf(child)) })),
    extensionsUsed: ['VRMC_vrm'],
    extensions: { VRMC_vrm: { specVersion: '1.0', meta: { name: 'rig', authors: ['fixture'], licenseUrl: 'https://vrm.dev/licenses/1.0/' }, humanoid: { humanBones: Object.fromEntries(names.map((name, node) => [name, { node }])) } } },
  };
  const encoded = new TextEncoder().encode(JSON.stringify(gltf));
  const bytes = new Uint8Array(20 + Math.ceil(encoded.length / 4) * 4).fill(32);
  const header = new DataView(bytes.buffer);
  [0x46546c67, 2, bytes.length, bytes.length - 20, 0x4e4f534a].forEach((value, index) => header.setUint32(index * 4, value, true));
  bytes.set(encoded, 20);
  const progress = globalThis.ProgressEvent;
  globalThis.ProgressEvent ??= class { constructor(type, values) { Object.assign(this, { type }, values); } };
  let renders = 0;
  let finishes = 0;
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    poseName: 'sit', bones: new Map(), clips: new Map(), idleRoot: new THREE.Group(), gazeTarget: new THREE.Object3D(),
    renderer: { render: () => renders++, getContext: () => ({ finish: () => finishes++ }) },
  });
  const stages = [];
  const stage = async (name, work) => {
    const before = renders + finishes;
    const result = await work();
    stages.push([name, renders + finishes - before]);
    return result;
  };
  try {
    assert.equal(await runtime.load(bytes.buffer, undefined, undefined, stage), true);
    assert.deepEqual(stages, [['parse', 0], ['select', 0], ['fit', 0], ['render', 2]]);
    assert.equal(runtime.clips.size, 0);
    assert.equal(runtime.clipAction, null);
    const expected = standingPose(runtime.vrm);
    for (const name of names) {
      const normalized = runtime.vrm.humanoid.getNormalizedBoneNode(name);
      const raw = runtime.vrm.humanoid.getRawBoneNode(name);
      assert.ok(normalized.quaternion.angleTo(new THREE.Quaternion().fromArray(expected[name].rotation)) < 1e-6, name);
      assert.ok(raw.quaternion.angleTo(normalized.quaternion) < 1e-6, `raw ${name}`);
    }
    renders = 0;
    Object.assign(runtime, { clock: { getDelta: () => 0 }, renderer: { render: () => renders++ } });
    for (const method of ['updateBasePose', 'updatePose', 'updateSeatedClearance', 'updateGesture', 'updateListening', 'updateFace', 'recordAnimation']) runtime[method] = () => {};
    const frame = globalThis.requestAnimationFrame;
    globalThis.requestAnimationFrame = () => 1;
    try { runtime.animate(0); } finally { globalThis.requestAnimationFrame = frame; }
    assert.equal(renders, 1);
  } finally {
    globalThis.ProgressEvent = progress;
    runtime.clear();
  }
});
