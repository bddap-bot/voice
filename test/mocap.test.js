import assert from 'node:assert/strict';
import test from 'node:test';
import * as THREE from 'three';
import { motion } from '../scripts/mocap.mjs';

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
