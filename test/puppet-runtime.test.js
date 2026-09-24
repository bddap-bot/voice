import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
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
    moodValues: Object.fromEntries(['happy', 'angry', 'sad', 'relaxed', 'surprised'].map((name) => [name, 0])),
    bones: new Map([['head', { node: bone }]]), gestureRotation: new THREE.Quaternion(),
  });
  runtime.updateMood(1000, { setValue() {} });
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

test('ready-to-play tracks decode without FBX parsing', async () => {
  const payload = new TextEncoder().encode(JSON.stringify({ name: 'Idle', duration: 1, tracks: [{ name: 'Hips.quaternion', times: [0, 1], values: [0, 0, 0, 1, 0, 0, 0, 1] }] }));
  const clip = await animationClip(payload, 'tracks', {});
  assert.equal(clip.name, 'Idle');
  assert.equal(clip.tracks[0].name, 'Hips.quaternion');
  assert.equal(clip.userData.poseTracks[0].valueSize, 4);
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
    moodValues: Object.fromEntries(['happy', 'angry', 'sad', 'relaxed', 'surprised'].map((name) => [name, 0])),
    mouthValues: { aa: 0, ih: 0, ou: 0, ee: 0, oh: 0 }, audio: null,
    bones: new Map([['head', { node: head }]]), gestureRotation: new THREE.Quaternion(),
    vrm: { expressionManager: { setValue: (name, value) => { values[name] = value; } } },
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
