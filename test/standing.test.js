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

const names = ['hips', 'spine', 'head', 'leftUpperLeg', 'leftLowerLeg', 'leftFoot', 'rightUpperLeg', 'rightLowerLeg', 'rightFoot', 'leftUpperArm', 'leftLowerArm', 'leftHand', 'rightUpperArm', 'rightLowerArm', 'rightHand'];

function puppetFile({ meshes = [] } = {}) {
  const children = { hips: ['spine', 'leftUpperLeg', 'rightUpperLeg'], spine: ['head', 'leftUpperArm', 'rightUpperArm'], leftUpperLeg: ['leftLowerLeg'], leftLowerLeg: ['leftFoot'], rightUpperLeg: ['rightLowerLeg'], rightLowerLeg: ['rightFoot'], leftUpperArm: ['leftLowerArm'], leftLowerArm: ['leftHand'], rightUpperArm: ['rightLowerArm'], rightLowerArm: ['rightHand'] };
  const positions = new Float32Array(meshes.flatMap(({ height }) => [0, 0, 0, 0.1, 0, 0, 0, height, 0]));
  const gltf = {
    asset: { version: '2.0' }, scene: 0, scenes: [{ nodes: [0, ...meshes.map((_, index) => names.length + index)] }],
    nodes: [
      ...names.map(name => ({ name, translation: [0, name === 'hips' ? 1 : 0.1, 0], children: (children[name] ?? []).map(child => names.indexOf(child)) })),
      ...meshes.map(({ name, visible }, index) => ({ name, mesh: index, ...visible === undefined ? {} : { extensions: { KHR_node_visibility: { visible } } } })),
    ],
    meshes: meshes.map((_, index) => ({ primitives: [{ attributes: { POSITION: index } }] })),
    accessors: meshes.map(({ height }, index) => ({ bufferView: 0, byteOffset: index * 36, componentType: 5126, count: 3, type: 'VEC3', min: [0, 0, 0], max: [0.1, height, 0] })),
    bufferViews: meshes.length ? [{ buffer: 0, byteLength: positions.byteLength }] : [],
    buffers: meshes.length ? [{ byteLength: positions.byteLength }] : [],
    extensionsUsed: ['VRMC_vrm', ...meshes.some(({ visible }) => visible !== undefined) ? ['KHR_node_visibility'] : []],
    extensions: { VRMC_vrm: { specVersion: '1.0', meta: { name: 'rig', authors: ['fixture'], licenseUrl: 'https://vrm.dev/licenses/1.0/' }, humanoid: { humanBones: Object.fromEntries(names.map((name, node) => [name, { node }])) } } },
  };
  if (!meshes.length) { delete gltf.meshes; delete gltf.accessors; delete gltf.bufferViews; delete gltf.buffers; }
  const encoded = new TextEncoder().encode(JSON.stringify(gltf));
  const json = Math.ceil(encoded.length / 4) * 4;
  const bin = positions.byteLength ? 8 + positions.byteLength : 0;
  const bytes = new Uint8Array(20 + json + bin).fill(32);
  const header = new DataView(bytes.buffer);
  [0x46546c67, 2, bytes.length, json, 0x4e4f534a].forEach((value, index) => header.setUint32(index * 4, value, true));
  bytes.set(encoded, 20);
  if (bin) {
    header.setUint32(20 + json, positions.byteLength, true);
    header.setUint32(24 + json, 0x004e4942, true);
    bytes.set(new Uint8Array(positions.buffer), 28 + json);
  }
  return bytes.buffer;
}

function loadingRuntime() {
  const idleRoot = new THREE.Group();
  return Object.assign(Object.create(PuppetRuntime.prototype), {
    poseName: 'sit', clips: new Map(), idleRoot, mixer: new THREE.AnimationMixer(idleRoot), gazeTarget: new THREE.Object3D(),
    renderer: { render() {}, getContext: () => ({ finish() {} }) },
  });
}

async function withProgressEvent(work) {
  const progress = globalThis.ProgressEvent;
  globalThis.ProgressEvent ??= class { constructor(type, values) { Object.assign(this, { type }, values); } };
  try { return await work(); } finally { globalThis.ProgressEvent = progress; }
}

test('a node the file hides by default stays hidden, and the figure fits to what is drawn', () => withProgressEvent(async () => {
  const runtime = loadingRuntime();
  try {
    assert.equal(await runtime.load(puppetFile({ meshes: [{ name: 'figure', height: 1.8 }, { name: 'prop', height: 20, visible: false }, { name: 'shown', height: 1, visible: true }] })), true);
    const byName = (name) => runtime.vrm.scene.getObjectByName(name);
    assert.equal(byName('prop').visible, false);
    assert.equal(byName('figure').visible, true);
    assert.equal(byName('shown').visible, true);
    assert.ok(Math.abs(runtime.vrm.scene.scale.y - 2.7 / 1.8) < 1e-6, `scale ${runtime.vrm.scene.scale.y}`);
  } finally { runtime.clear(); }
}));

test('an avatar loads and renders Standing before any clip exists', () => withProgressEvent(async () => {
  let renders = 0;
  let finishes = 0;
  const runtime = Object.assign(loadingRuntime(), {
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
    assert.equal(await runtime.load(puppetFile(), undefined, undefined, stage), true);
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
    runtime.clear();
  }
}));

function skinnedFile() {
  const children = { hips: ['spine', 'leftUpperLeg', 'rightUpperLeg'], spine: ['head', 'leftUpperArm', 'rightUpperArm'], leftUpperLeg: ['leftLowerLeg'], leftLowerLeg: ['leftFoot'], rightUpperLeg: ['rightLowerLeg'], rightLowerLeg: ['rightFoot'], leftUpperArm: ['leftLowerArm'], leftLowerArm: ['leftHand'], rightUpperArm: ['rightLowerArm'], rightLowerArm: ['rightHand'] };
  const forearm = names.indexOf('leftLowerArm');
  const triangle = [[0.5, 1.3, 0], [0.6, 1.3, 0], [0.5, 1.4, 0]];
  const positions = new Float32Array([...triangle, ...triangle.map(([x, y, z]) => [x, y + 0.05, z])].flat());
  const joints = new Uint8Array(Array(6).fill([0, 0, 0, 0]).flat());
  const weights = new Float32Array(Array(6).fill([1, 0, 0, 0]).flat());
  const inverseBinds = new Float32Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, -1.3, 0, 1]);
  const views = [positions, joints, weights, inverseBinds];
  let offset = 0;
  const bufferViews = views.map((view) => { const entry = { buffer: 0, byteOffset: offset, byteLength: view.byteLength }; offset += Math.ceil(view.byteLength / 4) * 4; return entry; });
  const binary = new Uint8Array(offset);
  views.forEach((view, index) => binary.set(new Uint8Array(view.buffer), bufferViews[index].byteOffset));
  const bound = (axis, pick) => pick(...[...positions].filter((_, at) => at % 3 === axis));
  const gltf = {
    asset: { version: '2.0' }, scene: 0, scenes: [{ nodes: [0, names.length] }],
    nodes: [
      ...names.map(name => ({ name, translation: [0, name === 'hips' ? 1 : 0.1, 0], children: (children[name] ?? []).map(child => names.indexOf(child)) })),
      { name: 'sleeves', mesh: 0, skin: 0 },
    ],
    meshes: [{ primitives: [{ attributes: { POSITION: 0, JOINTS_0: 1, WEIGHTS_0: 2 } }] }],
    skins: [{ joints: [forearm], inverseBindMatrices: 3 }],
    accessors: [
      { bufferView: 0, componentType: 5126, count: 6, type: 'VEC3', min: [0, 1, 2].map((axis) => bound(axis, Math.min)), max: [0, 1, 2].map((axis) => bound(axis, Math.max)) },
      { bufferView: 1, componentType: 5121, count: 6, type: 'VEC4' },
      { bufferView: 2, componentType: 5126, count: 6, type: 'VEC4' },
      { bufferView: 3, componentType: 5126, count: 1, type: 'MAT4' },
    ],
    bufferViews, buffers: [{ byteLength: binary.byteLength }],
    extensionsUsed: ['VRMC_vrm'],
    extensions: { VRMC_vrm: { specVersion: '1.0', meta: { name: 'rig', authors: ['fixture'], licenseUrl: 'https://vrm.dev/licenses/1.0/' }, humanoid: { humanBones: Object.fromEntries(names.map((name, node) => [name, { node }])) } } },
  };
  const encoded = new TextEncoder().encode(JSON.stringify(gltf));
  const json = Math.ceil(encoded.length / 4) * 4;
  const bytes = new Uint8Array(20 + json + 8 + binary.byteLength).fill(32);
  const header = new DataView(bytes.buffer);
  [0x46546c67, 2, bytes.length, json, 0x4e4f534a].forEach((value, index) => header.setUint32(index * 4, value, true));
  bytes.set(encoded, 20);
  header.setUint32(20 + json, binary.byteLength, true);
  header.setUint32(24 + json, 0x004e4942, true);
  bytes.set(binary, 28 + json);
  return bytes.buffer;
}

test('a mesh merged from several parts follows the arm its skin binds through a clip', () => withProgressEvent(async () => {
  const runtime = Object.assign(loadingRuntime(), { poseName: 'stand', handovers: new Map() });
  try {
    assert.equal(await runtime.load(skinnedFile()), true);
    const sleeves = runtime.vrm.scene.getObjectByName('sleeves');
    const skinned = () => {
      runtime.vrm.scene.updateMatrixWorld(true);
      sleeves.skeleton.update();
      const position = sleeves.geometry.getAttribute('position');
      return [0, 3].map((vertex) => sleeves.applyBoneTransform(vertex, new THREE.Vector3().fromBufferAttribute(position, vertex)).applyMatrix4(sleeves.matrixWorld));
    };
    const rest = skinned();
    const turn = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0, 0, 1), Math.PI / 2).toArray();
    await runtime.loadClips([{ action: 'idle', format: 'motion', data: { name: 'idle', duration: 1, tracks: [{ name: 'leftUpperArm.quaternion', times: [0, 1], values: [...turn, ...turn] }] } }]);
    runtime.mixer.update(0.5);
    runtime.vrm.update(0);
    const moved = skinned();
    for (const part of [0, 1]) assert.ok(moved[part].distanceTo(rest[part]) > 0.3, `the arm moves part ${part}: ${rest[part].toArray()} → ${moved[part].toArray()}`);
    assert.ok(Math.abs(moved[0].distanceTo(moved[1]) - rest[0].distanceTo(rest[1])) < 1e-4, `the parts move together: ${moved[0].toArray()} ${moved[1].toArray()}`);
  } finally { runtime.clear(); }
}));
