import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
import { MOOD_TABLE, PuppetRuntime, audioEnergy, audioVisemes, retargetMixamoClip, shouldBeat } from '../src/puppet.js';

function waveform(amplitude) {
  return Uint8Array.from({ length: 256 }, (_, index) => 128 + Math.round(Math.sin(index / 3) * amplitude));
}

test('silence closes every viseme', () => {
  const values = audioVisemes(waveform(0), new Uint8Array(128).fill(255), 48000, 256);
  assert.deepEqual(values, { aa: 0, ih: 0, ou: 0, ee: 0, oh: 0 });
});

test('spectral bands choose different VRM visemes', () => {
  const low = new Uint8Array(128);
  low[2] = 255;
  const high = new Uint8Array(128);
  high[15] = 255;
  const lowValues = audioVisemes(waveform(32), low, 48000, 256);
  const highValues = audioVisemes(waveform(32), high, 48000, 256);
  assert.equal(Object.entries(lowValues).sort((left, right) => right[1] - left[1])[0][0], 'ou');
  assert.equal(Object.entries(highValues).sort((left, right) => right[1] - left[1])[0][0], 'ih');
  assert.ok(lowValues.ou > 0.7);
  assert.ok(highValues.ih > 0.7);
});

test('every mood has bounded expressions and a head or shoulder pose', () => {
  assert.deepEqual(Object.keys(MOOD_TABLE), ['curious', 'amused', 'puzzled', 'thinking', 'pleased', 'apologetic', 'alert', 'sleepy', 'surprised', 'skeptical']);
  for (const mood of Object.values(MOOD_TABLE)) {
    assert.ok(Object.values(mood.expressions).every((value) => value >= 0 && value <= 1));
    assert.ok(['head', 'leftShoulder', 'rightShoulder'].some((bone) => bone in mood.bones));
  }
});

test('audio energy distinguishes silence from a speech beat', () => {
  assert.equal(audioEnergy(new Uint8Array(32).fill(128)), 0);
  assert.ok(audioEnergy(Uint8Array.from({ length: 32 }, (_, index) => index % 2 ? 180 : 76)) > 0.35);
});

test('speech attacks trigger bounded beats outside hub waits', () => {
  assert.equal(shouldBeat(0.12, 0.04, false), true);
  assert.equal(shouldBeat(0.12, 0.1, false), false);
  assert.equal(shouldBeat(0.12, 0.04, true), false);
});

test('audio beats never replace an active explicit gesture', () => {
  const runtime = Object.create(PuppetRuntime.prototype);
  runtime.audio = {
    analyser: {
      fftSize: 256,
      getByteTimeDomainData: (data) => data.forEach((_, index) => { data[index] = index % 2 ? 180 : 76; }),
      getByteFrequencyData: (data) => data.fill(80),
    },
    context: { sampleRate: 48000 },
    waveform: new Uint8Array(256),
    spectrum: new Uint8Array(128),
  };
  runtime.mouthValues = { aa: 0, ih: 0, ou: 0, ee: 0, oh: 0 };
  runtime.previousEnergy = 0;
  runtime.waitingForHub = false;
  runtime.clipGesture = 'nod';
  runtime.gestureState = { name: 'point_at' };
  runtime.beginGesture = (name) => { runtime.gestureState = { name }; };
  const manager = { setValue() {} };
  runtime.updateMouth(manager);
  assert.equal(runtime.gestureState.name, 'point_at');
  runtime.gestureState = null;
  runtime.previousEnergy = 0;
  runtime.updateMouth(manager);
  assert.equal(runtime.gestureState, null);
  runtime.clipGesture = null;
  runtime.previousEnergy = 0;
  runtime.updateMouth(manager);
  assert.equal(runtime.gestureState.name, 'beat');
});

test('waiting holds a readable gesture until the hub result releases it', () => {
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), { waitingForHub: false, gestureState: null, gestureOffsets: {} });
  runtime.waiting(true);
  assert.equal(runtime.waitingForHub, true);
  assert.equal(runtime.gestureState.name, 'waiting');
  assert.equal(runtime.gestureState.releaseAt, Infinity);
  runtime.waiting(false);
  assert.equal(runtime.waitingForHub, false);
  assert.equal(runtime.gestureState.releasing, true);
});

test('explicit gestures replace a hub wait and beats cannot replace either', () => {
  const played = [];
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), { waitingForHub: true, gestureState: { name: 'waiting' }, gestureOffsets: { head: [1, 0, 0] }, clips: new Map(), clipGesture: null, poseName: 'sit', gazeDestination: new THREE.Vector3(), playClip: (name) => played.push(name) });
  runtime.gesture('beat');
  assert.equal(runtime.gestureState.name, 'waiting');
  runtime.gesture('nod');
  assert.equal(runtime.gestureState, null);
  assert.deepEqual(runtime.gestureOffsets, {});
  assert.equal(runtime.clipGesture, 'nod');
  assert.deepEqual(played, ['nod']);
  runtime.gesture('beat');
  assert.equal(runtime.clipGesture, 'nod');
  runtime.gesture('point', 'panel');
  assert.equal(runtime.gestureState.name, 'point_at');
  assert.equal(runtime.clipGesture, null);
  assert.deepEqual(played, ['nod', 'sit-idle']);
  runtime.gesture('beat');
  assert.equal(runtime.gestureState.name, 'point_at');
});

test('the look-at target glances to a fresh panel and returns to camera dwell', () => {
  const head = new THREE.Object3D();
  const target = new THREE.Object3D();
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm: { lookAt: { target } },
    gazeTarget: target,
    gazePoint: new THREE.Vector3(0, 1.25, 6.4),
    gazeDestination: new THREE.Vector3(0, 1.25, 6.4),
    gazeMode: 'camera',
    gazeUntil: 0,
    nextSaccade: Infinity,
    gazeRotation: new THREE.Quaternion(),
    bones: new Map([['head', { node: head }]]),
  });
  runtime.setGaze('panel', 2200, 0);
  for (let frame = 0; frame < 30; frame++) {
    head.quaternion.identity();
    runtime.updateGaze(100 + frame * 16);
  }
  assert.equal(runtime.gazeMode, 'panel');
  assert.ok(target.position.x > 2.5);
  const panelX = target.position.x;
  const panelHeadYaw = head.rotation.y;
  const random = Math.random;
  Math.random = () => 0.5;
  try {
    for (let frame = 0; frame < 30; frame++) {
      head.quaternion.identity();
      runtime.updateGaze(2201 + frame * 16);
    }
  } finally {
    Math.random = random;
  }
  assert.equal(runtime.gazeMode, 'camera');
  assert.ok(target.position.x < panelX * 0.1);
  assert.ok(panelHeadYaw > 0.08);
  assert.ok(Math.abs(head.rotation.y) < panelHeadYaw * 0.1);
});

test('gaze is inert when a puppet has no look-at rig', () => {
  const target = new THREE.Object3D();
  const head = new THREE.Object3D();
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm: {},
    gazeTarget: target,
    gazePoint: new THREE.Vector3(0, 1.25, 6.4),
    gazeDestination: new THREE.Vector3(2.8, 1.35, 2.4),
    gazeMode: 'panel',
    gazeUntil: 2200,
    nextSaccade: 0,
    gazeRotation: new THREE.Quaternion(),
    bones: new Map([['head', { node: head }]]),
  });
  runtime.updateGaze(1000);
  assert.deepEqual(target.position.toArray(), [0, 0, 0]);
  assert.deepEqual(head.quaternion.toArray(), [0, 0, 0, 1]);
});

test('a hub wait resumes after an explicit clip finishes', () => {
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    waitingForHub: true,
    clipGesture: 'nod',
    clipAction: { isRunning: () => false },
    clipFallback: 'sit-idle',
    gestureState: null,
    gestureOffsets: {},
    playClip() {},
  });
  runtime.updatePose(performance.now());
  assert.equal(runtime.clipGesture, null);
  assert.equal(runtime.gestureState.name, 'waiting');
  assert.equal(runtime.gestureState.releaseAt, Infinity);
});

test('a hub wait resumes after an explicit procedural gesture releases', () => {
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    waitingForHub: true,
    gestureState: { name: 'point_at', from: {}, to: {}, started: 0, releaseAt: Infinity, releasing: true },
    gestureOffsets: {},
    bones: new Map(),
  });
  runtime.updateGesture(1000);
  assert.equal(runtime.gestureState.name, 'waiting');
  assert.equal(runtime.gestureState.releaseAt, Infinity);
});

test('sit and stand transitions keep model-root and head velocity bounded', () => {
  const stage = new THREE.Group();
  const model = new THREE.Group();
  const hips = new THREE.Bone();
  const head = new THREE.Bone();
  head.position.y = 1;
  hips.add(head);
  model.add(hips);
  stage.add(model);
  const clips = new Map([
    ['idle', new THREE.AnimationClip('idle', 1, [new THREE.NumberKeyframeTrack(`${hips.uuid}.position[y]`, [0, 1], [1.4, 1.4])])],
    ['sit-idle', new THREE.AnimationClip('sit-idle', 1, [new THREE.NumberKeyframeTrack(`${hips.uuid}.position[y]`, [0, 1], [0.75, 0.75])])],
    ['sit', new THREE.AnimationClip('sit', 0.8, [new THREE.NumberKeyframeTrack(`${hips.uuid}.position[y]`, [0, 0.8], [1.4, 0.75])])],
    ['stand', new THREE.AnimationClip('stand', 0.8, [new THREE.NumberKeyframeTrack(`${hips.uuid}.position[y]`, [0, 0.8], [0.75, 1.4])])],
  ]);
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    stage,
    vrm: { scene: model },
    mixer: new THREE.AnimationMixer(model),
    clips,
    clipAction: null,
    clipFallback: null,
    clipGesture: null,
    poseName: 'stand',
  });
  const modelPosition = new THREE.Vector3();
  const headPosition = new THREE.Vector3();
  const previousModel = new THREE.Vector3();
  const previousHead = new THREE.Vector3();
  const velocities = [];
  const sample = () => {
    stage.updateMatrixWorld(true);
    model.getWorldPosition(modelPosition);
    head.getWorldPosition(headPosition);
    velocities.push({ model: modelPosition.distanceTo(previousModel) * 60, head: headPosition.distanceTo(previousHead) * 60 });
    previousModel.copy(modelPosition);
    previousHead.copy(headPosition);
  };
  runtime.playClip('idle', 'idle');
  for (let frame = 0; frame < 15; frame++) runtime.mixer.update(1 / 60);
  stage.updateMatrixWorld(true);
  model.getWorldPosition(previousModel);
  head.getWorldPosition(previousHead);
  for (const pose of ['sit', 'stand']) {
    runtime.pose(pose);
    for (let frame = 0; frame < 60; frame++) {
      runtime.mixer.update(1 / 60);
      runtime.updatePose(frame * 1000 / 60);
      sample();
    }
  }
  assert.equal(stage.position.y, 0);
  assert.ok(Math.max(...velocities.map(({ model: velocity }) => velocity)) < 0.001);
  const peakHeadVelocity = Math.max(...velocities.map(({ head: velocity }) => velocity));
  assert.ok(peakHeadVelocity < 3, `peak head velocity ${peakHeadVelocity}`);
});

test('Mixamo rest rotations preserve an upright VRM bone-space invariant', () => {
  const source = new THREE.Group();
  const rig = new THREE.Bone();
  rig.name = 'mixamorigRoot';
  const hips = new THREE.Bone();
  hips.name = 'mixamorigHips';
  rig.rotation.set(0.35, -0.2, 0.7);
  hips.rotation.set(-0.4, 0.3, 1.1);
  source.add(rig);
  rig.add(hips);
  source.updateMatrixWorld(true);
  const rest = hips.quaternion.toArray();
  const rotation = new THREE.QuaternionKeyframeTrack('mixamorigHips.quaternion', [0, 1], [...rest, ...rest]);
  const position = new THREE.VectorKeyframeTrack('mixamorigHips.position', [0, 1], [0, 100, 0, 0, 110, 0]);
  source.animations = [new THREE.AnimationClip('Idle', 1, [rotation, position])];

  const target = new THREE.Group();
  const targetHips = new THREE.Bone();
  targetHips.name = 'NormalizedHips';
  const head = new THREE.Bone();
  head.position.y = 1;
  const leftFoot = new THREE.Bone();
  leftFoot.position.set(-0.2, -1, 0);
  const rightFoot = new THREE.Bone();
  rightFoot.position.set(0.2, -1, 0);
  target.add(targetHips);
  targetHips.add(head, leftFoot, rightFoot);
  const clip = retargetMixamoClip(source, { humanoid: { getNormalizedBoneNode: (name) => name === 'hips' ? targetHips : null } });
  assert.deepEqual(clip.tracks.map((track) => track.name), ['NormalizedHips.quaternion', 'NormalizedHips.position']);
  assert.ok(Math.abs(clip.tracks[1].values[4] - 1.1) < 1e-6);

  const mixer = new THREE.AnimationMixer(target);
  mixer.clipAction(clip).play();
  const hipsWorld = new THREE.Vector3();
  const headWorld = new THREE.Vector3();
  const footWorld = new THREE.Vector3();
  for (const time of [0, 0.25, 0.5, 0.75, 1]) {
    mixer.setTime(time);
    target.updateMatrixWorld(true);
    targetHips.getWorldPosition(hipsWorld);
    head.getWorldPosition(headWorld);
    assert.ok(headWorld.clone().sub(hipsWorld).angleTo(new THREE.Vector3(0, 1, 0)) < 1e-5);
    for (const foot of [leftFoot, rightFoot]) {
      foot.getWorldPosition(footWorld);
      assert.ok(footWorld.y < hipsWorld.y);
    }
  }
});

test('VRM0 retarget flips quaternion and root-motion x/z axes', () => {
  const source = new THREE.Group();
  const hips = new THREE.Bone();
  hips.name = 'mixamorigHips';
  source.add(hips);
  const rotation = new THREE.QuaternionKeyframeTrack('mixamorigHips.quaternion', [0], [0.1, 0.2, 0.3, 0.9]);
  const position = new THREE.VectorKeyframeTrack('mixamorigHips.position', [0], [100, 200, 300]);
  source.animations = [new THREE.AnimationClip('Move', 1, [rotation, position])];
  const target = new THREE.Bone();
  target.name = 'NormalizedHips';
  const clip = retargetMixamoClip(source, { meta: { metaVersion: '0' }, humanoid: { getNormalizedBoneNode: () => target } });
  const expected = new THREE.Quaternion(0.1, 0.2, 0.3, 0.9).normalize();
  assert.ok(Math.abs(clip.tracks[0].values[0] + expected.x) < 1e-6);
  assert.ok(Math.abs(clip.tracks[0].values[1] - expected.y) < 1e-6);
  assert.ok(Math.abs(clip.tracks[0].values[2] + expected.z) < 1e-6);
  assert.deepEqual(Array.from(clip.tracks[1].values), [-1, 2, -3]);
});
