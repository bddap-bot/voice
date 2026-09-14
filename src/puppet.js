import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { VRMLoaderPlugin, VRMUtils } from '@pixiv/three-vrm';

const BONE_POSES = {
  sit: {
    leftUpperLeg: [-1.12, 0, 0.05],
    rightUpperLeg: [-1.12, 0, -0.05],
    leftLowerLeg: [1.32, 0, 0],
    rightLowerLeg: [1.32, 0, 0],
    spine: [-0.08, 0, 0],
    leftUpperArm: [0.12, 0, -1.05],
    rightUpperArm: [0.12, 0, 1.05],
    leftLowerArm: [0, 0, -0.18],
    rightLowerArm: [0, 0, 0.18],
  },
  stand: {
    leftUpperArm: [0.08, 0, -1.22],
    rightUpperArm: [0.08, 0, 1.22],
    leftLowerArm: [0, 0, -0.12],
    rightLowerArm: [0, 0, 0.12],
  },
  listen: {
    spine: [-0.04, 0, 0],
    head: [0.08, 0, 0.03],
    leftUpperArm: [0.14, 0, -1.08],
    rightUpperArm: [0.14, 0, 1.08],
    leftLowerArm: [0, 0, -0.42],
    rightLowerArm: [0, 0, 0.42],
  },
};

const VISEMES = ['aa', 'ih', 'ou', 'ee', 'oh'];

function bandEnergy(spectrum, sampleRate, fftSize, low, high) {
  const first = Math.max(1, Math.ceil(low * fftSize / sampleRate));
  const last = Math.min(spectrum.length - 1, Math.floor(high * fftSize / sampleRate));
  let total = 0;
  for (let index = first; index <= last; index++) total += spectrum[index];
  return total / Math.max(1, last - first + 1) / 255;
}

export function audioVisemes(waveform, spectrum, sampleRate, fftSize) {
  let square = 0;
  for (const sample of waveform) {
    const centered = (sample - 128) / 128;
    square += centered * centered;
  }
  const rms = Math.sqrt(square / waveform.length);
  const gate = THREE.MathUtils.smoothstep(rms, 0.018, 0.16);
  const raw = {
    aa: bandEnergy(spectrum, sampleRate, fftSize, 800, 1300),
    ih: bandEnergy(spectrum, sampleRate, fftSize, 2400, 4000),
    ou: bandEnergy(spectrum, sampleRate, fftSize, 180, 420),
    ee: bandEnergy(spectrum, sampleRate, fftSize, 1300, 2400),
    oh: bandEnergy(spectrum, sampleRate, fftSize, 420, 800),
  };
  const shaped = Object.fromEntries(VISEMES.map((name) => [name, raw[name] * raw[name]]));
  const total = Math.max(Object.values(shaped).reduce((sum, value) => sum + value, 0), 0.001);
  return Object.fromEntries(VISEMES.map((name) => [name, gate * shaped[name] / total * 0.82]));
}

export class PuppetRuntime {
  constructor(canvas) {
    this.canvas = canvas;
    this.renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
    this.renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.scene = new THREE.Scene();
    this.camera = new THREE.PerspectiveCamera(28, 1, 0.05, 30);
    this.camera.position.set(0, 1.25, 6.4);
    this.camera.lookAt(0, 1.25, 0);
    this.stage = new THREE.Group();
    this.stage.position.y = -0.72;
    this.idleRoot = new THREE.Group();
    this.stage.add(this.idleRoot);
    this.scene.add(this.stage);
    this.scene.add(new THREE.HemisphereLight(0xd9e6ff, 0x15111d, 2.7));
    const key = new THREE.DirectionalLight(0xffd9bf, 3.1);
    key.position.set(2.5, 4, 3);
    this.scene.add(key);
    const rim = new THREE.DirectionalLight(0x7d8cff, 2.4);
    rim.position.set(-3, 2, -2);
    this.scene.add(rim);
    this.mixer = new THREE.AnimationMixer(this.idleRoot);
    const idle = new THREE.AnimationClip('base-idle', 6, [
      new THREE.NumberKeyframeTrack('.position[y]', [0, 1.5, 3, 4.5, 6], [0, 0.018, 0, 0.012, 0]),
      new THREE.NumberKeyframeTrack('.rotation[z]', [0, 2, 4, 6], [-0.012, 0.015, -0.008, -0.012]),
      new THREE.NumberKeyframeTrack('.rotation[y]', [0, 2.5, 4.5, 6], [0, 0.018, -0.015, 0]),
    ]);
    this.mixer.clipAction(idle).play();
    this.clock = new THREE.Clock();
    this.poseName = 'sit';
    this.poseStart = performance.now();
    this.poseFromY = -0.72;
    this.poseToY = -0.72;
    this.bones = new Map();
    this.lookTarget = new THREE.Euler();
    this.lookOffset = new THREE.Quaternion();
    this.lookTargetQuaternion = new THREE.Quaternion();
    this.nextLook = 0;
    this.nextBlink = performance.now() + 1200;
    this.blinkStart = 0;
    this.audio = null;
    this.audioEpoch = 0;
    this.mouthValues = Object.fromEntries(VISEMES.map((name) => [name, 0]));
    this.resize = new ResizeObserver(() => this.fit());
    this.resize.observe(canvas);
    this.fit();
    this.animate = this.animate.bind(this);
    this.frame = 0;
    this.start();
  }
  start() {
    if (!this.frame) this.frame = requestAnimationFrame(this.animate);
  }
  pause() {
    cancelAnimationFrame(this.frame);
    this.frame = 0;
  }
  async load(bytes, valid = () => true, beforeCommit = async () => {}) {
    const loader = new GLTFLoader();
    loader.register((parser) => new VRMLoaderPlugin(parser));
    const url = URL.createObjectURL(new Blob([bytes], { type: 'model/gltf-binary' }));
    let gltf;
    try { gltf = await loader.loadAsync(url); }
    finally { URL.revokeObjectURL(url); }
    const vrm = gltf.userData.vrm;
    if (!vrm) throw new Error('file is not a VRM puppet');
    VRMUtils.rotateVRM0(vrm);
    if (!valid()) {
      VRMUtils.deepDispose(vrm.scene);
      return false;
    }
    try { await beforeCommit(); }
    catch (error) {
      VRMUtils.deepDispose(vrm.scene);
      throw error;
    }
    if (!valid()) {
      VRMUtils.deepDispose(vrm.scene);
      return false;
    }
    if (this.vrm) {
      this.idleRoot.remove(this.vrm.scene);
      VRMUtils.deepDispose(this.vrm.scene);
    }
    const box = new THREE.Box3().setFromObject(vrm.scene);
    const size = box.getSize(new THREE.Vector3());
    const scale = size.y ? 2.7 / size.y : 1;
    vrm.scene.scale.setScalar(scale);
    const fitted = new THREE.Box3().setFromObject(vrm.scene);
    const center = fitted.getCenter(new THREE.Vector3());
    vrm.scene.position.x -= center.x;
    vrm.scene.position.y -= fitted.min.y;
    vrm.scene.position.z -= center.z;
    vrm.scene.traverse((object) => { object.frustumCulled = false; });
    this.idleRoot.add(vrm.scene);
    this.vrm = vrm;
    this.bones.clear();
    for (const bone of ['leftUpperLeg', 'rightUpperLeg', 'leftLowerLeg', 'rightLowerLeg', 'spine', 'head', 'leftUpperArm', 'rightUpperArm', 'leftLowerArm', 'rightLowerArm']) {
      const node = vrm.humanoid?.getNormalizedBoneNode(bone);
      if (!node) continue;
      const rest = node.quaternion.clone();
      this.bones.set(bone, { node, rest, from: rest.clone(), target: rest.clone() });
    }
    this.pose(this.poseName);
    return true;
  }
  clear() {
    if (!this.vrm) return;
    this.idleRoot.remove(this.vrm.scene);
    VRMUtils.deepDispose(this.vrm.scene);
    this.vrm = null;
    this.bones.clear();
  }
  async attachAudio(stream, owner = stream) {
    const epoch = ++this.audioEpoch;
    const previous = this.audio;
    this.audio = null;
    previous?.source.disconnect();
    previous?.context.close().catch(() => {});
    const Context = globalThis.AudioContext ?? globalThis.webkitAudioContext;
    if (!Context) throw new Error('Web Audio is unavailable');
    const context = new Context();
    let source;
    try {
      const analyser = context.createAnalyser();
      analyser.fftSize = 256;
      analyser.smoothingTimeConstant = 0.25;
      source = context.createMediaStreamSource(stream);
      source.connect(analyser);
      const audio = {
        context,
        source,
        analyser,
        owner,
        waveform: new Uint8Array(analyser.fftSize),
        spectrum: new Uint8Array(analyser.frequencyBinCount),
      };
      this.audio = audio;
      await context.resume();
      if (epoch !== this.audioEpoch) {
        if (this.audio === audio) this.audio = null;
        source.disconnect();
        await context.close().catch(() => {});
        return false;
      }
      return true;
    } catch (error) {
      if (this.audio?.context === context) this.audio = null;
      source?.disconnect();
      await context.close().catch(() => {});
      throw error;
    }
  }
  async detachAudio(owner) {
    if (owner && this.audio?.owner !== owner) return;
    this.audioEpoch++;
    const audio = this.audio;
    this.audio = null;
    audio?.source.disconnect();
    for (const name of VISEMES) {
      this.mouthValues[name] = 0;
      this.vrm?.expressionManager?.setValue(name, 0);
    }
    await audio?.context.close().catch(() => {});
  }
  fit() {
    const width = Math.max(1, this.canvas.clientWidth);
    const height = Math.max(1, this.canvas.clientHeight);
    this.renderer.setSize(width, height, false);
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
  }
  pose(name) {
    if (!BONE_POSES[name]) throw new Error(`unknown pose ${name}`);
    this.poseName = name;
    this.poseStart = performance.now();
    this.poseFromY = this.stage.position.y;
    this.poseToY = name === 'sit' ? -0.72 : 0;
    for (const [bone, entry] of this.bones) {
      entry.from.copy(entry.node.quaternion);
      entry.target.setFromEuler(new THREE.Euler(...(BONE_POSES[name][bone] ?? [0, 0, 0]))).multiply(entry.rest);
    }
  }
  updatePose(now) {
    const amount = THREE.MathUtils.smoothstep((now - this.poseStart) / 420, 0, 1);
    this.stage.position.y = THREE.MathUtils.lerp(this.poseFromY, this.poseToY, amount);
    for (const entry of this.bones.values()) entry.node.quaternion.slerpQuaternions(entry.from, entry.target, amount);
  }
  updateFace(now) {
    const manager = this.vrm?.expressionManager;
    if (manager) {
      if (now >= this.nextBlink && !this.blinkStart) this.blinkStart = now;
      if (this.blinkStart) {
        const phase = (now - this.blinkStart) / 180;
        manager.setValue('blink', Math.sin(Math.min(1, phase) * Math.PI));
        if (phase >= 1) {
          this.blinkStart = 0;
          this.nextBlink = now + 2200 + Math.random() * 4200;
        }
      }
      this.updateMouth(manager);
    }
    const head = this.bones.get('head')?.node;
    if (!head) return;
    if (now >= this.nextLook) {
      this.lookTarget.set((Math.random() - 0.5) * 0.1, (Math.random() - 0.5) * 0.24, 0);
      this.lookTargetQuaternion.setFromEuler(this.lookTarget);
      this.nextLook = now + 2400 + Math.random() * 3600;
    }
    if (this.poseName === 'stand' || this.poseName === 'sit') {
      this.lookOffset.slerp(this.lookTargetQuaternion, 0.012);
      head.quaternion.multiply(this.lookOffset);
    }
  }
  updateMouth(manager) {
    let targets;
    if (this.audio) {
      this.audio.analyser.getByteTimeDomainData(this.audio.waveform);
      this.audio.analyser.getByteFrequencyData(this.audio.spectrum);
      targets = audioVisemes(this.audio.waveform, this.audio.spectrum, this.audio.context.sampleRate, this.audio.analyser.fftSize);
    }
    for (const name of VISEMES) {
      this.mouthValues[name] = THREE.MathUtils.lerp(this.mouthValues[name], targets?.[name] ?? 0, 0.65);
      manager.setValue(name, this.mouthValues[name]);
    }
  }
  animate(now) {
    const delta = Math.min(this.clock.getDelta(), 0.05);
    this.mixer.update(delta);
    this.updatePose(now);
    this.updateFace(now);
    this.vrm?.update(delta);
    this.renderer.render(this.scene, this.camera);
    this.frame = requestAnimationFrame(this.animate);
  }
  dispose() {
    this.pause();
    this.resize.disconnect();
    this.detachAudio();
    this.clear();
    this.renderer.dispose();
  }
}
