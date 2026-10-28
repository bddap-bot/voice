import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
import { standingPose } from '../src/standing.js';
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

for (const version of ['0', '1']) test(`loading an idle keeps authored arm and leg tracks in VRM ${version}`, async () => {
  const vrm = rig(version);
  const rotation = new THREE.Quaternion().setFromEuler(new THREE.Euler(0.3, -0.4, 0.2)).toArray();
  const tracks = ['leftUpperArm', 'leftLowerArm', 'leftUpperLeg', 'leftLowerLeg'].map(bone => ({ name: `${bone}.quaternion`, times: [0, 1], values: [...rotation, ...rotation] }));
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm, clips: new Map(), poseName: 'stand', clipAction: {}, prepareHandovers() {},
  });
  await runtime.loadClips([{ action: 'idle', format: 'motion', data: { name: 'idle', duration: 1, tracks } }]);
  const clip = runtime.clips.get('idle');
  assert.equal(clip.tracks.length, tracks.length);
  for (const track of clip.tracks) {
    const expected = rotation.map((v, i) => version === '0' && i % 2 === 0 ? -v : v);
    assert.ok(new THREE.Quaternion().fromArray(track.values).normalize().angleTo(new THREE.Quaternion().fromArray(expected)) < 1e-6, track.name);
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
  const idleRoot = new THREE.Group();
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    poseName: 'sit', clips: new Map(), idleRoot, mixer: new THREE.AnimationMixer(idleRoot), gazeTarget: new THREE.Object3D(),
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
    for (const method of ['updateBasePose', 'updatePose', 'updateFace', 'recordAnimation']) runtime[method] = () => {};
    runtime.animate(0);
    assert.equal(renders, 1);
  } finally {
    globalThis.ProgressEvent = progress;
    runtime.clear();
  }
});
