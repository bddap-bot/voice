import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { MOOD_TABLE, PuppetRuntime, animationClip, audioEnergy, loudnessViseme, pointAtOffsets, screenTarget, shouldBeat, transcriptVisemes } from '../src/puppet.js';

function waveform(amplitude) {
  return Uint8Array.from({ length: 256 }, (_, index) => 128 + Math.round(Math.sin(index / 3) * amplitude));
}

test('silence closes every transcript-selected viseme', () => {
  const values = loudnessViseme('aa', waveform(0));
  assert.deepEqual(values, { aa: 0, ih: 0, ou: 0, ee: 0, oh: 0 });
});

test('transcript vowels select all five VRM visemes over the loudness envelope', () => {
  assert.deepEqual(transcriptVisemes('A I U E O'), ['aa', null, 'ih', null, 'ou', null, 'ee', null, 'oh']);
  for (const name of ['aa', 'ih', 'ou', 'ee', 'oh']) {
    const values = loudnessViseme(name, waveform(32));
    assert.ok(values[name] > 0.7);
    assert.equal(Object.values(values).filter(Boolean).length, 1);
  }
});

test('speech deltas queue text shapes at their audio time and preserve chunk order', () => {
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), { speech: [], speechUntil: 0 });
  runtime.speak('ai', 100);
  runtime.speak('u', 120);
  assert.deepEqual(runtime.speech, [{ name: 'aa', at: 100 }, { name: 'ih', at: 172 }, { name: 'ou', at: 244 }]);
  assert.equal(runtime.speechUntil, 316);
  runtime.speak('e', 500);
  assert.deepEqual(runtime.speech, [{ name: 'ee', at: 500 }]);
});

test('live amplitude opens the mouth between transcript vowels', () => {
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    audio: { analyser: { getByteTimeDomainData: (data) => data.set(waveform(32)) }, waveform: new Uint8Array(256) },
    speech: [{ name: null, at: 100 }], speechUntil: 200,
    mouthValues: { aa: 0, ih: 0, ou: 0, ee: 0, oh: 0 }, previousEnergy: 0,
    waitingForHub: true,
  });
  const values = {};
  runtime.updateMouth({ setValue: (name, value) => { values[name] = value; } }, 150);
  assert.ok(values.aa > 0.4);
  assert.deepEqual(Object.keys(values).filter((name) => name !== 'aa' && values[name]), []);
});

test('moods have bounded expressions and poses, with empty neutral targets', () => {
  assert.deepEqual(Object.keys(MOOD_TABLE), ['neutral', 'curious', 'amused', 'puzzled', 'thinking', 'pleased', 'sad', 'angry', 'apologetic', 'alert', 'sleepy', 'relaxed', 'surprised', 'skeptical']);
  assert.deepEqual(MOOD_TABLE.neutral, { expressions: {}, bones: {} });
  for (const [name, mood] of Object.entries(MOOD_TABLE)) {
    assert.ok(Object.values(mood.expressions).every((value) => value >= 0 && value <= 1));
    if (name !== 'neutral') assert.ok(['head', 'leftShoulder', 'rightShoulder'].some((bone) => bone in mood.bones));
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
  runtime.speech = [{ name: 'aa', at: 0 }];
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
    bones: new Map([['head', { node: bone }]]), gestureRotation: new THREE.Quaternion(),
  });
  runtime.updateMood(1000, { expressions: [] });
  assert.ok(bone.quaternion.angleTo(clipRotation) > 0.01);
  assert.ok(bone.quaternion.angleTo(new THREE.Quaternion().setFromEuler(new THREE.Euler(...MOOD_TABLE.thinking.bones.head))) > 0.01);
});

test('procedural rotations stay bounded without a clip and across a clip handover', () => {
  const run = (handover) => {
    const head = new THREE.Bone();
    const rest = head.quaternion.clone();
    const first = new THREE.Quaternion().setFromEuler(new THREE.Euler(0.08, -0.12, 0.04));
    const second = new THREE.Quaternion().setFromEuler(new THREE.Euler(-0.14, 0.18, -0.06));
    let frame = 0;
    const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
      bones: new Map([['head', { node: head, rest, base: rest.clone() }]]),
      mixer: { update() { if (handover && frame <= 90) head.quaternion.copy(frame < 90 ? first : second); } },
      gestureRotation: new THREE.Quaternion(),
      listeningMotion: { lean: 0, nod: 0.03, tilt: 0.04 },
    });
    const samples = [];
    for (; frame <= 180; frame++) {
      runtime.updateBasePose(1 / 60);
      runtime.updateListening();
      if (frame % 30 === 0) samples.push(head.quaternion.clone());
    }
    return { rest, first, second, samples };
  };
  const idle = run(false);
  assert.ok(Math.max(...idle.samples.map((rotation) => rotation.angleTo(idle.rest))) < 0.06);
  assert.ok(idle.samples.at(-1).angleTo(idle.samples.at(-2)) < 1e-7);
  const clipped = run(true);
  assert.ok(clipped.samples[2].angleTo(clipped.first) < 0.06);
  assert.ok(clipped.samples.at(-1).angleTo(clipped.second) < 0.06);
  assert.ok(clipped.samples.at(-1).angleTo(clipped.samples.at(-2)) < 1e-7);
});

test('seated poses move both upper arms outward without changing standing poses', () => {
  const left = new THREE.Object3D();
  const right = new THREE.Object3D();
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    poseName: 'sit',
    bones: new Map([['leftUpperArm', { node: left }], ['rightUpperArm', { node: right }]]),
    gestureRotation: new THREE.Quaternion(),
  });
  runtime.updateSeatedClearance();
  assert.ok(left.rotation.z < -0.09);
  assert.ok(right.rotation.z > 0.09);
  left.rotation.set(0, 0, 0);
  right.rotation.set(0, 0, 0);
  runtime.poseName = 'stand';
  runtime.updateSeatedClearance();
  assert.deepEqual(left.quaternion.toArray(), [0, 0, 0, 1]);
  assert.deepEqual(right.quaternion.toArray(), [0, 0, 0, 1]);
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

test('point-at uses the near hand for panels on either side', () => {
  const camera = new THREE.PerspectiveCamera(28, 1, 0.05, 30);
  camera.position.set(0, 1.25, 6.4);
  camera.lookAt(0, 1.25, 0);
  camera.updateMatrixWorld();
  camera.updateProjectionMatrix();
  const viewport = { left: 300, top: 0, width: 400, height: 700 };
  const panels = [
    { rect: { left: 720, top: 200, width: 250, height: 340 }, near: 'left', far: 'right' },
    { rect: { left: 30, top: 180, width: 220, height: 360 }, near: 'right', far: 'left' },
  ];
  for (const { rect, near, far } of panels) {
    const target = screenTarget(camera, viewport, rect);
    const torso = new THREE.Object3D();
    const hands = {};
    const shoulders = {};
    const bones = new Map();
    for (const side of ['left', 'right']) {
      const upper = new THREE.Object3D();
      const lower = new THREE.Object3D();
      const hand = new THREE.Object3D();
      upper.position.set(side === 'left' ? 0.2 : -0.2, 1.35, 0);
      lower.position.y = 0.45;
      hand.position.y = 0.42;
      upper.rotation.set(0.25, side === 'left' ? -0.35 : 0.35, side === 'left' ? 0.4 : -0.4);
      torso.add(upper);
      upper.add(lower);
      lower.add(hand);
      bones.set(`${side}UpperArm`, { node: upper });
      bones.set(`${side}LowerArm`, { node: lower });
      hands[side] = hand;
      shoulders[side] = upper;
    }
    torso.updateMatrixWorld(true);
    const offsets = pointAtOffsets(target, bones);
    assert.ok(`${near}UpperArm` in offsets);
    assert.ok(!(`${far}UpperArm` in offsets));
    const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
      bones, gestureRotation: new THREE.Quaternion(), gestureOffsets: offsets,
      gestureState: { from: {}, to: offsets, started: 0, releaseAt: Infinity, releasing: false },
    });
    runtime.updateGesture(1000);
    torso.updateMatrixWorld(true);
    const nearPosition = hands[near].getWorldPosition(new THREE.Vector3());
    const farPosition = hands[far].getWorldPosition(new THREE.Vector3());
    const shoulderPosition = shoulders[near].getWorldPosition(new THREE.Vector3());
    const handDirection = nearPosition.clone().sub(shoulderPosition).normalize();
    const targetDirection = target.clone().sub(shoulderPosition).normalize();
    assert.ok(Math.sign(nearPosition.x) === Math.sign(target.x));
    assert.ok(Math.abs(nearPosition.x - target.x) < Math.abs(farPosition.x - target.x));
    assert.ok(handDirection.angleTo(targetDirection) < 0.12);
  }
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
  const feet = [0.1, -0.1].map((x) => {
    const foot = new THREE.Bone();
    foot.position.set(x, -1, 0);
    hips.add(foot);
    return foot;
  });
  model.add(hips);
  stage.add(model);
  const clip = (name, times, angles) => new THREE.AnimationClip(name, times.at(-1), [
    new THREE.QuaternionKeyframeTrack(`${hips.uuid}.quaternion`, times, angles.flatMap((angle) => new THREE.Quaternion().setFromEuler(new THREE.Euler(angle, 0, 0)).toArray())),
    new THREE.VectorKeyframeTrack(`${hips.uuid}.position`, times, times.flatMap(() => [0, 1, 0])),
  ]);
  const clips = new Map([
    ['idle', clip('idle', [0, 0.35, 1], [0.12, 0.04, 0.12])],
    ['sit-idle', clip('sit-idle', [0, 0.6, 1], [1.42, 1.32, 1.42])],
    ['sit', clip('sit', [0, 0.5, 0.8], [0, 1.3, 1.3])],
    ['stand', clip('stand', [0, 0.5, 0.8], [1.3, 0, 0])],
  ]);
  for (const [name, clip] of clips) {
    clip.userData.action = name;
    clip.userData.poseTracks = clip.tracks.map((track) => ({ name: track.name, valueSize: track.getValueSize(), interpolant: track.createInterpolant() }));
  }
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    stage,
    vrm: { scene: model, humanoid: { update() {}, getRawBoneNode: (name) => ({ leftFoot: feet[0], rightFoot: feet[1] })[name] } },
    ankleHeight: 0.1,
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
  runtime.plantFeet();
  stage.updateMatrixWorld(true);
  model.getWorldPosition(previousModel);
  head.getWorldPosition(previousHead);
  for (const pose of ['sit', 'stand']) {
    runtime.pose(pose);
    for (let frame = 0; frame < 60; frame++) {
      runtime.mixer.update(1 / 60);
      runtime.updatePose(frame * 1000 / 60);
      runtime.plantFeet();
      sample();
      if (frame >= 30) handoverVelocities.push(velocities.at(-1));
    }
  }
  assert.equal(stage.position.y, 0);
  for (const part of ['model', 'head']) {
    const peak = Math.max(...handoverVelocities.map((velocity) => velocity[part]));
    assert.ok(peak < 0.3, `peak ${part} velocity ${peak}`);
  }
  assert.ok(runtime.handovers.get('sit:sit-idle').offset > 0.3, JSON.stringify(runtime.handovers.get('sit:sit-idle')));
  assert.ok(runtime.handovers.get('stand:idle').offset > 0.3, JSON.stringify(runtime.handovers.get('stand:idle')));
  assert.ok(runtime.handovers.get('sit:sit-idle').duration > 1.5);
  assert.ok(runtime.handovers.get('stand:idle').duration > runtime.handovers.get('sit:sit-idle').duration);
});

test('motions bind to each puppet by bone, mirroring VRM 0 and scaling hips to its height', async () => {
  const motion = { name: 'Idle', duration: 1, hipsHeight: 100, tracks: [
    { name: 'hips.quaternion', times: [0, 1], values: [0.1, 0.2, 0.3, 0.9, 0, 0, 0, 1] },
    { name: 'hips.position', times: [0], values: [10, 100, 20] },
    { name: 'leftToes.quaternion', times: [0], values: [0, 0, 0, 1] },
  ] };
  const hips = new THREE.Bone();
  hips.name = 'Normalized_Hips';
  const puppet = (metaVersion, height) => ({ meta: { metaVersion }, humanoid: { getNormalizedBoneNode: (name) => name === 'hips' ? hips : null, normalizedRestPose: { hips: { position: [0.5, height, 0.25] } } } });
  const modern = await animationClip(motion, 'motion', puppet('1', 0.9));
  assert.deepEqual(modern.tracks.map((track) => track.name), ['Normalized_Hips.quaternion', 'Normalized_Hips.position']);
  assert.deepEqual(Array.from(modern.tracks[0].values), [0.1, 0.2, 0.3, 0.9, 0, 0, 0, 1].map(Math.fround));
  assert.deepEqual(Array.from(modern.tracks[1].values), [10 * 0.009, 0.9, 20 * 0.009].map(Math.fround));
  assert.equal(modern.userData.poseTracks[0].valueSize, 4);
  const legacy = await animationClip(motion, 'motion', puppet('0', 1.2));
  assert.deepEqual(Array.from(legacy.tracks[0].values), [-0.1, 0.2, -0.3, 0.9, -0, 0, -0, 1].map(Math.fround));
  assert.deepEqual(Array.from(legacy.tracks[1].values), [-10 * 0.012, 1.2, -20 * 0.012].map(Math.fround));
  const stray = { ...motion, tracks: [...motion.tracks, { name: 'rightHand.position', times: [0], values: [1, 2, 3] }] };
  await assert.rejects(animationClip(stray, 'motion', puppet('1', 0.9)), /invalid motion track/);
  await assert.rejects(animationClip({ ...motion, hipsHeight: 0 }, 'motion', puppet('1', 0.9)), /no source hips height/);
});

test('every loaded puppet shows its first clip at full weight on its first frame, and telemetry records only changed weights and hip height', () => {
  const events = [];
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    mixer: new THREE.AnimationMixer(new THREE.Group()),
    handovers: new Map([['idle:sit', { offset: 0, duration: 0.36 }]]),
    onAnimation: (state) => events.push(state),
  });
  const showPuppet = (height) => {
    const scene = new THREE.Group();
    const hips = new THREE.Bone();
    scene.position.y = height;
    scene.add(hips);
    const clip = (name, y) => {
      const value = new THREE.AnimationClip(name, 1, [new THREE.NumberKeyframeTrack(`${hips.uuid}.position[y]`, [0], [y])]);
      value.userData.action = name;
      return [name, value];
    };
    Object.assign(runtime, { vrm: { scene, humanoid: { getRawBoneNode: () => hips } }, clips: new Map([clip('idle', 1), clip('sit', 0)]) });
    runtime.playClip('idle', 'idle');
    runtime.mixer.update(1 / 60);
    runtime.recordAnimation();
  };
  showPuppet(2);
  assert.deepEqual(events[0], { clips: [{ name: 'idle', weight: 1 }], hip_height: 3 });
  for (let frame = 0; frame < 60; frame++) {
    runtime.mixer.update(1 / 60);
    runtime.recordAnimation();
  }
  assert.equal(events.length, 1);
  runtime.playClip('sit', 'sit');
  runtime.mixer.update(0.18);
  runtime.recordAnimation();
  assert.deepEqual(events[1], { clips: [{ name: 'idle', weight: 0.5 }, { name: 'sit', weight: 0.5 }], hip_height: 2.5 });
  runtime.mixer.update(0.2);
  runtime.playClip('sit', 'sit');
  runtime.mixer.update(1 / 60);
  runtime.recordAnimation();
  assert.deepEqual(events[2], { clips: [{ name: 'sit', weight: 1 }], hip_height: 2 }, 'replaying the only visible clip must not blend in the rest pose');
  showPuppet(4);
  assert.deepEqual(events[3], { clips: [{ name: 'idle', weight: 1 }], hip_height: 5 });
});

test('expired idle dwell cannot interrupt a clip gesture or retain its gesture marker', () => {
  const scene = new THREE.Group();
  const clip = (name) => new THREE.AnimationClip(name, 1, [new THREE.NumberKeyframeTrack('.position[y]', [0], [1])]);
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm: { scene }, mixer: new THREE.AnimationMixer(scene),
    clips: new Map(['idle', 'idle-2', 'nod'].map((name) => [name, clip(name)])),
    poseName: 'stand', idleClip: 'idle', nextIdleAt: 0,
  });
  runtime.playClip('idle', 'idle');
  runtime.mixer.update(0.2);
  runtime.gesture('nod');
  runtime.mixer.update(0.2);
  runtime.updatePose(1000);
  assert.equal(runtime.clipAction.getClip().name, 'nod', 'expired idle dwell must not replace a running nod');
  assert.equal(runtime.clipGesture, 'nod');
  runtime.mixer.update(1);
  runtime.updatePose(2000);
  assert.equal(runtime.clipGesture, null, 'completed nod must release its gesture marker');
  assert.equal(runtime.clipAction.getClip().name, 'idle-2');
  runtime.nextIdleAt = 2000;
  runtime.updatePose(2001);
  assert.equal(runtime.clipAction.getClip().name, 'idle', 'actual idle clips must still rotate');
});

test('interrupting a long seated fade removes every seated contribution by the new fade deadline', () => {
  const scene = new THREE.Group();
  scene.position.y = 1;
  const clip = (name, height) => {
    const value = new THREE.AnimationClip(name, 1, [new THREE.NumberKeyframeTrack('.position[y]', [0], [height])]);
    value.userData.action = name;
    return value;
  };
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm: { scene }, mixer: new THREE.AnimationMixer(scene),
    clips: new Map([['sit', clip('sit', 0.5)], ['sit-idle', clip('sit-idle', 0.5)], ['stand', clip('stand', 1)]]),
    handovers: new Map([['sit:sit-idle', { duration: 2, offset: 0 }]]), poseName: 'sit',
  });
  runtime.playClip('sit', 'sit');
  runtime.mixer.update(0.3);
  const seated = runtime.clipAction;
  runtime.playClip('sit-idle', 'sit-idle');
  runtime.mixer.update(0.1);
  const seatedIdle = runtime.clipAction;
  const before = [seated.getEffectiveWeight(), seatedIdle.getEffectiveWeight()];
  runtime.pose('listen');
  runtime.mixer.update(0.001);
  const interruptedWeight = seatedIdle.getEffectiveWeight();
  runtime.mixer.update(0.2);
  assert.equal(seated.getEffectiveWeight(), 0, 'older seated action must finish fading at the new deadline');
  assert.equal(seatedIdle.getEffectiveWeight(), 0);
  assert.ok(interruptedWeight <= before[1], 'interrupting a fade must not increase an outgoing action weight');
  assert.equal(runtime.poseName, 'listen');
  assert.ok(Math.abs(scene.position.y - 1) < 1e-6, 'rendered height must contain no remaining seated contribution');
  runtime.pose('sit');
  runtime.mixer.update(0.2);
  assert.equal(runtime.clipAction.getEffectiveWeight(), 1, 'reused actions must recover full weight');
  assert.ok(Math.abs(scene.position.y - 0.5) < 1e-6);
});

test('a posture already held never replays or restarts a clip, so the hips hold their idle', async () => {
  const model = new THREE.Group();
  const hips = new THREE.Bone();
  hips.position.y = 1.6;
  model.add(hips);
  const clip = (name, times, heights) => {
    const value = new THREE.AnimationClip(name, times.at(-1), [new THREE.VectorKeyframeTrack(`${hips.uuid}.position`, times, heights.flatMap((height) => [0, height, 0]))]);
    value.userData.action = name;
    return [name, value];
  };
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm: { scene: model }, mixer: new THREE.AnimationMixer(model), handovers: new Map(), poseName: 'sit',
    clips: new Map([clip('idle', [0, 1], [1.4, 1.4]), clip('sit-idle', [0, 1], [0.75, 0.75]), clip('sit', [0, 0.5, 0.8], [1.4, 0.75, 0.75]), clip('stand', [0, 0.5, 0.8], [0.75, 1.4, 1.4])]),
  });
  const hipHeights = (seconds) => Array.from({ length: seconds * 60 }, (_, frame) => {
    runtime.mixer.update(1 / 60);
    runtime.updatePose(frame * 1000 / 60);
    return hips.position.y;
  });
  runtime.playClip('idle', 'idle');
  await runtime.loadClips([]);
  assert.equal(runtime.clipAction.getClip().name, 'sit', 'a seated puppet sits down from the standing idle it loads in');
  hipHeights(3);
  runtime.pose('stand');
  hipHeights(3);
  for (const [request, repeat] of [['stand', () => runtime.pose('stand')], ['listen', () => runtime.pose('listen')], ['clip reload', () => runtime.loadClips([])]]) {
    repeat();
    assert.ok(hipHeights(2).every((height) => Math.abs(height - 1.4) < 0.01), `${request} while standing must hold the standing idle`);
  }
  runtime.pose('sit');
  hipHeights(3);
  runtime.pose('sit');
  assert.ok(hipHeights(2).every((height) => Math.abs(height - 0.75) < 0.01), 'sit while seated must hold the seated idle');
});

test('asleep holds the eyes shut under a sleepy droop and waking reopens them and clears the droop', () => {
  const head = new THREE.Object3D();
  const values = {};
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    moodName: null, moodFrom: {}, moodBones: {}, moodStarted: 0, sleeping: false, nextBlink: Infinity, blinkStart: 0,
    mouthValues: { aa: 0, ih: 0, ou: 0, ee: 0, oh: 0 }, audio: null,
    bones: new Map([['head', { node: head }]]), gestureRotation: new THREE.Quaternion(),
    vrm: { expressionManager: { expressions: [], setValue: (name, value) => { values[name] = value; } } },
  });
  runtime.asleep(true);
  runtime.moodStarted = 0;
  for (const now of [1000, 9000]) {
    head.quaternion.identity();
    runtime.nextBlink = now - 1;
    runtime.updateFace(now);
    assert.equal(values.blink, 1);
  }
  assert.equal(runtime.moodName, 'sleepy');
  assert.ok(head.quaternion.angleTo(new THREE.Quaternion()) > 0.1);
  runtime.asleep(false);
  runtime.nextBlink = Infinity;
  runtime.moodStarted = 840;
  head.quaternion.identity();
  runtime.updateFace(1000);
  assert.ok(head.quaternion.angleTo(new THREE.Quaternion()) > 0.01);
  runtime.moodStarted = 0;
  head.quaternion.identity();
  runtime.updateFace(1000);
  assert.equal(values.blink, 0);
  assert.equal(runtime.moodName, null);
  assert.ok(head.quaternion.angleTo(new THREE.Quaternion()) < 1e-6);
});

for (const name of ['surprised', 'Surprised', 'SURPRISED']) test(`mood actions reach ${name} and neutral clears weights and pose`, async () => {
  const { VRMExpression, VRMExpressionManager, VRMExpressionMorphTargetBind } = await import('@pixiv/three-vrm');
  const manager = new VRMExpressionManager();
  const mesh = new THREE.Mesh();
  mesh.morphTargetInfluences = [0, 0];
  for (const [index, expressionName] of [name, 'angry'].entries()) {
    const expression = new VRMExpression(expressionName);
    expression.addBind(new VRMExpressionMorphTargetBind({ primitives: [mesh], index, weight: 1 }));
    manager.registerExpression(expression);
  }
  const head = new THREE.Object3D();
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    moodName: null, moodFrom: {}, moodBones: {}, moodStarted: 0,
    bones: new Map([['head', { node: head }]]), gestureRotation: new THREE.Quaternion(),
  });
  const settle = () => {
    for (let i = 0; i < 120; i++) {
      head.quaternion.identity();
      runtime.updateMood(performance.now() + 1000, manager);
      manager.update();
    }
  };
  runtime.mood('surprised');
  settle();
  assert.ok(mesh.morphTargetInfluences[0] > 0.89, `${name} must move the face`);
  runtime.mood('angry');
  settle();
  assert.ok(mesh.morphTargetInfluences[1] > 0.77);
  assert.ok(head.quaternion.angleTo(new THREE.Quaternion()) > 0.01);
  runtime.mood('neutral');
  settle();
  assert.ok(mesh.morphTargetInfluences.every(value => value < 1e-6));
  assert.ok(head.quaternion.angleTo(new THREE.Quaternion()) < 1e-6);
  assert.throws(() => runtime.mood('unknown'), /unknown mood/);
});

test('the feet anchor the puppet through sitting, gestures and standing while the hips move', async () => {
  const scene = new THREE.Group();
  scene.scale.setScalar(1.7);
  new THREE.Group().add(scene);
  const bone = (name, parent, x, y) => {
    const node = new THREE.Bone();
    node.name = name;
    node.position.set(x, y, 0);
    parent.add(node);
    return node;
  };
  const hips = bone('Hips', scene, 0.3, 1);
  const legs = ['Left', 'Right'].map((side, index) => {
    const upper = bone(`${side}UpperLeg`, hips, index ? -0.1 : 0.1, 0);
    const lower = bone(`${side}LowerLeg`, upper, 0, -0.45);
    return { upper, lower, foot: bone(`${side}Foot`, lower, 0, -0.45) };
  });
  const nodes = { hips, leftFoot: legs[0].foot, rightFoot: legs[1].foot, leftUpperLeg: legs[0].upper, rightUpperLeg: legs[1].upper, leftLowerLeg: legs[0].lower, rightLowerLeg: legs[1].lower };
  const node = (name) => nodes[name] ?? null;
  const vrm = { scene, meta: { metaVersion: '1' }, humanoid: { update() {}, getRawBoneNode: node, getNormalizedBoneNode: node, normalizedRestPose: { hips: { position: hips.position.toArray() } } } };
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm, ankleHeight: 0.1, mixer: new THREE.AnimationMixer(scene), clips: new Map(), handovers: new Map(),
    bones: new Map(), poseName: 'stand', idleClip: 'idle', nextIdleAt: Infinity,
  });
  const bend = (angle, sign) => new THREE.Quaternion().setFromEuler(new THREE.Euler(sign * angle, 0, 0)).toArray();
  const payload = (name, heights, offset, angles) => ({
    name, duration: 1, hipsHeight: 1, tracks: [
      { name: 'hips.position', times: [0, 0.5, 1], values: heights.flatMap((height, i) => [offset + i * 0.2, height, offset - i * 0.3]) },
      ...['left', 'right'].flatMap((side) => [
        { name: `${side}UpperLeg.quaternion`, times: [0, 1], values: angles.flatMap((angle) => bend(angle, -1)) },
        { name: `${side}LowerLeg.quaternion`, times: [0, 1], values: angles.flatMap((angle) => bend(angle, 1)) },
      ]),
    ],
  });
  const entries = [
    ['idle', [1, 1.02, 1], 0.1, [0, 0.05]], ['sit', [1, 0.7, 0.5], 0.4, [0, 1.5]],
    ['sit-idle', [0.5, 0.52, 0.5], -0.2, [1.5, 1.45]], ['stand', [0.5, 0.8, 1], -0.5, [1.5, 0]],
  ].map(([action, heights, offset, angles]) => ({ action, format: 'motion', data: payload(action, heights, offset, angles) }));
  await runtime.loadClips(entries);
  const left = new THREE.Vector3();
  const right = new THREE.Vector3();
  const hip = new THREE.Vector3();
  const hipsSeen = [];
  let time = 0;
  const advance = (seconds) => {
    for (let frame = 0; frame < seconds * 60; frame++) {
      time += 1 / 60;
      runtime.updateBasePose(1 / 60);
      runtime.updatePose(time * 1000);
      runtime.plantFeet();
      legs[0].foot.getWorldPosition(left);
      legs[1].foot.getWorldPosition(right);
      assert.ok(Math.abs(left.x + right.x) < 1e-6, `feet X drift at ${time}`);
      assert.ok(Math.abs(left.z + right.z) < 1e-6, `feet Z drift at ${time}`);
      assert.ok(Math.abs(Math.min(left.y, right.y) - 0.1) < 1e-6, `feet height drift at ${time}`);
      hipsSeen.push(hips.getWorldPosition(hip).clone());
    }
  };
  runtime.playClip('idle', 'idle');
  advance(1);
  runtime.pose('sit');
  advance(7);
  await runtime.loadClips([{ action: 'clap', format: 'motion', data: payload('clap', [0.5, 0.55, 0.5], 0.8, [1.5, 1.5]) }]);
  assert.equal(runtime.clipStance('clap'), 'sit');
  runtime.gesture('clap');
  assert.equal(runtime.poseName, 'sit');
  advance(4);
  runtime.pose('stand');
  advance(5);
  const spread = (axis) => Math.max(...hipsSeen.map((point) => point[axis])) - Math.min(...hipsSeen.map((point) => point[axis]));
  assert.ok(spread('y') > 0.6, `hips height spread ${spread('y')}`);
  assert.ok(spread('z') > 0.8, `hips depth spread ${spread('z')}`);
});

function gestureRuntime() {
  const scene = new THREE.Group();
  scene.add(new THREE.Mesh(new THREE.BoxGeometry(1, 2, 1)));
  const vrm = {
    scene, meta: { metaVersion: '1' }, update() {},
    humanoid: { setNormalizedPose() {}, getNormalizedBoneNode: () => null, getRawBoneNode: () => scene },
  };
  const clips = new Map(['idle', 'idle-2', 'sit-idle', 'sit', 'stand', 'wave', 'nod'].map((name) => {
    const clip = new THREE.AnimationClip(name, 1, [new THREE.VectorKeyframeTrack('.position', [0, 1], [0, 0, 0, 0, 0, 0])]);
    clip.userData.action = name;
    return [name, clip];
  }));
  return Object.assign(Object.create(PuppetRuntime.prototype), {
    vrm, clips, mixer: new THREE.AnimationMixer(new THREE.Group()), idleRoot: new THREE.Group(),
    bones: new Map(), handovers: new Map(), poseName: 'stand', idleClip: null,
    renderer: { render() {}, getContext: () => ({ finish() {} }) },
    nextIdleAt: Infinity, clipGesture: null, pendingGesture: null, gestureState: null, gestureOffsets: {},
  });
}

function advanceGesture(runtime, seconds) {
  for (let frame = 0; frame < seconds * 60; frame++) {
    runtime.mixer.update(1 / 60);
    runtime.updatePose(runtime.mixer.time * 1000);
  }
}

test('the paintOff pose cancels a queued standing gesture before the stand completes', () => {
  for (const cancel of [false, true]) {
    const runtime = gestureRuntime();
    runtime.poseName = 'sit';
    runtime.playIdle(0);
    const fired = [];
    const playClip = runtime.playClip.bind(runtime);
    runtime.playClip = (name, fallback) => { fired.push(name); playClip(name, fallback); };
    runtime.gesture('wave');
    assert.equal(runtime.clipAction.getClip().name, 'stand');
    advanceGesture(runtime, 0.2);
    if (cancel) runtime.pose('sit');
    advanceGesture(runtime, 3);
    assert.equal(fired.includes('wave'), !cancel);
    assert.equal(runtime.pendingGesture, null);
    assert.equal(runtime.poseName, cancel ? 'sit' : 'stand');
  }
});

test('model reload drops clip and queued gestures and resumes idle rotation', async (t) => {
  const runtime = gestureRuntime();
  const replacement = gestureRuntime();
  t.mock.method(GLTFLoader.prototype, 'loadAsync', async () => ({ userData: { vrm: replacement.vrm } }));
  for (const queued of [false, true]) {
    runtime.poseName = queued ? 'sit' : 'stand';
    runtime.playIdle(0);
    runtime.gesture(queued ? 'wave' : 'nod');
    advanceGesture(runtime, 0.2);
    assert.ok(queued ? runtime.pendingGesture : runtime.clipGesture);
    await runtime.load(new Uint8Array());
    runtime.clips = new Map(replacement.clips);
    await runtime.loadClips([]);
    const fired = [];
    const playClip = runtime.playClip.bind(runtime);
    runtime.playClip = (name, fallback) => { fired.push(name); playClip(name, fallback); };
    advanceGesture(runtime, 3);
    assert.equal(runtime.clipGesture, null);
    assert.equal(runtime.pendingGesture, null);
    assert.ok(!fired.includes('nod') && !fired.includes('wave'));
    const idle = runtime.clipAction.getClip().name;
    runtime.updatePose(runtime.nextIdleAt + 1);
    assert.notEqual(runtime.clipAction.getClip().name, idle);
    assert.ok(Number.isFinite(runtime.nextIdleAt));
    runtime.gesture('beat');
    assert.equal(runtime.gestureState.name, 'beat');
  }
});
