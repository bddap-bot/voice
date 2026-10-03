import * as THREE from 'three';

const PUPPET_UNITS = 2.7;

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
    this.camera = new THREE.PerspectiveCamera();
    this.eyes = null;
    this.viewer = null;
    this.ready = true;
    this.pixels = new Uint8Array(eye[0] * 2 * eye[1] * 4);
  }
  scene([x, y, z]) {
    return [x * this.units, (y + this.offset) * this.units, z * this.units];
  }
  pose({ eyes, head }) {
    this.eyes = eyes.map((point) => this.scene(point));
    this.viewer = new THREE.Vector3(...this.scene(head));
  }
  attach(renderer) {
    renderer.setPixelRatio(1);
    renderer.setSize(this.eye[0] * 2, this.eye[1], false);
    renderer.setClearColor(0x000000, 0);
  }
  render(renderer, scene) {
    if (!this.ready || !this.eyes || this.eyes.some(([, , z]) => z <= 0.01)) return;
    const [width, height] = this.eye;
    const near = 0.05;
    renderer.setScissorTest(true);
    renderer.clear();
    this.eyes.forEach((position, index) => {
      const { left, right, bottom, top } = eyeFrustum(position, this.rect, near);
      this.camera.position.fromArray(position);
      this.camera.quaternion.identity();
      this.camera.updateMatrixWorld();
      this.camera.projectionMatrix.makePerspective(left, right, top, bottom, near, 200);
      this.camera.projectionMatrixInverse.copy(this.camera.projectionMatrix).invert();
      renderer.setViewport(index * width, 0, width, height);
      renderer.setScissor(index * width, 0, width, height);
      renderer.render(scene, this.camera);
    });
    renderer.setScissorTest(false);
    const gl = renderer.getContext();
    gl.readPixels(0, 0, width * 2, height, gl.RGBA, gl.UNSIGNED_BYTE, this.pixels);
    this.ready = false;
    this.send(this.pixels);
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
      else if (message.type === 'ready' && view) view.ready = true;
      else if (message.type === 'tap') onTap();
    });
  });
}
