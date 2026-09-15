import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import * as THREE from 'three';
import { FBXLoader } from 'three/examples/jsm/loaders/FBXLoader.js';
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
  assert.deepEqual(Object.keys(MOOD_TABLE), ['curious', 'amused', 'puzzled', 'thinking', 'pleased', 'sad', 'angry', 'apologetic', 'alert', 'sleepy', 'relaxed', 'surprised', 'skeptical']);
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
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), { waitingForHub: true, gestureState: { name: 'waiting' }, gestureOffsets: { head: [1, 0, 0] }, clips: new Map(), clipGesture: null, poseName: 'stand', gazeDestination: new THREE.Vector3(), playClip: (name) => played.push(name) });
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
  assert.deepEqual(played, ['nod', 'idle']);
  runtime.gesture('beat');
  assert.equal(runtime.gestureState.name, 'point_at');
});

test('named clip gestures play during a pending hub wait', () => {
  const played = [];
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    waitingForHub: true, gestureState: { name: 'waiting' }, gestureOffsets: { head: [0.2, 0, 0] },
    clips: new Map(), clipGesture: null, pendingGesture: null, poseName: 'stand',
    playClip: (name) => played.push(name),
  });
  assert.equal(runtime.gesture('wave'), true);
  assert.deepEqual(played, ['wave']);
  assert.equal(runtime.gestureState, null);
  assert.equal(runtime.clipGesture, 'wave');
});

test('a standing gesture requested while seated stands first and then plays', () => {
  const clip = (name, height) => new THREE.AnimationClip(name, 1, [new THREE.NumberKeyframeTrack('.position[y]', [0], [height])]);
  const played = [];
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    waitingForHub: false,
    gestureState: null,
    gestureOffsets: {},
    clips: new Map([['idle', clip('idle', 1)], ['sit-idle', clip('sit-idle', 0.5)], ['clap', clip('clap', 0.95)]]),
    clipGesture: null,
    pendingGesture: null,
    poseName: 'sit',
    playClip(name, fallback) { played.push(name); this.clipAction = { isRunning: () => false }; this.clipFallback = fallback; },
    idleClip: null,
  });
  assert.equal(runtime.gesture('clap'), true);
  assert.deepEqual(played, ['stand']);
  assert.equal(runtime.poseName, 'stand');
  runtime.updatePose(1000);
  assert.deepEqual(played, ['stand', 'clap']);
  assert.equal(runtime.clipGesture, 'clap');
});

test('procedural mood rotation composes on a running clip pose', () => {
  const bone = new THREE.Object3D();
  const clipRotation = new THREE.Quaternion().setFromEuler(new THREE.Euler(0.4, 0.2, -0.1));
  bone.quaternion.copy(clipRotation);
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    moodName: 'thinking', moodFrom: {}, moodBones: {}, moodStarted: 0,
    moodValues: Object.fromEntries(['happy', 'angry', 'sad', 'relaxed', 'surprised'].map((name) => [name, 0])),
    bones: new Map([['head', { node: bone }]]), gestureRotation: new THREE.Quaternion(),
  });
  runtime.updateMood(1000, { setValue() {} });
  assert.ok(bone.quaternion.angleTo(clipRotation) > 0.01);
  assert.ok(bone.quaternion.angleTo(new THREE.Quaternion().setFromEuler(new THREE.Euler(...MOOD_TABLE.thinking.bones.head))) > 0.01);
});

test('idle variants use random dwell and never repeat consecutively', () => {
  const played = [];
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    clips: new Map(['idle', 'idle-2', 'idle-3'].map((name) => [name, {}])),
    poseName: 'stand',
    idleClip: 'idle',
    playClip: (name) => played.push(name),
  });
  const random = Math.random;
  Math.random = () => 0;
  try {
    runtime.playIdle(1000);
    const firstDeadline = runtime.nextIdleAt;
    runtime.playIdle(firstDeadline);
  } finally {
    Math.random = random;
  }
  assert.deepEqual(played, ['idle-2', 'idle']);
  assert.equal(runtime.nextIdleAt, 15000);
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
    clips: new Map([['sit-idle', {}]]),
    poseName: 'sit',
    idleClip: null,
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
    ['idle', new THREE.AnimationClip('idle', 1, [new THREE.VectorKeyframeTrack(`${hips.uuid}.position`, [0, 0.35, 1], [0, 1.34, 0, 0, 1.38, 0, 0, 1.34, 0])])],
    ['sit-idle', new THREE.AnimationClip('sit-idle', 1, [new THREE.VectorKeyframeTrack(`${hips.uuid}.position`, [0, 0.6, 1], [0, 0.69, 0, 0, 0.74, 0, 0, 0.69, 0])])],
    ['sit', new THREE.AnimationClip('sit', 0.8, [new THREE.VectorKeyframeTrack(`${hips.uuid}.position`, [0, 0.5, 0.8], [0, 1.4, 0, 0, 0.75, 0, 0, 0.75, 0])])],
    ['stand', new THREE.AnimationClip('stand', 0.8, [new THREE.VectorKeyframeTrack(`${hips.uuid}.position`, [0, 0.5, 0.8], [0, 0.75, 0, 0, 1.4, 0, 0, 1.4, 0])])],
  ]);
  for (const [name, clip] of clips) {
    clip.userData.action = name;
    clip.userData.poseTracks = clip.tracks.map((track) => ({ name: track.name, valueSize: track.getValueSize(), interpolant: track.createInterpolant() }));
  }
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    stage,
    vrm: { scene: model },
    mixer: new THREE.AnimationMixer(model),
    clips,
    clipAction: null,
    clipFallback: null,
    clipGesture: null,
    poseName: 'stand',
    idleClip: 'idle',
    nextIdleAt: Infinity,
    handovers: new Map(),
  });
  runtime.prepareHandovers();
  const modelPosition = new THREE.Vector3();
  const headPosition = new THREE.Vector3();
  const previousModel = new THREE.Vector3();
  const previousHead = new THREE.Vector3();
  const velocities = [];
  const handoverVelocities = [];
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
      if (frame >= 30) handoverVelocities.push(velocities.at(-1));
    }
  }
  assert.equal(stage.position.y, 0);
  assert.ok(Math.max(...velocities.map(({ model: velocity }) => velocity)) < 0.001);
  const peakHeadVelocity = Math.max(...handoverVelocities.map(({ head: velocity }) => velocity));
  assert.ok(peakHeadVelocity < 0.3, `peak head velocity ${peakHeadVelocity}`);
  assert.ok(runtime.handovers.get('sit:sit-idle').offset > 0.3, JSON.stringify(runtime.handovers.get('sit:sit-idle')));
  assert.ok(runtime.handovers.get('stand:idle').offset > 0.3, JSON.stringify(runtime.handovers.get('stand:idle')));
  assert.ok(runtime.handovers.get('sit:sit-idle').duration > 1.5);
  assert.ok(runtime.handovers.get('stand:idle').duration > runtime.handovers.get('sit:sit-idle').duration);
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

test('real Mixamo rest frames preserve signed standing and seated joint angles', () => {
  const load = (name) => {
    const bytes = fs.readFileSync(new URL(`fixtures/${name}`, import.meta.url));
    return new FBXLoader().parse(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength), '');
  };
  const mockVrm0 = () => {
    const scene = new THREE.Group();
    scene.rotation.y = Math.PI;
    const nodes = {};
    const bone = (name, parent, position) => {
      const node = new THREE.Bone();
      node.name = `Normalized_${name}`;
      node.position.fromArray(position);
      (parent ?? scene).add(node);
      nodes[name] = node;
      return node;
    };
    const hips = bone('hips', null, [0, 0.9, 0]);
    const spine = bone('spine', hips, [0, 0.1, 0]);
    const chest = bone('chest', spine, [0, 0.15, 0]);
    const upperChest = bone('upperChest', chest, [0, 0.1, 0]);
    const neck = bone('neck', upperChest, [0, 0.15, 0]);
    bone('head', neck, [0, 0.1, 0]);
    for (const [side, direction] of [['left', -1], ['right', 1]]) {
      const shoulder = bone(`${side}Shoulder`, upperChest, [direction * 0.05, 0.1, 0]);
      const upperArm = bone(`${side}UpperArm`, shoulder, [direction * 0.1, 0, 0]);
      const lowerArm = bone(`${side}LowerArm`, upperArm, [direction * 0.25, 0, 0]);
      bone(`${side}Hand`, lowerArm, [direction * 0.25, 0, 0]);
      const upperLeg = bone(`${side}UpperLeg`, hips, [direction * 0.1, -0.05, 0]);
      const lowerLeg = bone(`${side}LowerLeg`, upperLeg, [0, -0.4, 0]);
      const foot = bone(`${side}Foot`, lowerLeg, [0, -0.4, 0]);
      bone(`${side}Toes`, foot, [0, -0.05, 0.1]);
    }
    return { scene, nodes, meta: { metaVersion: '0' }, humanoid: { getNormalizedBoneNode: (name) => nodes[name], getRawBoneNode: (name) => nodes[name] } };
  };
  const position = (node) => node.getWorldPosition(new THREE.Vector3());
  const signedAngle = (first, second) => THREE.MathUtils.radToDeg(Math.atan2(first.clone().cross(second).x, first.dot(second)));
  const measurements = (nodes) => {
    const torsoDown = position(nodes.hips).sub(position(nodes.spine));
    const torsoUp = torsoDown.clone().negate();
    const arm = position(nodes.leftLowerArm).sub(position(nodes.leftUpperArm));
    const thigh = position(nodes.leftLowerLeg).sub(position(nodes.leftUpperLeg));
    const shin = position(nodes.leftFoot).sub(position(nodes.leftLowerLeg));
    return { arm: signedAngle(arm, torsoDown), knee: signedAngle(thigh, shin), hip: signedAngle(torsoUp, thigh) };
  };
  const sourceNodes = (source) => Object.fromEntries([
    ['hips', 'mixamorigHips'], ['spine', 'mixamorigSpine'], ['leftUpperArm', 'mixamorigLeftArm'],
    ['leftLowerArm', 'mixamorigLeftForeArm'], ['leftUpperLeg', 'mixamorigLeftUpLeg'],
    ['leftLowerLeg', 'mixamorigLeftLeg'], ['leftFoot', 'mixamorigLeftFoot'],
  ].map(([key, name]) => [key, source.getObjectByName(name)]));
  for (const name of ['mixamo-standing.fbx', 'mixamo-seated.fbx']) {
    const source = load(name);
    const sourceHipsRestY = source.getObjectByName('mixamorigHips').position.y;
    const vrm = mockVrm0();
    vrm.scene.updateMatrixWorld(true);
    const clip = retargetMixamoClip(source, vrm);
    const sourceMixer = new THREE.AnimationMixer(source);
    const targetMixer = new THREE.AnimationMixer(vrm.scene);
    sourceMixer.clipAction(source.animations[0]).play();
    targetMixer.clipAction(clip).play();
    sourceMixer.setTime(2);
    targetMixer.setTime(2);
    source.updateMatrixWorld(true);
    vrm.scene.updateMatrixWorld(true);
    const expected = measurements(sourceNodes(source));
    const actual = measurements(vrm.nodes);
    for (const joint of ['arm', 'knee', 'hip']) {
      assert.ok(Math.abs(actual[joint] - expected[joint]) < 5, `${name} ${joint}: ${actual[joint]} versus ${expected[joint]}`);
    }
    const hipsTrack = clip.tracks.find((track) => track.name === 'Normalized_hips.position');
    const sourceTrack = source.animations[0].tracks.find((track) => track.name.endsWith('mixamorigHips.position'));
    assert.ok(Math.abs(hipsTrack.values[1] - sourceTrack.values[1] * 0.9 / sourceHipsRestY) < 1e-5);
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
