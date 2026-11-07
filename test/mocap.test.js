import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import * as THREE from 'three';
import { install, motion } from '../scripts/mocap.mjs';

const BONES = [
  ['lhipjoint', 'root', [1, -0.4, 0], 1, []], ['lfemur', 'lhipjoint', [0.34202, -0.939693, 0], 7, ['rx', 'ry', 'rz']],
  ['ltibia', 'lfemur', [0.34202, -0.939693, 0], 7, ['rx']], ['lfoot', 'ltibia', [0, -0.1, 1], 1.5, ['rx', 'rz']], ['ltoes', 'lfoot', [0, 0, 1], 0.7, ['rx']],
  ['rhipjoint', 'root', [-1, -0.4, 0], 1, []], ['rfemur', 'rhipjoint', [-0.34202, -0.939693, 0], 7, ['rx', 'ry', 'rz']],
  ['rtibia', 'rfemur', [-0.34202, -0.939693, 0], 7, ['rx']], ['rfoot', 'rtibia', [0, -0.1, 1], 1.5, ['rx', 'rz']], ['rtoes', 'rfoot', [0, 0, 1], 0.7, ['rx']],
  ['lowerback', 'root', [0, 1, 0], 2, ['rx', 'ry', 'rz']], ['upperback', 'lowerback', [0, 1, 0], 2, ['rx', 'ry', 'rz']],
  ['thorax', 'upperback', [0, 1, 0], 2, ['rx', 'ry', 'rz']], ['lowerneck', 'thorax', [0, 1, 0], 1.5, ['rx', 'ry', 'rz']],
  ['upperneck', 'lowerneck', [0, 1, 0], 1.5, ['rx', 'ry', 'rz']], ['head', 'upperneck', [0, 1, 0], 1.5, ['rx', 'ry', 'rz']],
  ['lclavicle', 'thorax', [0.92, 0.38, 0.05], 3.6, ['ry', 'rz']], ['lhumerus', 'lclavicle', [1, 0, 0], 4.3, ['rx', 'ry', 'rz']],
  ['lradius', 'lhumerus', [1, 0, 0], 3.3, ['rx']], ['lwrist', 'lradius', [1, 0, 0], 1.6, ['ry']], ['lhand', 'lwrist', [1, 0, 0], 0.4, ['rx', 'rz']],
  ['rclavicle', 'thorax', [-0.92, 0.38, 0.05], 3.6, ['ry', 'rz']], ['rhumerus', 'rclavicle', [-1, 0, 0], 4.3, ['rx', 'ry', 'rz']],
  ['rradius', 'rhumerus', [-1, 0, 0], 3.3, ['rx']], ['rwrist', 'rradius', [-1, 0, 0], 1.6, ['ry']], ['rhand', 'rwrist', [-1, 0, 0], 0.4, ['rx', 'rz']],
];

function asf() {
  const data = BONES.map(([name, , direction, length, dof]) => `begin\nname ${name}\ndirection ${direction.join(' ')}\nlength ${length}\naxis 0 0 ${name.endsWith('humerus') ? 30 : 0} XYZ\n${dof.length ? `dof ${dof.join(' ')}` : ''}\nend`);
  const children = new Map();
  for (const [name, parent] of BONES) children.set(parent, [...(children.get(parent) ?? []), name]);
  const hierarchy = [...children].map(([parent, names]) => `${parent} ${names.join(' ')}`);
  return `:units\n  length 0.45\n:root\n   order TX TY TZ RX RY RZ\n   axis XYZ\n:bonedata\n${data.join('\n')}\n:hierarchy\nbegin\n${hierarchy.join('\n')}\nend\n`;
}

function amc(frames) {
  return `:FULLY-SPECIFIED\n:DEGREES\n${frames.map((bones, index) => `${index + 1}\n${Object.entries(bones).map(([name, values]) => `${name} ${values.join(' ')}`).join('\n')}`).join('\n')}\n`;
}

const STANDING = { lfemur: [0, 0, -20], rfemur: [0, 0, 20] };
const worlds = (result, at) => {
  const parents = { spine: 'hips', chest: 'spine', upperChest: 'chest', neck: 'upperChest', head: 'neck', leftShoulder: 'upperChest', leftUpperArm: 'leftShoulder', leftLowerArm: 'leftUpperArm', leftUpperLeg: 'hips', leftLowerLeg: 'leftUpperLeg' };
  const local = (bone) => new THREE.Quaternion().fromArray(result.tracks.find((track) => track.name === `${bone}.quaternion`).values, at * 4);
  const world = (bone) => parents[bone] ? world(parents[bone]).multiply(local(bone)) : local(bone);
  return world;
};

test('a capture retargets onto the T-pose: each bone points where the subject pointed it, the figure facing +Z', () => {
  const frames = Array.from({ length: 121 }, () => ({ root: [5, 15.8, -3, 0, 90, 0], ...STANDING, lhumerus: [0, 0, -50], lclavicle: [0, 0] }));
  const result = motion(asf(), amc(frames), { name: 'test', rate: 60 });
  const world = worlds(result, 0);
  const near = (vector, expected, message) => assert.ok(vector.angleTo(new THREE.Vector3(...expected).normalize()) < 1e-3, `${message}: ${vector.toArray()}`);
  near(new THREE.Vector3(0, 0, 1).applyQuaternion(world('hips')), [0, 0, 1], 'a subject facing +X is turned to face +Z');
  near(new THREE.Vector3(0, -1, 0).applyQuaternion(world('leftUpperLeg')), [0, -1, 0], 'a leg the subject stands straight stands straight, whatever its rest splay');
  near(new THREE.Vector3(0, 1, 0).applyQuaternion(world('head')), [0, 1, 0], 'the neck chain stays upright under the turn');
  const arm = new THREE.Vector3(1, 0, 0).applyQuaternion(world('leftUpperArm'));
  assert.ok(arm.y < -0.5, `an arm lowered about its own axis points down: ${arm.toArray()}`);
  assert.equal(result.duration, 2);
  assert.equal(result.tracks.find((track) => track.name === 'hips.quaternion').times.length, 61);
  const hips = result.tracks.find((track) => track.name === 'hips.position').values;
  assert.deepEqual([hips[0], hips[2]], [0, 0], 'the clip starts at the origin');
  assert.ok(Math.abs(hips[1] - result.hipsHeight) < 0.3, `a subject standing straight carries the hips at their rest height: ${hips[1]} vs ${result.hipsHeight}`);
});

test('an idle loops: its last frame returns to its first', () => {
  const frames = Array.from({ length: 121 }, (_, index) => ({ root: [0, 15.8, 0, 0, 0, 0], ...STANDING, lowerback: [index / 4, 0, 0] }));
  const result = motion(asf(), amc(frames), { name: 'idle-9', rate: 60, loop: true });
  const spine = result.tracks.find((track) => track.name === 'spine.quaternion').values;
  const first = new THREE.Quaternion().fromArray(spine, 0);
  const last = new THREE.Quaternion().fromArray(spine, spine.length - 4);
  assert.ok(first.angleTo(last) < 1e-4);
  const middle = new THREE.Quaternion().fromArray(spine, 30 * 4);
  assert.ok(first.angleTo(middle) > 0.1, 'away from the seam the capture plays as recorded');
});

test('a window shorter than a second is refused', () => {
  assert.throws(() => motion(asf(), amc(Array.from({ length: 30 }, () => ({ root: [0, 15.8, 0, 0, 0, 0] }))), { name: 'short', rate: 60 }), /shorter than a second/);
});

test('a capture whose feet leap for one sample and back is refused, since the overlay stands the figure on its feet each frame', () => {
  const frames = Array.from({ length: 121 }, (_, index) => ({ root: [0, 15.8, 0, 0, 0, 0], ...STANDING, rfemur: index === 60 || index === 61 ? [40, 0, 20] : [0, 0, 20] }));
  assert.throws(() => motion(asf(), amc(frames), { name: 'glitch', rate: 60 }), /glitch: its feet jump [\d.]+% of hip height out and back at 1\.00 s of the take/);
  const steady = Array.from({ length: 121 }, (_, index) => ({ root: [0, 15.8, 0, 0, 0, 0], ...STANDING, rfemur: [40 * index / 120, 0, 20] }));
  assert.doesNotThrow(() => motion(asf(), amc(steady), { name: 'swing', rate: 60 }), 'a leg swinging steadily is no leap');
});

function glb(doc) {
  const text = Buffer.from(JSON.stringify(doc).padEnd(Math.ceil(JSON.stringify(doc).length / 4) * 4, ' '));
  const header = Buffer.alloc(20);
  header.write('glTF', 0);
  header.writeUInt32LE(2, 4);
  header.writeUInt32LE(20 + text.length, 8);
  header.writeUInt32LE(text.length, 12);
  header.write('JSON', 16);
  return Buffer.concat([header, text]);
}

const spreadOf = (file) => {
  const bytes = fs.readFileSync(file);
  return JSON.parse(bytes.subarray(20, 20 + bytes.readUInt32LE(12))).extensions?.VOICE_arms;
};

test('a clip write installs its clips and every catalog figure re-measured against them together, or nothing', (t) => {
  const root = fs.mkdtempSync(path.join(process.env.TMPDIR ?? os.tmpdir(), 'mocap-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const [clips, catalog] = ['clips', 'catalog'].map((name) => path.join(root, name));
  fs.mkdirSync(clips);
  fs.mkdirSync(path.join(catalog, 'nested.vrm'), { recursive: true });
  fs.writeFileSync(path.join(clips, 'clips.manifest.json'), JSON.stringify({ clips: [{ action: 'wave', name: 'wave.fbx', source: 'elsewhere' }] }));
  fs.writeFileSync(path.join(clips, 'wave.fbx'), 'wave');
  const figures = ['a.vrm', 'b.vrm'].map((name) => path.join(catalog, name));
  for (const figure of [...figures, path.join(catalog, '.hidden.vrm')]) fs.writeFileSync(figure, glb({ asset: { version: '2.0' }, nodes: [{ name: 'Root' }] }));
  const voiceVr = path.join(root, 'voice-vr');
  const calls = path.join(root, 'calls');
  const measuring = (refused = '') => `#!/bin/sh\n[ "$2" = --clips ] || exit 2\necho "$4" >> '${calls}'\ncase "$4" in *'${refused || '-'}') exit 1;; esac\nprintf '{"angle":0.1,"body":"b","clips":"%s","revision":1}' "$(cat "$3"/*.motion "$3"/*.fbx | sha256sum | cut -c1-64)"\n`;
  const served = () => execFileSync('sh', ['-c', `cat '${clips}'/*.motion '${clips}'/*.fbx | sha256sum | cut -c1-64`], { encoding: 'utf8' }).trim();
  const measured = () => fs.existsSync(calls) ? fs.readFileSync(calls, 'utf8').split('\n').filter(Boolean).map((file) => path.basename(file)) : [];
  const state = () => [clips, catalog].map((dir) => fs.readdirSync(dir).sort().map((name) => [name, fs.statSync(path.join(dir, name)).isFile() ? fs.readFileSync(path.join(dir, name), 'utf8') : 'dir']));
  fs.writeFileSync(voiceVr, measuring(), { mode: 0o755 });
  const previous = process.env.VOICE_VR;
  process.env.VOICE_VR = voiceVr;
  t.after(() => { if (previous === undefined) delete process.env.VOICE_VR; else process.env.VOICE_VR = previous; });
  const idle = (name, value) => ({ entry: { action: name, name: `${name}.motion`, source: 'Capture take' }, motion: { name, value } });

  install(clips, catalog, 'Capture', [idle('idle-1', 1), idle('idle-2', 2)]);
  assert.deepEqual(measured(), ['a.vrm', 'b.vrm'], 'every catalog figure is measured, nothing else in the catalog');
  const first = figures.map(spreadOf);
  assert.ok(first.every((spread) => spread?.clips === served()), 'each figure is measured against exactly the clips the write leaves served');

  install(clips, catalog, 'Capture', [idle('idle-1', 1)]);
  assert.ok(!fs.existsSync(path.join(clips, 'idle-2.motion')), 'a dropped clip leaves the served set');
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(clips, 'clips.manifest.json'), 'utf8')).clips.map((entry) => entry.name), ['wave.fbx', 'idle-1.motion']);
  assert.equal(measured().length, 4);
  for (const spread of figures.map(spreadOf)) assert.equal(spread.clips, served(), 'dropping a clip re-measures every figure against the clips left');
  assert.notEqual(served(), first[0].clips);

  const republishing = `#!/bin/sh\ncase "$4" in *a.vrm) printf ' ' >> "$4";; esac\n` + measuring().split('\n').slice(1).join('\n');
  for (const [stub, refusal] of [[measuring('b.vrm'), /1 of 2 catalog figures could not be measured.*no clip and no figure changed: b\.vrm/], ['#!/bin/sh\nexit 1\n', /2 of 2 .*a\.vrm, b\.vrm/], [republishing, /a\.vrm changed while the write measured them/]]) {
    fs.writeFileSync(voiceVr, stub);
    const [before, a] = [state(), fs.readFileSync(figures[0])];
    assert.throws(() => install(clips, catalog, 'Capture', [idle('idle-1', 3), idle('idle-3', 4)]), refusal);
    if (stub === republishing) fs.writeFileSync(figures[0], a);
    assert.deepEqual(state(), before, 'a write that cannot re-measure every figure changes no clip, no figure and leaves nothing staged');
    for (const spread of figures.map(spreadOf)) assert.equal(spread.clips, served(), 'so the catalog still serves');
  }
  fs.writeFileSync(voiceVr, measuring());
  install(clips, catalog, 'Capture', [idle('idle-1', 3), idle('idle-3', 4)]);
  assert.ok(fs.existsSync(path.join(clips, 'idle-3.motion')));
  for (const spread of figures.map(spreadOf)) assert.equal(spread.clips, served(), 'a rerun installs the write');
});
