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
  runtime.gestureState = { name: 'point_at' };
  runtime.beginGesture = (name) => { runtime.gestureState = { name }; };
  const manager = { setValue() {} };
  runtime.updateMouth(manager);
  assert.equal(runtime.gestureState.name, 'point_at');
  runtime.gestureState = null;
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

test('panel pointing can replace the waiting gesture when display material arrives', () => {
  const runtime = Object.assign(Object.create(PuppetRuntime.prototype), { waitingForHub: true, gestureState: { name: 'waiting' }, gestureOffsets: {} });
  runtime.gesture('point', 'panel');
  assert.equal(runtime.gestureState.name, 'point_at');
  assert.throws(() => runtime.gesture('beat'), /waiting/);
});

test('Mixamo FBX tracks retarget onto VRM humanoid bones', () => {
  const hips = new THREE.Bone();
  hips.name = 'NormalizedHips';
  const arm = new THREE.Bone();
  arm.name = 'NormalizedRightUpperArm';
  const nodes = { hips, rightUpperArm: arm };
  const rotation = new THREE.QuaternionKeyframeTrack('mixamorigRightArm.quaternion', [0, 1], [0, 0, 0, 1, 0.2, 0, 0, 0.98]);
  const position = new THREE.VectorKeyframeTrack('mixamorigHips.position', [0, 1], [0, 100, 0, 0, 110, 0]);
  const source = { animations: [new THREE.AnimationClip('Pointing', 1, [rotation, position])] };
  const clip = retargetMixamoClip(source, { humanoid: { getNormalizedBoneNode: (name) => nodes[name] } });
  assert.deepEqual(clip.tracks.map((track) => track.name), ['NormalizedRightUpperArm.quaternion', 'NormalizedHips.position']);
  assert.ok(Math.abs(clip.tracks[1].values[4] - 1.1) < 1e-6);
});
