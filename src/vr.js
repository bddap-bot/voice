import * as THREE from 'three';

const PUPPET_UNITS = 2.7;
const HEADER = 8;
const EDGE_PIXELS = 2;

const corner = new THREE.Vector3();
const seen = new THREE.Vector3();
const bone = new THREE.Matrix4();
const placed = new THREE.Matrix4();
const spread = new THREE.Vector3();
const low = new THREE.Vector3();
const pieces = new WeakMap();
const merged = new WeakMap();
const identity = new THREE.Matrix4();

function outlinePad(material, mode) {
  const materials = Array.isArray(material) ? material : [material];
  return Math.max(0, ...materials.map((each) => each.isOutline && each.outlineWidthMode === mode ? each.outlineWidthFactor : 0));
}

function piecesOf(mesh) {
  if (pieces.has(mesh)) return pieces.get(mesh);
  const { position, skinIndex, skinWeight } = mesh.geometry.attributes;
  const morphs = mesh.geometry.morphAttributes.position ?? [];
  const relative = mesh.geometry.morphTargetsRelative;
  const skinned = mesh.isSkinnedMesh && skinIndex && skinWeight;
  const found = new Map();
  const point = new THREE.Vector3();
  const reach = new THREE.Vector3();
  const target = new THREE.Vector3();
  for (let vertex = 0; vertex < position.count; vertex++) {
    point.fromBufferAttribute(position, vertex);
    reach.set(0, 0, 0);
    for (const morph of morphs) {
      target.fromBufferAttribute(morph, vertex);
      if (!relative) target.sub(point);
      reach.x += Math.abs(target.x);
      reach.y += Math.abs(target.y);
      reach.z += Math.abs(target.z);
    }
    for (let slot = 0; slot < (skinned ? 4 : 1); slot++) {
      if (skinned && !(skinWeight.getComponent(vertex, slot) > 0)) continue;
      const index = skinned ? skinIndex.getComponent(vertex, slot) : -1;
      if (!found.has(index)) found.set(index, { box: new THREE.Box3(), reach: new THREE.Vector3() });
      const piece = found.get(index);
      piece.box.expandByPoint(point);
      piece.reach.max(reach);
    }
  }
  const result = { pieces: [...found], world: outlinePad(mesh.material, 'worldCoordinates'), screen: outlinePad(mesh.material, 'screenCoordinates') };
  pieces.set(mesh, result);
  return result;
}

function mergedPieces(meshes, inner) {
  const key = meshes.map((mesh) => mesh.id).join();
  const cached = merged.get(meshes[0]);
  if (cached?.key === key) return cached;
  const found = new Map();
  let world = 0;
  let screen = 0;
  for (const mesh of meshes) {
    const own = piecesOf(mesh);
    world = Math.max(world, own.world);
    screen = Math.max(screen, own.screen);
    for (const [index, piece] of own.pieces) {
      if (!found.has(index)) found.set(index, { box: new THREE.Box3(), reach: new THREE.Vector3() });
      found.get(index).box.union(piece.box);
      found.get(index).reach.max(piece.reach);
    }
  }
  const pieces = [...found].map(([index, { box: local, reach }]) => {
    const center = local.getCenter(new THREE.Vector3());
    const half = local.getSize(new THREE.Vector3()).multiplyScalar(0.5);
    const linear = inner.elements;
    return { index, center: center.applyMatrix4(inner), half: extent(linear, half, new THREE.Vector3()), reach: extent(linear, reach, new THREE.Vector3()) };
  });
  const result = { key, pieces, world, screen };
  merged.set(meshes[0], result);
  return result;
}

function extent(e, { x, y, z }, target) {
  return target.set(
    Math.abs(e[0]) * x + Math.abs(e[4]) * y + Math.abs(e[8]) * z,
    Math.abs(e[1]) * x + Math.abs(e[5]) * y + Math.abs(e[9]) * z,
    Math.abs(e[2]) * x + Math.abs(e[6]) * y + Math.abs(e[10]) * z,
  );
}

export function puppetBounds(scene) {
  const groups = [];
  scene.traverseVisible((mesh) => {
    if (!mesh.isMesh) return;
    const skeleton = mesh.isSkinnedMesh ? mesh.skeleton : null;
    const outer = skeleton ? new THREE.Matrix4().multiplyMatrices(mesh.matrixWorld, mesh.bindMatrixInverse) : mesh.matrixWorld;
    const inner = skeleton ? mesh.bindMatrix : identity;
    const group = groups.find((each) => each.skeleton === skeleton && each.outer.equals(outer) && each.inner.equals(inner));
    if (group) group.meshes.push(mesh);
    else groups.push({ skeleton, outer, inner, meshes: [mesh] });
  });
  const bounds = new THREE.Box3();
  let screen = 0;
  for (const { skeleton, outer, inner, meshes } of groups) {
    const { pieces: parts, world, screen: outline } = mergedPieces(meshes, inner);
    const morphing = Math.max(1, ...meshes.flatMap((mesh) => mesh.morphTargetInfluences ?? []).map(Math.abs));
    for (const { index, center, half, reach } of parts) {
      const matrix = index < 0 ? outer : placed.multiplyMatrices(outer, bone.fromArray(skeleton.boneMatrices, index * 16));
      spread.copy(reach).multiplyScalar(morphing).add(half);
      extent(matrix.elements, spread, spread).addScalar(world);
      corner.copy(center).applyMatrix4(matrix);
      bounds.expandByPoint(low.copy(corner).sub(spread)).expandByPoint(corner.add(spread));
    }
    screen = Math.max(screen, outline);
  }
  return { box: bounds, screen };
}

function pixelRect(camera, { box: bounds, screen: outline }, [width, height]) {
  if (bounds.isEmpty()) return [0, 0, 0, 0];
  const elements = camera.projectionMatrix.elements;
  const screen = outline * Math.max(1, elements[0] / elements[5]);
  const low = [Infinity, Infinity];
  const high = [-Infinity, -Infinity];
  for (let index = 0; index < 8; index++) {
    corner.set(index & 1 ? bounds.max.x : bounds.min.x, index & 2 ? bounds.max.y : bounds.min.y, index & 4 ? bounds.max.z : bounds.min.z);
    if (seen.copy(corner).applyMatrix4(camera.matrixWorldInverse).z >= -camera.near) return [0, 0, width, height];
    corner.project(camera);
    low[0] = Math.min(low[0], corner.x);
    low[1] = Math.min(low[1], corner.y);
    high[0] = Math.max(high[0], corner.x);
    high[1] = Math.max(high[1], corner.y);
  }
  const pixel = (ndc, size, round) => Math.min(size, Math.max(0, round((ndc + 1) / 2 * size)));
  const left = pixel(low[0] - screen, width, Math.floor) - EDGE_PIXELS;
  const bottom = pixel(low[1] - screen, height, Math.floor) - EDGE_PIXELS;
  const right = pixel(high[0] + screen, width, Math.ceil) + EDGE_PIXELS;
  const top = pixel(high[1] + screen, height, Math.ceil) + EDGE_PIXELS;
  return [left, bottom, right, top];
}

function union(rects, [width, height]) {
  const [left, bottom, right, top] = rects.reduce((a, b) => [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[2], b[2]), Math.max(a[3], b[3])]);
  const x = Math.max(0, left);
  const y = Math.max(0, bottom);
  const w = Math.max(0, Math.min(width, right) - x);
  const h = Math.max(0, Math.min(height, top) - y);
  return w && h ? [x, y, w, h] : [0, 0, 0, 0];
}

export function eyeFrustum([x, y, z], { left, right, bottom, top }, near) {
  const scale = near / z;
  return { left: (left - x) * scale, right: (right - x) * scale, bottom: (bottom - y) * scale, top: (top - y) * scale };
}

export class StereoView {
  constructor({ eye, quad, height, margin }, send) {
    this.eye = eye;
    this.send = send;
    this.units = PUPPET_UNITS / height;
    const [width, tall] = quad;
    this.offset = tall / 2 - margin;
    this.rect = { left: -width / 2 * this.units, right: width / 2 * this.units, bottom: -margin * this.units, top: (tall - margin) * this.units };
    this.cameras = [new THREE.PerspectiveCamera(), new THREE.PerspectiveCamera()];
    this.eyes = null;
    this.viewer = null;
    this.onPose = null;
    this.frame = new Uint8Array(HEADER + eye[0] * 2 * eye[1] * 4);
  }
  scene([x, y, z]) {
    return [x * this.units, (y + this.offset) * this.units, z * this.units];
  }
  pose({ eyes, head }) {
    this.eyes = eyes.map((point) => this.scene(point));
    this.viewer = new THREE.Vector3(...this.scene(head));
    this.onPose?.();
  }
  attach(renderer) {
    renderer.setPixelRatio(1);
    renderer.setSize(this.eye[0] * 2, this.eye[1], false);
    renderer.setClearColor(0x000000, 0);
  }
  render(renderer, scene) {
    if (!this.eyes || this.eyes.some(([, , z]) => z <= 0.01)) return;
    const [width, height] = this.eye;
    const near = 0.05;
    renderer.setScissorTest(true);
    renderer.clear();
    this.eyes.forEach((position, index) => {
      const camera = this.cameras[index];
      const { left, right, bottom, top } = eyeFrustum(position, this.rect, near);
      camera.position.fromArray(position);
      camera.quaternion.identity();
      camera.updateMatrixWorld();
      camera.projectionMatrix.makePerspective(left, right, top, bottom, near, 200);
      camera.projectionMatrixInverse.copy(camera.projectionMatrix).invert();
      renderer.setViewport(index * width, 0, width, height);
      renderer.setScissor(index * width, 0, width, height);
      renderer.render(scene, camera);
    });
    renderer.setScissorTest(false);
    const bounds = puppetBounds(scene);
    const [x, y, w, h] = union(this.cameras.map((camera) => pixelRect(camera, bounds, this.eye)), this.eye);
    const header = new DataView(this.frame.buffer);
    [x, y, w, h].forEach((value, index) => header.setUint16(index * 2, value, true));
    const gl = renderer.getContext();
    const size = w * h * 4;
    if (size) this.eyes.forEach((_, index) => gl.readPixels(index * width + x, y, w, h, gl.RGBA, gl.UNSIGNED_BYTE, this.frame, HEADER + index * size));
    this.send(this.frame.subarray(0, HEADER + 2 * size));
  }
}

export function connectVrHost(param, { WebSocket = globalThis.WebSocket, onTap = () => {} } = {}) {
  const [port, nonce] = String(param).split('.');
  if (!/^\d+$/.test(port) || !nonce) throw new Error('vr host parameter is <port>.<nonce>');
  const socket = new WebSocket(`ws://127.0.0.1:${port}/`);
  socket.binaryType = 'arraybuffer';
  return new Promise((resolve, reject) => {
    let view = null;
    socket.addEventListener('open', () => socket.send(JSON.stringify({ type: 'hello', nonce })));
    socket.addEventListener('error', () => reject(new Error('vr host unreachable')));
    socket.addEventListener('close', () => { if (!view) reject(new Error('vr host closed')); });
    socket.addEventListener('message', ({ data }) => {
      const message = JSON.parse(data);
      if (message.type === 'hello') {
        view = new StereoView(message, (pixels) => socket.send(pixels));
        resolve({ token: message.token, view });
      } else if (message.type === 'pose') view?.pose(message);
      else if (message.type === 'tap') onTap();
    });
  });
}
