import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { crc32, deflateSync, gzipSync } from 'node:zlib';
import { build } from 'esbuild';
import { launchChromium, openPage } from './chromium.mjs';

const out = new URL('../vr/golden/', import.meta.url).pathname;
const GOLDEN = { eye: [-0.03, 0.02, 0.5], size: 256, time: 0.5, quad: 0.4, floor: -0.17, height: 0.3, pageHeight: 2.7 };

function png(width, height, rgba) {
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const check = Buffer.alloc(4);
    check.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, check]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 6, 0, 0, 0], 8);
  const rows = Buffer.concat(Array.from({ length: height }, (_, y) => Buffer.concat([Buffer.from([0]), rgba.subarray(y * width * 4, (y + 1) * width * 4)])));
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', header), chunk('IDAT', deflateSync(rows)), chunk('IEND', Buffer.alloc(0))]);
}

function webp(width, height, rgba) {
  const scratch = mkdtempSync(join(tmpdir(), 'vr-golden-'));
  try {
    writeFileSync(join(scratch, 'in.png'), png(width, height, rgba));
    execFileSync('cwebp', ['-quiet', '-lossless', '-exact', join(scratch, 'in.png'), '-o', join(scratch, 'out.webp')]);
    return readFileSync(join(scratch, 'out.webp'));
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}

function checker() {
  const colors = [[230, 60, 60], [250, 240, 220], [60, 90, 220], [250, 200, 60]];
  const pixels = Buffer.alloc(8 * 8 * 4);
  for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) pixels.set([...colors[(x >> 2) + 2 * (y >> 2)], 255], (y * 8 + x) * 4);
  return pixels;
}

const rotation = (axis, angle) => { const s = Math.sin(angle / 2); return [axis[0] * s, axis[1] * s, axis[2] * s, Math.cos(angle / 2)]; };
const rotate = ([x, y, z, w], [vx, vy, vz]) => {
  const [tx, ty, tz] = [2 * (y * vz - z * vy), 2 * (z * vx - x * vz), 2 * (x * vy - y * vx)];
  return [vx + w * tx + (y * tz - z * ty), vy + w * ty + (z * tx - x * tz), vz + w * tz + (x * ty - y * tx)];
};
const inverse = ([x, y, z, w]) => [-x, -y, -z, w];

function figure() {
  const armTurn = rotation([0, 0, 1], -0.3);
  const bones = [
    ['hips', null, [0, 0.95, 0]], ['spine', 'hips', [0, 1.05, 0]], ['chest', 'spine', [0, 1.2, 0]], ['neck', 'chest', [0, 1.4, 0]], ['head', 'neck', [0, 1.48, 0]],
    ['leftUpperLeg', 'hips', [0.09, 0.9, 0]], ['leftLowerLeg', 'leftUpperLeg', [0.09, 0.48, 0]], ['leftFoot', 'leftLowerLeg', [0.09, 0.08, 0]],
    ['rightUpperLeg', 'hips', [-0.09, 0.9, 0]], ['rightLowerLeg', 'rightUpperLeg', [-0.09, 0.48, 0]], ['rightFoot', 'rightLowerLeg', [-0.09, 0.08, 0]],
    ['leftUpperArm', 'chest', [0.17, 1.37, 0], armTurn], ['leftLowerArm', 'leftUpperArm', [0.43, 1.37, 0]], ['leftHand', 'leftLowerArm', [0.67, 1.37, 0]],
    ['rightUpperArm', 'chest', [-0.17, 1.37, 0]], ['rightLowerArm', 'rightUpperArm', [-0.43, 1.37, 0]], ['rightHand', 'rightLowerArm', [-0.67, 1.37, 0]],
  ];
  const index = new Map(bones.map(([name], at) => [name, at + 1]));
  const worldTurn = (name) => { const bone = bones.find(([each]) => each === name); return bone?.[3] ?? (bone?.[1] ? worldTurn(bone[1]) : [0, 0, 0, 1]); };
  const nodes = [{ name: 'Root', children: [index.get('hips')] }];
  for (const [name, parent, world, turn] of bones) {
    const from = parent ? bones.find(([each]) => each === parent)[2] : [0, 0, 0];
    const offset = world.map((value, axis) => value - from[axis]);
    const node = { name, translation: parent ? rotate(inverse(worldTurn(parent)), offset) : offset };
    if (turn) node.rotation = turn;
    const children = bones.filter(([, each]) => each === name).map(([child]) => index.get(child));
    if (children.length) node.children = children;
    nodes.push(node);
  }
  const meshNode = nodes.length;
  nodes.push({ name: 'Body', mesh: 0, skin: 0 });

  const positions = [], normals = [], uvs = [], joints = [], weights = [], primitives = [];
  const faces = [[[1, 0, 0], [0, 0, -1], [0, 1, 0]], [[-1, 0, 0], [0, 0, 1], [0, 1, 0]], [[0, 1, 0], [1, 0, 0], [0, 0, -1]], [[0, -1, 0], [1, 0, 0], [0, 0, 1]], [[0, 0, 1], [1, 0, 0], [0, 1, 0]], [[0, 0, -1], [-1, 0, 0], [0, 1, 0]]];
  const groups = new Map();
  const add = (material, triangle) => { if (!groups.has(material)) groups.set(material, []); groups.get(material).push(...triangle); };
  const box = (material, center, half, skin) => {
    for (const [normal, u, v] of faces) {
      const first = positions.length / 3;
      for (const [su, sv] of [[-1, -1], [1, -1], [1, 1], [-1, 1]]) {
        const point = [0, 1, 2].map((axis) => center[axis] + (normal[axis] + u[axis] * su + v[axis] * sv) * half[axis]);
        positions.push(...point);
        normals.push(...normal);
        uvs.push((su + 1) / 2, (1 - sv) / 2);
        const [slots, amounts] = skin(point);
        joints.push(...slots);
        weights.push(...amounts);
      }
      add(material, [first, first + 1, first + 2, first + 0, first + 2, first + 3]);
    }
  };
  const rigid = (name) => () => [[index.get(name) - 1, 0, 0, 0], [1, 0, 0, 0]];
  const middle = (from, to) => from.map((value, axis) => (value + to[axis]) / 2);
  const world = (name) => bones.find(([each]) => each === name)[2];
  box(0, [0, 1.14, 0], [0.16, 0.26, 0.09], (point) => point[1] > 1.14 ? [[index.get('chest') - 1, 0, 0, 0], [1, 0, 0, 0]] : [[index.get('spine') - 1, index.get('hips') - 1, 0, 0], [0.5, 0.5, 0, 0]]);
  box(1, [0, 1.6, 0.01], [0.1, 0.12, 0.11], rigid('head'));
  box(4, [0, 1.44, 0], [0.04, 0.05, 0.04], rigid('neck'));
  for (const side of ['left', 'right']) {
    const sign = side === 'left' ? 1 : -1;
    box(4, middle(world(`${side}UpperLeg`), world(`${side}LowerLeg`)), [0.06, 0.21, 0.06], rigid(`${side}UpperLeg`));
    box(4, middle(world(`${side}LowerLeg`), world(`${side}Foot`)), [0.05, 0.2, 0.05], rigid(`${side}LowerLeg`));
    box(0, [sign * 0.09, 0.04, 0.04], [0.05, 0.04, 0.1], rigid(`${side}Foot`));
    box(4, [sign * 0.3, 1.37, 0], [0.13, 0.04, 0.04], (point) => Math.abs(point[0]) > 0.4 ? [[index.get(`${side}UpperArm`) - 1, index.get(`${side}LowerArm`) - 1, 0, 0], [0.5, 0.5, 0, 0]] : [[index.get(`${side}UpperArm`) - 1, 0, 0, 0], [1, 0, 0, 0]]);
    box(4, [sign * 0.55, 1.37, 0], [0.12, 0.035, 0.035], rigid(`${side}LowerArm`));
    box(0, [sign * 0.71, 1.37, 0], [0.04, 0.05, 0.02], rigid(`${side}Hand`));
  }
  box(3, [0, 0.84, 0], [0.2, 0.12, 0.12], rigid('hips'));
  const eyes = positions.length / 3;
  for (const sign of [1, -1]) {
    const first = positions.length / 3;
    for (const [x, y] of [[-0.025, -0.02], [0.025, -0.02], [0.025, 0.02], [-0.025, 0.02]]) {
      positions.push(sign * 0.045 + x, 1.63 + y, 0.121);
      normals.push(0, 0, 1);
      uvs.push(0, 0);
      joints.push(index.get('head') - 1, 0, 0, 0);
      weights.push(1, 0, 0, 0);
    }
    add(2, [first, first + 1, first + 2, first, first + 2, first + 3]);
  }
  const blink = new Float32Array(positions.length);
  for (let vertex = eyes; vertex < positions.length / 3; vertex++) if (positions[vertex * 3 + 1] > 1.63) blink[vertex * 3 + 1] = -0.036;

  const binary = [], bufferViews = [], accessors = [];
  let length = 0;
  const view = (bytes, target) => {
    const padded = Buffer.concat([Buffer.from(bytes), Buffer.alloc((4 - bytes.byteLength % 4) % 4)]);
    bufferViews.push({ buffer: 0, byteOffset: length, byteLength: bytes.byteLength, ...(target ? { target } : {}) });
    binary.push(padded);
    length += padded.length;
    return bufferViews.length - 1;
  };
  const accessor = (array, type, componentType, extra = {}) => {
    const size = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT4: 16 }[type];
    accessors.push({ bufferView: view(new Uint8Array(array.buffer, array.byteOffset, array.byteLength)), componentType, count: array.length / size, type, ...extra });
    return accessors.length - 1;
  };
  const bounds = (values) => [0, 1, 2].map((axis) => values.filter((_, at) => at % 3 === axis)).map((axis) => [Math.min(...axis), Math.max(...axis)]);
  const range = bounds(positions);
  const attributes = {
    POSITION: accessor(new Float32Array(positions), 'VEC3', 5126, { min: range.map(([low]) => low), max: range.map(([, high]) => high) }),
    NORMAL: accessor(new Float32Array(normals), 'VEC3', 5126),
    TEXCOORD_0: accessor(new Float32Array(uvs), 'VEC2', 5126),
    JOINTS_0: accessor(new Uint16Array(joints), 'VEC4', 5123),
    WEIGHTS_0: accessor(new Float32Array(weights), 'VEC4', 5126),
  };
  const blinkRange = bounds([...blink]);
  const target = accessor(blink, 'VEC3', 5126, { min: blinkRange.map(([low]) => low), max: blinkRange.map(([, high]) => high) });
  for (const [material, triangles] of [...groups].sort(([a], [b]) => a - b)) primitives.push({ attributes, indices: accessor(new Uint16Array(triangles), 'SCALAR', 5123), material, targets: [{ POSITION: target }] });
  const inverseBind = new Float32Array(bones.flatMap(([name, , [x, y, z]]) => {
    const [qx, qy, qz, qw] = inverse(worldTurn(name));
    const [tx, ty, tz] = rotate([qx, qy, qz, qw], [-x, -y, -z]);
    return [1 - 2 * (qy * qy + qz * qz), 2 * (qx * qy + qz * qw), 2 * (qx * qz - qy * qw), 0, 2 * (qx * qy - qz * qw), 1 - 2 * (qx * qx + qz * qz), 2 * (qy * qz + qx * qw), 0, 2 * (qx * qz + qy * qw), 2 * (qy * qz - qx * qw), 1 - 2 * (qx * qx + qy * qy), 0, tx, ty, tz, 1];
  }));
  const skin = { joints: bones.map(([name]) => index.get(name)), inverseBindMatrices: accessor(inverseBind, 'MAT4', 5126), skeleton: index.get('hips') };
  const image = view(webp(8, 8, checker()));
  const toon = (extra) => ({ VRMC_materials_mtoon: { specVersion: '1.0', shadingToonyFactor: 0.9, shadingShiftFactor: 0, giEqualizationFactor: 0.9, ...extra } });
  const materials = [
    { name: 'body', pbrMetallicRoughness: { baseColorFactor: [0.8, 0.55, 0.45, 1] }, extensions: toon({ shadeColorFactor: [0.45, 0.25, 0.35], outlineWidthMode: 'worldCoordinates', outlineWidthFactor: 0.04, outlineColorFactor: [0.12, 0.05, 0.1], outlineLightingMixFactor: 0.5 }) },
    { name: 'head', pbrMetallicRoughness: { baseColorTexture: { index: 0 } }, extensions: toon({ shadeColorFactor: [0.55, 0.55, 0.7], shadeMultiplyTexture: { index: 0 }, outlineWidthMode: 'screenCoordinates', outlineWidthFactor: 0.02, outlineColorFactor: [0.05, 0.05, 0.15], outlineLightingMixFactor: 0 }) },
    { name: 'eyes', alphaMode: 'MASK', alphaCutoff: 0.5, pbrMetallicRoughness: { baseColorFactor: [0.1, 0.1, 0.2, 1] }, extensions: toon({ shadeColorFactor: [0.05, 0.05, 0.1] }) },
    { name: 'skirt', alphaMode: 'BLEND', pbrMetallicRoughness: { baseColorFactor: [0.3, 0.4, 0.9, 0.6] }, extensions: toon({ shadeColorFactor: [0.15, 0.2, 0.5], transparentWithZWrite: true }) },
    { name: 'limbs', pbrMetallicRoughness: { baseColorFactor: [0.9, 0.78, 0.68, 1] }, extensions: toon({ shadeColorFactor: [0.6, 0.45, 0.45] }) },
  ];
  const json = {
    asset: { version: '2.0', generator: 'voice vr golden figure' },
    extensionsUsed: ['VRMC_vrm', 'VRMC_materials_mtoon', 'EXT_texture_webp'],
    extensionsRequired: ['EXT_texture_webp'],
    scene: 0,
    scenes: [{ nodes: [0, meshNode] }],
    nodes, meshes: [{ primitives }], skins: [skin], materials,
    images: [{ bufferView: image, mimeType: 'image/webp' }],
    samplers: [{ magFilter: 9729, minFilter: 9987, wrapS: 10497, wrapT: 10497 }],
    textures: [{ sampler: 0, extensions: { EXT_texture_webp: { source: 0 } } }],
    bufferViews, accessors, buffers: [{ byteLength: length }],
    extensions: {
      VRMC_vrm: {
        specVersion: '1.0',
        meta: { name: 'golden figure', authors: ['voice'], licenseUrl: 'https://vrm.dev/licenses/1.0/', avatarPermission: 'everyone', commercialUsage: 'personalNonProfit', creditNotation: 'unnecessary', modification: 'allowModificationRedistribution' },
        humanoid: { humanBones: Object.fromEntries(bones.map(([name]) => [name, { node: index.get(name) }])) },
        expressions: { preset: { blink: { morphTargetBinds: [{ node: meshNode, index: 0, weight: 1 }], isBinary: false } } },
      },
    },
  };
  const text = Buffer.from(JSON.stringify(json));
  const jsonChunk = Buffer.concat([text, Buffer.alloc((4 - text.length % 4) % 4, 0x20)]);
  const binChunk = Buffer.concat(binary);
  const header = (size, type) => { const value = Buffer.alloc(8); value.writeUInt32LE(size, 0); value.write(type, 4, 'latin1'); return value; };
  const total = 12 + 8 + jsonChunk.length + 8 + binChunk.length;
  const head = Buffer.alloc(12);
  head.write('glTF', 0, 'latin1');
  head.writeUInt32LE(2, 4);
  head.writeUInt32LE(total, 8);
  return Buffer.concat([head, header(jsonChunk.length, 'JSON'), jsonChunk, header(binChunk.length, 'BIN\0'), binChunk]);
}

function idle() {
  const times = [0, 1, 2];
  const turn = (axis, angle) => [rotation(axis, 0), rotation(axis, angle), rotation(axis, 0)].flat();
  return {
    name: 'golden idle', duration: 2, hipsHeight: 95,
    tracks: [
      { name: 'hips.position', times, values: [0, 95, 0, 6, 92, 2, 0, 95, 0] },
      { name: 'spine.quaternion', times, values: turn([0, 0, 1], 0.2) },
      { name: 'head.quaternion', times, values: turn([0, 1, 0], 0.6) },
      { name: 'leftUpperArm.quaternion', times, values: turn([0, 0, 1], 0.8) },
      { name: 'rightLowerArm.quaternion', times, values: turn([0, 1, 0], -1.0) },
      { name: 'leftUpperLeg.quaternion', times, values: turn([1, 0, 0], -0.5) },
    ],
  };
}

async function pageRender(vrm, motion) {
  const bundle = await build({
    stdin: {
      resolveDir: new URL('.', import.meta.url).pathname,
      contents: `
        import * as THREE from 'three';
        import { PuppetRuntime } from '../src/puppet.js';
        globalThis.renderGolden = async (vrm, motion, { eye: [x, y, z], size, time, quad, floor, height, pageHeight }) => {
          const units = pageHeight / height;
          const canvas = document.createElement('canvas');
          document.body.append(canvas);
          const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: false, preserveDrawingBuffer: true });
          const camera = new THREE.PerspectiveCamera();
          const near = 0.05;
          const scene = ([px, py, pz]) => [px * units, (py - floor) * units, pz * units];
          const [ex, ey, ez] = scene([x, y, z]);
          const [left, bottom] = scene([-quad / 2, -quad / 2, 0]);
          const [right, top] = scene([quad / 2, quad / 2, 0]);
          const scale = near / ez;
          camera.position.set(ex, ey, ez);
          camera.updateMatrixWorld();
          camera.projectionMatrix.makePerspective((left - ex) * scale, (right - ex) * scale, (top - ey) * scale, (bottom - ey) * scale, near, 200);
          camera.projectionMatrixInverse.copy(camera.projectionMatrix).invert();
          const runtime = new PuppetRuntime(canvas, renderer);
          runtime.pause();
          runtime.resize.disconnect();
          renderer.setPixelRatio(1);
          renderer.setSize(size, size, false);
          renderer.setClearColor(0x000000, 0);
          runtime.camera = camera;
          const bytes = Uint8Array.from(atob(vrm), (char) => char.charCodeAt(0)).buffer;
          if (!await runtime.load(bytes)) throw new Error('figure did not load');
          runtime.vrm.lookAt = null;
          runtime.sleeping = true;
          await runtime.loadClips([{ action: 'idle', format: 'motion', data: motion }]);
          runtime.pose('stand');
          runtime.playIdle();
          runtime.clipAction.time = time;
          runtime.clock = { getDelta: () => 0 };
          runtime.animate(0);
          const gl = renderer.getContext();
          const pixels = new Uint8Array(size * size * 4);
          gl.readPixels(0, 0, size, size, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
          let text = '';
          for (let row = size - 1; row >= 0; row--) text += String.fromCharCode(...pixels.subarray(row * size * 4, (row + 1) * size * 4));
          return btoa(text);
        };`,
    },
    bundle: true, format: 'iife', write: false, logLevel: 'warning',
  });
  const chrome = await launchChromium({ args: ['--headless=new', '--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist'] });
  try {
    const { evaluate } = await openPage(chrome.devtools.call);
    await evaluate(bundle.outputFiles[0].text);
    return Buffer.from(await evaluate(`renderGolden(${JSON.stringify(vrm.toString('base64'))}, ${JSON.stringify(motion)}, ${JSON.stringify(GOLDEN)})`), 'base64');
  } finally { await chrome.close(); }
}

mkdirSync(out, { recursive: true });
const vrm = figure();
const motion = idle();
writeFileSync(join(out, 'figure.vrm'), vrm);
writeFileSync(join(out, 'idle.json'), `${JSON.stringify(motion)}\n`);
writeFileSync(join(out, 'pose.json'), `${JSON.stringify(GOLDEN)}\n`);
writeFileSync(join(out, 'page.rgba.gz'), gzipSync(await pageRender(vrm, motion), { level: 9 }));
