import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import * as THREE from 'three';

const root = path.resolve(import.meta.dirname, '..');
const PUBLISH = path.join(root, 'scripts/publish-avatar.py');
const cache = process.env.VOICE_MOCAP_CACHE ?? path.join(root, 'node_modules/.cache/mocap');
const FPS = 30;
const SEAM = 0.5;
const STANCE_JUMP = 0.01;

const BONES = {
  root: 'hips', lowerback: 'spine', upperback: 'chest', thorax: 'upperChest', lowerneck: 'neck', head: 'head',
  lclavicle: 'leftShoulder', lhumerus: 'leftUpperArm', lradius: 'leftLowerArm', lhand: 'leftHand',
  rclavicle: 'rightShoulder', rhumerus: 'rightUpperArm', rradius: 'rightLowerArm', rhand: 'rightHand',
  lfemur: 'leftUpperLeg', ltibia: 'leftLowerLeg', lfoot: 'leftFoot',
  rfemur: 'rightUpperLeg', rtibia: 'rightLowerLeg', rfoot: 'rightFoot',
};

const PARENT = {
  spine: 'hips', chest: 'spine', upperChest: 'chest', neck: 'upperChest', head: 'neck',
  leftShoulder: 'upperChest', leftUpperArm: 'leftShoulder', leftLowerArm: 'leftUpperArm', leftHand: 'leftLowerArm',
  rightShoulder: 'upperChest', rightUpperArm: 'rightShoulder', rightLowerArm: 'rightUpperArm', rightHand: 'rightLowerArm',
  leftUpperLeg: 'hips', leftLowerLeg: 'leftUpperLeg', leftFoot: 'leftLowerLeg',
  rightUpperLeg: 'hips', rightLowerLeg: 'rightUpperLeg', rightFoot: 'rightLowerLeg',
};

// A VRM's normalized rest is a T-pose with straight limbs; each source bone's rest direction is turned onto it.
const T_POSE = {
  spine: [0, 1, 0], chest: [0, 1, 0], upperChest: [0, 1, 0], neck: [0, 1, 0], head: [0, 1, 0],
  leftShoulder: [1, 0, 0], leftUpperArm: [1, 0, 0], leftLowerArm: [1, 0, 0], leftHand: [1, 0, 0],
  rightShoulder: [-1, 0, 0], rightUpperArm: [-1, 0, 0], rightLowerArm: [-1, 0, 0], rightHand: [-1, 0, 0],
  leftUpperLeg: [0, -1, 0], leftLowerLeg: [0, -1, 0], rightUpperLeg: [0, -1, 0], rightLowerLeg: [0, -1, 0],
};

const euler = (degrees) => new THREE.Quaternion().setFromEuler(new THREE.Euler(...degrees.map(THREE.MathUtils.degToRad), 'ZYX'));

export function parseAsf(text) {
  const bones = new Map([['root', { name: 'root', direction: new THREE.Vector3(), length: 0, axis: new THREE.Quaternion(), dof: ['rx', 'ry', 'rz'], children: [] }]]);
  const lines = text.split('\n').map((line) => line.trim());
  const section = (name) => lines.findIndex((line) => line.startsWith(name));
  const rootOrder = lines.find((line) => line.startsWith('order'))?.split(/\s+/).slice(1).map((dof) => dof.toLowerCase());
  if (rootOrder) bones.get('root').order = rootOrder;
  let bone = null;
  for (const line of lines.slice(section(':bonedata'), section(':hierarchy'))) {
    const [key, ...rest] = line.split(/\s+/);
    if (key === 'begin') bone = { dof: [], children: [] };
    else if (key === 'end') bones.set(bone.name, bone);
    else if (key === 'name') bone.name = rest[0];
    else if (key === 'direction') bone.direction = new THREE.Vector3(...rest.map(Number));
    else if (key === 'length') bone.length = Number(rest[0]);
    else if (key === 'axis') {
      if (rest[3] !== 'XYZ') throw new Error(`${bone.name}: axis order ${rest[3]}`);
      bone.axis = euler(rest.slice(0, 3).map(Number));
    } else if (key === 'dof') bone.dof = rest;
  }
  for (const line of lines.slice(section(':hierarchy') + 1)) {
    if (line === 'begin' || line === 'end' || !line) continue;
    const [parent, ...children] = line.split(/\s+/);
    for (const child of children) {
      bones.get(parent).children.push(child);
      bones.get(child).parent = parent;
    }
  }
  return bones;
}

export function parseAmc(text) {
  const frames = [];
  for (const line of text.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#') || trimmed.startsWith(':')) continue;
    if (/^\d+$/.test(trimmed)) frames.push(new Map());
    else {
      const [name, ...values] = trimmed.split(/\s+/);
      frames.at(-1).set(name, values.map(Number));
    }
  }
  return frames;
}

function pose(bones, frame) {
  const world = new Map();
  const start = new Map();
  const end = new Map();
  const visit = (name) => {
    const bone = bones.get(name);
    const values = frame.get(name) ?? [];
    let local;
    if (name === 'root') {
      const channel = (dof) => values[bone.order.indexOf(dof)] ?? 0;
      local = euler(['rx', 'ry', 'rz'].map(channel));
      const at = new THREE.Vector3(channel('tx'), channel('ty'), channel('tz'));
      world.set(name, local);
      start.set(name, at);
      end.set(name, at.clone());
    } else {
      const angles = ['rx', 'ry', 'rz'].map((dof) => { const index = bone.dof.indexOf(dof); return index < 0 ? 0 : values[index] ?? 0; });
      local = bone.axis.clone().multiply(euler(angles)).multiply(bone.axis.clone().invert());
      const rotation = world.get(bone.parent).clone().multiply(local);
      world.set(name, rotation);
      start.set(name, end.get(bone.parent).clone());
      end.set(name, start.get(name).clone().add(bone.direction.clone().applyQuaternion(rotation).multiplyScalar(bone.length)));
    }
    for (const child of bone.children) visit(child);
  };
  visit('root');
  return { world, start, end };
}

function restAlignment(bones) {
  const align = new Map([['hips', new THREE.Quaternion()]]);
  for (const [source, target] of Object.entries(BONES)) {
    if (source === 'root') continue;
    const direction = T_POSE[target];
    align.set(target, direction ? new THREE.Quaternion().setFromUnitVectors(new THREE.Vector3(...direction), bones.get(source).direction.clone().normalize()) : new THREE.Quaternion());
  }
  return align;
}

function sample(frames, rate, time) {
  const at = time * rate;
  const index = Math.min(frames.length - 2, Math.floor(at));
  return [frames[index], frames[index + 1], at - index];
}

export function motion(asf, amc, { name, rate, from = 0, to = Infinity, loop = false }) {
  const bones = parseAsf(asf);
  const frames = parseAmc(amc).map((frame) => pose(bones, frame));
  const align = restAlignment(bones);
  const duration = Math.min(to, (frames.length - 1) / rate) - from;
  if (!(duration > 1)) throw new Error(`${name}: window shorter than a second`);
  const count = Math.floor(duration * FPS) + 1;
  const lowest = (frame) => Math.min(...['lfoot', 'rfoot', 'ltoes', 'rtoes'].flatMap((bone) => [frame.start.get(bone).y, frame.end.get(bone).y]));
  const floors = frames.map(lowest).sort((a, b) => a - b);
  const floor = floors[Math.floor(floors.length * 0.1)];
  const drop = (bone) => -bones.get(bone).direction.y * bones.get(bone).length;
  const hipsHeight = drop('lhipjoint') + bones.get('lfemur').length + bones.get('ltibia').length + drop('lfoot') + drop('ltoes');
  const samples = [];
  for (let index = 0; index < count; index++) {
    const [a, b, t] = sample(frames, rate, from + index / FPS);
    const rotations = {};
    for (const [source, target] of Object.entries(BONES)) {
      rotations[target] = a.world.get(source).clone().slerp(b.world.get(source), t).multiply(align.get(target));
    }
    const ankles = ['lfoot', 'rfoot'].map((bone) => a.start.get(bone).clone().lerp(b.start.get(bone), t));
    samples.push({ rotations, hips: a.start.get('root').clone().lerp(b.start.get('root'), t), stance: new THREE.Vector3((ankles[0].x + ankles[1].x) / 2, Math.min(ankles[0].y, ankles[1].y), (ankles[0].z + ankles[1].z) / 2) });
  }
  const jump = stanceJump(samples.map(({ stance }) => stance), hipsHeight);
  if (jump.size > STANCE_JUMP) throw new Error(`${name}: its feet jump ${(jump.size * 100).toFixed(1)}% of hip height out and back at ${(from + jump.index / FPS).toFixed(2)} s of the take`);
  const facing = samples.reduce((sum, { rotations }) => sum.add(new THREE.Vector3(0, 0, 1).applyQuaternion(rotations.hips).setY(0).normalize()), new THREE.Vector3());
  const yaw = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0, 1, 0), -Math.atan2(facing.x, facing.z));
  const origin = samples[0].hips.clone().applyQuaternion(yaw);
  const local = samples.map(({ rotations, hips }) => ({
    hips: hips.clone().applyQuaternion(yaw).sub(new THREE.Vector3(origin.x, floor, origin.z)),
    rotations: Object.fromEntries(Object.entries(rotations).map(([bone, rotation]) => [bone, PARENT[bone] ? rotations[PARENT[bone]].clone().invert().multiply(rotation) : rotation.clone().premultiply(yaw)])),
  }));
  if (loop) {
    const seam = Math.min(count - 1, Math.round(SEAM * FPS));
    for (let step = 1; step <= seam; step++) {
      const entry = local[count - 1 - seam + step];
      const weight = THREE.MathUtils.smoothstep(step / seam, 0, 1);
      for (const [bone, rotation] of Object.entries(entry.rotations)) rotation.slerp(local[0].rotations[bone], weight);
      entry.hips.lerp(local[0].hips, weight);
    }
  }
  const times = local.map((_, index) => round(index / FPS));
  const tracks = Object.keys(BONES).map((source) => BONES[source]).map((bone) => ({
    name: `${bone}.quaternion`, times, values: local.flatMap(({ rotations }) => alignHemisphere(rotations[bone]).toArray().map(round)),
  }));
  tracks.push({ name: 'hips.position', times, values: local.flatMap(({ hips }) => hips.toArray().map(round)) });
  return { name, duration: round((count - 1) / FPS), hipsHeight: round(hipsHeight), tracks };
}

export function stanceJump(stances, hipsHeight) {
  let worst = { size: 0, index: 0 };
  for (let index = 1; index < stances.length - 1; index++) {
    const size = stances[index].distanceTo(stances[index - 1].clone().add(stances[index + 1]).multiplyScalar(0.5)) / hipsHeight;
    if (size > worst.size && size > stances[index - 1].distanceTo(stances[index + 1]) / hipsHeight) worst = { size, index };
  }
  return worst;
}

const round = (value) => Math.round(value * 1e5) / 1e5;
const alignHemisphere = (q) => q.w < 0 ? q.set(-q.x, -q.y, -q.z, -q.w) : q;

async function fetchPinned(url, sha256) {
  const file = path.join(cache, createHash('sha256').update(url).digest('hex').slice(0, 24));
  let bytes = fs.existsSync(file) ? fs.readFileSync(file) : null;
  if (!bytes) {
    const response = await fetch(url, { signal: AbortSignal.timeout(60000) });
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    bytes = Buffer.from(await response.arrayBuffer());
  }
  const actual = createHash('sha256').update(bytes).digest('hex');
  if (actual !== sha256) throw new Error(`${url} has sha256 ${actual}, expected ${sha256}`);
  fs.mkdirSync(cache, { recursive: true });
  fs.writeFileSync(file, bytes);
  return bytes.toString('latin1');
}

async function main() {
  const [command, out, catalog] = process.argv.slice(2);
  const sources = JSON.parse(fs.readFileSync(path.join(root, 'docs/poses/mocap.json'), 'utf8'));
  if (command === 'pin') {
    for (const take of Object.values(sources.takes)) {
      for (const kind of ['asf', 'amc']) {
        const url = sources.url.replace('{subject}', take.subject).replace('{file}', kind === 'asf' ? `${take.subject}.asf` : `${take.id}.amc`);
        const response = await fetch(url, { signal: AbortSignal.timeout(60000) });
        take[kind] = createHash('sha256').update(Buffer.from(await response.arrayBuffer())).digest('hex');
      }
    }
    fs.writeFileSync(path.join(root, 'docs/poses/mocap.json'), `${JSON.stringify(sources, null, 2)}\n`);
    return;
  }
  if (command !== 'write' || !out || !catalog) throw new Error('usage: node scripts/mocap.mjs write <clip dir> <catalog dir> | pin');
  if (path.resolve(out).startsWith(root + path.sep)) throw new Error('the clips are written outside the repository');
  const clips = [];
  for (const clip of sources.clips) {
    const take = sources.takes[clip.take];
    const asf = await fetchPinned(sources.url.replace('{subject}', take.subject).replace('{file}', `${take.subject}.asf`), take.asf);
    const amc = await fetchPinned(sources.url.replace('{subject}', take.subject).replace('{file}', `${take.id}.amc`), take.amc);
    clips.push({
      entry: { action: clip.action, name: `${clip.action}.motion`, source: `${sources.source} ${take.id} (${take.description}), ${clip.from ?? 0}–${clip.to ?? 'end'} s`, licence: sources.licence },
      motion: motion(asf, amc, { name: clip.action, rate: take.rate, from: clip.from, to: clip.to, loop: clip.action.replace(/-\d+$/, '') === 'idle' }),
    });
  }
  install(out, catalog, sources.source, clips);
  console.log(`${clips.length} clips written to ${out}`);
}

export function install(out, catalog, source, clips) {
  const figures = fs.readdirSync(catalog, { withFileTypes: true }).filter((entry) => entry.isFile() && !entry.name.startsWith('.') && entry.name.endsWith('.vrm')).map((entry) => entry.name).sort();
  const manifestFile = path.join(out, 'clips.manifest.json');
  const manifest = fs.existsSync(manifestFile) ? JSON.parse(fs.readFileSync(manifestFile, 'utf8')) : { clips: [] };
  const written = new Set(clips.map(({ entry }) => entry.name));
  for (const { entry, motion } of clips) fs.writeFileSync(path.join(out, entry.name), JSON.stringify(motion));
  const own = (entry) => entry.source?.startsWith(`${source} `);
  for (const entry of manifest.clips.filter((entry) => own(entry) && !written.has(entry.name))) fs.rmSync(path.join(out, entry.name), { force: true });
  const entries = clips.map(({ entry }) => entry);
  manifest.clips = [...manifest.clips.filter((entry) => !own(entry) && !written.has(entry.name) && !entries.some((mine) => mine.action === entry.action)), ...entries];
  fs.writeFileSync(manifestFile, `${JSON.stringify(manifest, null, 2)}\n`);
  const failed = figures.filter((name) => spawnSync('python3', [PUBLISH, path.join(catalog, name), path.join(catalog, name)], { stdio: 'inherit' }).status !== 0);
  if (failed.length) throw new Error(`${failed.length} of ${figures.length} catalog figures were not re-measured against the clips written, so the overlay refuses them until a write re-measures them: ${failed.join(', ')}`);
}

if (process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(new URL(import.meta.url))) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
