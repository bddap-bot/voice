import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { VRMLoaderPlugin, VRMUtils } from '@pixiv/three-vrm';
import { VRMAnimationLoaderPlugin, createVRMAnimationClip } from '@pixiv/three-vrm-animation';

const VISEMES = ['aa', 'ih', 'ou', 'ee', 'oh'];
const MOOD_EXPRESSIONS = ['happy', 'angry', 'sad', 'relaxed', 'surprised'];

const GESTURES = {
  beat: { rightUpperArm: [-0.24, 0, 0.4], rightLowerArm: [-0.48, 0, 0.16] },
  point_at: { spine: [0, -0.42, 0], head: [0, 0.28, 0], rightUpperArm: [0, 0, 1.2], rightLowerArm: [0, 0, -0.08] },
  waiting: { spine: [-0.08, 0.12, 0], head: [0.12, -0.18, 0.08], leftUpperArm: [-0.28, 0, -0.15], rightUpperArm: [-0.58, 0, 0.4], rightLowerArm: [-0.92, 0, 0.26] },
};

const CLIP_GESTURES = new Set(['point', 'nod', 'shrug', 'think', 'wave', 'no', 'laugh', 'clap', 'bow', 'thumbs-up', 'stretch', 'look-around']);
const IDLE_CLIPS = { stand: ['idle', 'idle-2', 'idle-3'], sit: ['sit-idle', 'sit-idle-2'] };
const SEATED_ARM_CLEARANCE = { leftUpperArm: [0, 0, -0.1], rightUpperArm: [0, 0, 0.1] };

const GAZE_POINTS = {
  camera: [0, 1.25, 6.4],
  panel: [2.8, 1.35, 2.4],
  away: [-1.8, 1.8, 2.8],
};

function handoverFor(from, to) {
  const fromTracks = new Map((from.userData.poseTracks ?? []).map((track) => [track.name, track]));
  const pairs = (to.userData.poseTracks ?? []).flatMap((track) => fromTracks.has(track.name) ? [[fromTracks.get(track.name), track]] : []);
  if (!pairs.length) return { offset: 0, duration: 1.5 };
  const samples = Math.max(2, Math.ceil(to.duration * 60));
  let best = { offset: 0, distance: Infinity };
  for (let sample = 0; sample < samples; sample++) {
    const offset = sample * to.duration / samples;
    let square = 0;
    for (const [left, right] of pairs) {
      const a = left.interpolant.evaluate(from.duration);
      const b = right.interpolant.evaluate(offset);
      if (left.valueSize === 4) square += new THREE.Quaternion().fromArray(a).angleTo(new THREE.Quaternion().fromArray(b)) ** 2;
      else square += new THREE.Vector3().fromArray(a).distanceToSquared(new THREE.Vector3().fromArray(b));
    }
    const distance = Math.sqrt(square / pairs.length);
    if (distance < best.distance) best = { offset, distance };
  }
  return { offset: best.offset, duration: THREE.MathUtils.clamp(1.5 + best.distance * 4, 1.5, 2.5) };
}

export async function animationClip(bytes, format, vrm) {
  if (format === 'vrma') {
    const loader = new GLTFLoader();
    loader.register((parser) => new VRMAnimationLoaderPlugin(parser));
    const url = URL.createObjectURL(new Blob([bytes], { type: 'model/gltf-binary' }));
    try {
      const gltf = await loader.loadAsync(url);
      const animation = gltf.userData.vrmAnimations?.[0];
      if (!animation) throw new Error('VRMA has no animation');
      return createVRMAnimationClip(animation, vrm);
    } finally { URL.revokeObjectURL(url); }
  }
  if (format === 'tracks') {
    const value = JSON.parse(new TextDecoder().decode(bytes));
    if (!value || typeof value.name !== 'string' || !Number.isFinite(value.duration) || !Array.isArray(value.tracks)) throw new Error('invalid animation tracks');
    const tracks = value.tracks.map((track) => {
      if (typeof track?.name !== 'string' || !Array.isArray(track.times) || !Array.isArray(track.values)) throw new Error('invalid animation track');
      return track.name.endsWith('.quaternion')
        ? new THREE.QuaternionKeyframeTrack(track.name, track.times, track.values)
        : new THREE.VectorKeyframeTrack(track.name, track.times, track.values);
    });
    if (!tracks.some((track) => track.name.endsWith('.quaternion'))) throw new Error('animation has no rotation tracks');
    const clip = new THREE.AnimationClip(value.name, value.duration, tracks);
    clip.userData.poseTracks = tracks.map((track) => ({ name: track.name, valueSize: track.getValueSize(), interpolant: track.createInterpolant() }));
    return clip;
  }
  throw new Error(`unsupported animation format ${format}`);
}

export const MOOD_TABLE = {
  curious: { expressions: { surprised: 0.22 }, bones: { head: [-0.03, 0.12, 0.08], leftShoulder: [0, 0, 0.05] } },
  amused: { expressions: { happy: 0.68, relaxed: 0.12 }, bones: { head: [0.02, -0.08, -0.07], rightShoulder: [0, 0, -0.06] } },
  puzzled: { expressions: { sad: 0.2, surprised: 0.12 }, bones: { head: [0.04, 0.14, 0.1], leftShoulder: [0, 0, 0.08] } },
  thinking: { expressions: { relaxed: 0.28 }, bones: { head: [0.08, -0.12, 0.04], rightShoulder: [0.03, 0, -0.05] } },
  pleased: { expressions: { happy: 0.76 }, bones: { head: [-0.04, 0, -0.03], leftShoulder: [0, 0, 0.04], rightShoulder: [0, 0, -0.04] } },
  sad: { expressions: { sad: 0.78 }, bones: { head: [0.13, 0, 0.06], leftShoulder: [0.09, 0, 0.06], rightShoulder: [0.09, 0, -0.06] } },
  angry: { expressions: { angry: 0.78 }, bones: { head: [0.03, -0.1, -0.08], spine: [-0.05, 0, 0] } },
  apologetic: { expressions: { sad: 0.48 }, bones: { head: [0.09, 0, 0.04], leftShoulder: [0.06, 0, 0.03], rightShoulder: [0.06, 0, -0.03] } },
  alert: { expressions: { surprised: 0.35 }, bones: { head: [-0.08, 0, 0], spine: [-0.04, 0, 0] } },
  sleepy: { expressions: { relaxed: 0.68 }, bones: { head: [0.16, -0.08, 0.08], spine: [0.08, 0, 0] } },
  relaxed: { expressions: { relaxed: 0.78 }, bones: { head: [0.06, 0.05, 0.04], leftShoulder: [0.07, 0, 0.05], rightShoulder: [0.07, 0, -0.05] } },
  surprised: { expressions: { surprised: 0.9 }, bones: { head: [-0.12, 0, 0], leftShoulder: [-0.08, 0, 0.08], rightShoulder: [-0.08, 0, -0.08] } },
  skeptical: { expressions: { angry: 0.18 }, bones: { head: [0.02, -0.16, -0.11], rightShoulder: [0, 0, -0.06] } },
};

function copyOffsets(source = {}) {
  return Object.fromEntries(Object.entries(source).map(([name, values]) => [name, [...values]]));
}

function blendOffsets(from, to, amount) {
  const names = new Set([...Object.keys(from), ...Object.keys(to)]);
  return Object.fromEntries([...names].map((name) => [name, [0, 1, 2].map((axis) => THREE.MathUtils.lerp(from[name]?.[axis] ?? 0, to[name]?.[axis] ?? 0, amount))]));
}

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

export function audioEnergy(waveform) {
  let square = 0;
  for (const sample of waveform) {
    const centered = (sample - 128) / 128;
    square += centered * centered;
  }
  return Math.sqrt(square / waveform.length);
}

export function shouldBeat(energy, previousEnergy, waiting) {
  return !waiting && energy > 0.075 && energy > previousEnergy * 1.28;
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
    this.clips = new Map();
    this.handovers = new Map();
    this.clipAction = null;
    this.clipFallback = null;
    this.clipGesture = null;
    this.pendingGesture = null;
    this.idleClip = null;
    this.nextIdleAt = Infinity;
    this.clock = new THREE.Clock();
    this.poseName = 'sit';
    this.bones = new Map();
    this.gazeTarget = new THREE.Object3D();
    this.gazeTarget.position.fromArray(GAZE_POINTS.camera);
    this.scene.add(this.gazeTarget);
    this.gazePoint = this.gazeTarget.position.clone();
    this.gazeDestination = this.gazePoint.clone();
    this.gazeMode = 'camera';
    this.gazeUntil = 0;
    this.nextSaccade = 0;
    this.gazeRotation = new THREE.Quaternion();
    this.nextBlink = performance.now() + 1200;
    this.blinkStart = 0;
    this.audio = null;
    this.audioEpoch = 0;
    this.mouthValues = Object.fromEntries(VISEMES.map((name) => [name, 0]));
    this.moodValues = Object.fromEntries(MOOD_EXPRESSIONS.map((name) => [name, 0]));
    this.moodName = null;
    this.moodFrom = {};
    this.moodBones = {};
    this.moodStarted = performance.now();
    this.gestureState = null;
    this.gestureOffsets = {};
    this.gestureRotation = new THREE.Quaternion();
    this.listeningMotion = { amount: 0, lean: 0, nod: 0, tilt: 0 };
    this.waitingForHub = false;
    this.previousEnergy = 0;
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
  async load(bytes, initialClip, valid = () => true, beforeCommit = async () => {}) {
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
    const preparedClip = await animationClip(initialClip.bytes, initialClip.format, vrm);
    if (!valid()) {
      VRMUtils.deepDispose(vrm.scene);
      return false;
    }
    if (this.vrm) {
      this.idleRoot.remove(this.vrm.scene);
      VRMUtils.deepDispose(this.vrm.scene);
    }
    this.vrm = vrm;
    if (vrm.lookAt) vrm.lookAt.target = this.gazeTarget;
    this.bones.clear();
    for (const bone of ['leftUpperLeg', 'rightUpperLeg', 'leftLowerLeg', 'rightLowerLeg', 'spine', 'head', 'leftShoulder', 'rightShoulder', 'leftUpperArm', 'rightUpperArm', 'leftLowerArm', 'rightLowerArm']) {
      const node = vrm.humanoid?.getNormalizedBoneNode(bone);
      if (!node) continue;
      const rest = node.quaternion.clone();
      this.bones.set(bone, { node, rest, base: rest.clone(), from: rest.clone(), target: rest.clone() });
    }
    for (const name of MOOD_EXPRESSIONS) {
      const expression = vrm.expressionManager?.getExpression(name);
      if (expression) {
        expression.overrideMouth = 'none';
        expression.overrideBlink = 'none';
        expression.overrideLookAt = 'none';
      }
    }
    this.clips.clear();
    preparedClip.userData.action = initialClip.action;
    this.clips.set(initialClip.action, preparedClip);
    this.prepareHandovers();
    this.idleRoot.add(vrm.scene);
    this.playIdle();
    return true;
  }
  async loadClips(entries) {
    for (const entry of entries) {
      const clip = await animationClip(entry.bytes, entry.format, this.vrm);
      clip.userData.action = entry.action;
      this.clips.set(entry.action, clip);
    }
    this.prepareHandovers();
    this.pose(this.poseName);
  }
  prepareHandovers() {
    this.handovers.clear();
    for (const [transition, idles] of [['stand', IDLE_CLIPS.stand], ['sit', IDLE_CLIPS.sit]]) {
      const from = this.clips.get(transition);
      if (!from) continue;
      for (const idle of idles) {
        const to = this.clips.get(idle);
        if (to) this.handovers.set(`${transition}:${idle}`, handoverFor(from, to));
      }
    }
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
    const resolved = name === 'listen' ? 'idle' : name;
    if (!['sit', 'stand', 'idle'].includes(resolved)) throw new Error(`unknown pose ${name}`);
    this.poseName = name;
    const transition = resolved === 'sit' ? 'sit' : resolved === 'stand' ? 'stand' : 'idle';
    const fallback = resolved === 'sit' ? 'sit-idle' : 'idle';
    this.playClip(transition, fallback);
  }
  gesture(name, target) {
    const resolved = name === 'point' && target === 'panel' ? 'point_at' : name;
    if (!GESTURES[resolved] && !CLIP_GESTURES.has(resolved)) throw new Error(`unknown gesture ${name}`);
    if (resolved === 'beat' && (this.waitingForHub || this.gestureState || this.clipGesture)) return;
    if (resolved === 'point_at') this.setGaze('panel', 2200);
    if (resolved === 'think') this.setGaze('away', 1800);
    if (CLIP_GESTURES.has(resolved)) {
      if (this.poseName === 'sit' && this.clipStance(resolved) === 'stand') {
        this.pendingGesture = { name, target };
        this.pose('stand');
        return true;
      }
      this.gestureState = null;
      this.gestureOffsets = {};
      this.playClip(resolved, this.poseName === 'sit' ? 'sit-idle' : 'idle');
      this.clipGesture = resolved;
    } else {
      if (this.clipGesture) this.playClip(this.poseName === 'sit' ? 'sit-idle' : 'idle', this.poseName === 'sit' ? 'sit-idle' : 'idle');
      this.clipGesture = null;
      this.beginGesture(resolved, false);
    }
    return true;
  }
  clipStance(name) {
    const position = this.clips.get(name)?.tracks.find((track) => track.name.endsWith('.position'));
    const stand = this.clips.get('idle')?.tracks.find((track) => track.name.endsWith('.position'));
    const sit = this.clips.get('sit-idle')?.tracks.find((track) => track.name.endsWith('.position'));
    if (!position || !stand || !sit) return 'stand';
    const height = position.values[1];
    return Math.abs(height - sit.values[1]) < Math.abs(height - stand.values[1]) ? 'sit' : 'stand';
  }
  playIdle(now = performance.now()) {
    const pose = this.poseName === 'sit' ? 'sit' : 'stand';
    const available = IDLE_CLIPS[pose].filter((name) => this.clips.has(name));
    const choices = available.filter((name) => name !== this.idleClip);
    const name = choices[Math.floor(Math.random() * choices.length)] ?? available[0];
    if (!name) return;
    this.idleClip = name;
    this.nextIdleAt = available.length > 1 ? now + 7000 + Math.random() * 7000 : Infinity;
    this.playClip(name, name);
  }
  playClip(name, fallback) {
    const clip = this.clips.get(name);
    if (!clip) return;
    const previous = this.clipAction;
    const handover = previous && this.handovers?.get(`${previous.getClip().userData.action}:${name}`);
    const duration = handover?.duration ?? 0.18;
    previous?.fadeOut(duration);
    const action = this.mixer.clipAction(clip, this.vrm.scene).reset();
    if (handover) action.time = handover.offset;
    action.fadeIn(duration).play();
    action.setLoop(name === fallback ? THREE.LoopRepeat : THREE.LoopOnce, name === fallback ? Infinity : 1);
    action.clampWhenFinished = name !== fallback;
    this.clipAction = action;
    this.clipFallback = fallback;
  }
  beginGesture(name, hold) {
    const now = performance.now();
    this.gestureState = { name, from: copyOffsets(this.gestureOffsets), to: copyOffsets(GESTURES[name]), started: now, releaseAt: hold ? Infinity : now + 900, releasing: false };
  }
  waiting(active) {
    this.waitingForHub = active;
    if (active && !this.clipGesture && !this.gestureState) this.beginGesture('waiting', true);
    else if (this.gestureState?.name === 'waiting') this.releaseGesture(performance.now());
  }
  listening(motion) {
    this.listeningMotion = motion;
  }
  releaseGesture(now) {
    this.gestureState = { name: this.gestureState?.name, from: copyOffsets(this.gestureOffsets), to: {}, started: now, releaseAt: Infinity, releasing: true };
  }
  look(direction) {
    if (direction !== 'toward' && direction !== 'away') throw new Error(`unknown look ${direction}`);
    this.setGaze(direction === 'away' ? 'away' : 'camera', 1800);
  }
  setGaze(mode, duration, now = performance.now()) {
    this.gazeMode = mode;
    this.gazeUntil = now + duration;
    this.gazeDestination.fromArray(GAZE_POINTS[mode]);
  }
  mood(name) {
    if (!MOOD_TABLE[name]) throw new Error(`unknown mood ${name}`);
    if (this.moodName === name) return;
    this.moodName = name;
    this.moodFrom = copyOffsets(this.moodBones);
    this.moodStarted = performance.now();
  }
  updatePose(now) {
    if (this.clipAction?.isRunning() && this.clipFallback === this.idleClip && now >= this.nextIdleAt) {
      this.playIdle(now);
      return;
    }
    if (this.clipAction && !this.clipAction.isRunning() && this.clipFallback) {
      this.clipFallback = null;
      this.clipGesture = null;
      if (this.pendingGesture) {
        const pending = this.pendingGesture;
        this.pendingGesture = null;
        this.gesture(pending.name, pending.target);
        return;
      }
      this.playIdle(now);
      if (this.waitingForHub) this.beginGesture('waiting', true);
    }
  }
  updateGesture(now) {
    if (!this.gestureState) return;
    if (!this.gestureState.releasing && now >= this.gestureState.releaseAt) this.releaseGesture(now);
    const amount = THREE.MathUtils.smoothstep((now - this.gestureState.started) / 320, 0, 1);
    this.gestureOffsets = blendOffsets(this.gestureState.from, this.gestureState.to, amount);
    for (const [name, values] of Object.entries(this.gestureOffsets)) {
      const bone = this.bones.get(name)?.node;
      if (!bone) continue;
      this.gestureRotation.setFromEuler(new THREE.Euler(...values));
      bone.quaternion.multiply(this.gestureRotation);
    }
    if (this.gestureState.releasing && amount === 1) {
      this.gestureState = null;
      if (this.waitingForHub) this.beginGesture('waiting', true);
    }
  }
  updateSeatedClearance() {
    if (this.poseName !== 'sit') return;
    for (const [name, values] of Object.entries(SEATED_ARM_CLEARANCE)) {
      const bone = this.bones.get(name)?.node;
      if (!bone) continue;
      this.gestureRotation.setFromEuler(new THREE.Euler(...values));
      bone.quaternion.multiply(this.gestureRotation);
    }
  }
  updateListening() {
    const { lean, nod, tilt } = this.listeningMotion;
    const spine = this.bones.get('spine')?.node;
    const head = this.bones.get('head')?.node;
    if (spine) {
      this.gestureRotation.setFromEuler(new THREE.Euler(lean, 0, 0));
      spine.quaternion.multiply(this.gestureRotation);
    }
    if (head) {
      this.gestureRotation.setFromEuler(new THREE.Euler(nod, 0, tilt));
      head.quaternion.multiply(this.gestureRotation);
    }
  }
  updateMood(now, manager) {
    const mood = MOOD_TABLE[this.moodName];
    for (const name of MOOD_EXPRESSIONS) {
      this.moodValues[name] = THREE.MathUtils.lerp(this.moodValues[name], mood?.expressions[name] ?? 0, 0.12);
      manager.setValue(name, this.moodValues[name]);
    }
    if (!mood) return;
    const amount = THREE.MathUtils.smoothstep((now - this.moodStarted) / 320, 0, 1);
    this.moodBones = blendOffsets(this.moodFrom, mood.bones, amount);
    for (const [name, values] of Object.entries(this.moodBones)) {
      const bone = this.bones.get(name)?.node;
      if (!bone) continue;
      this.gestureRotation.setFromEuler(new THREE.Euler(...values));
      bone.quaternion.multiply(this.gestureRotation);
    }
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
      this.updateMood(now, manager);
      this.updateMouth(manager);
    }
    this.updateGaze(now);
  }
  updateGaze(now) {
    if (!this.vrm?.lookAt) return;
    if (this.gazeMode !== 'camera' && now >= this.gazeUntil) {
      this.gazeMode = 'camera';
      this.nextSaccade = now;
    }
    if (this.gazeMode === 'camera' && now >= this.nextSaccade) {
      this.gazeDestination.fromArray(GAZE_POINTS.camera);
      this.gazeDestination.x += (Math.random() - 0.5) * 0.24;
      this.gazeDestination.y += (Math.random() - 0.5) * 0.12;
      this.nextSaccade = now + 1800 + Math.random() * 3200;
    } else if (this.gazeMode !== 'camera') this.gazeDestination.fromArray(GAZE_POINTS[this.gazeMode]);
    this.gazePoint.lerp(this.gazeDestination, 0.08);
    this.gazeTarget.position.copy(this.gazePoint);
    const head = this.bones.get('head')?.node;
    if (!head) return;
    const yaw = THREE.MathUtils.clamp(Math.atan2(this.gazePoint.x, this.gazePoint.z) * 0.18, -0.14, 0.14);
    const pitch = THREE.MathUtils.clamp(-Math.atan2(this.gazePoint.y - 1.25, Math.hypot(this.gazePoint.x, this.gazePoint.z)) * 0.14, -0.08, 0.08);
    this.gazeRotation.setFromEuler(new THREE.Euler(pitch, yaw, 0));
    head.quaternion.multiply(this.gazeRotation);
  }
  updateMouth(manager) {
    let targets;
    if (this.audio) {
      this.audio.analyser.getByteTimeDomainData(this.audio.waveform);
      this.audio.analyser.getByteFrequencyData(this.audio.spectrum);
      targets = audioVisemes(this.audio.waveform, this.audio.spectrum, this.audio.context.sampleRate, this.audio.analyser.fftSize);
      const energy = audioEnergy(this.audio.waveform);
      if (shouldBeat(energy, this.previousEnergy, this.waitingForHub)) this.gesture('beat');
      this.previousEnergy = energy;
    }
    for (const name of VISEMES) {
      this.mouthValues[name] = THREE.MathUtils.lerp(this.mouthValues[name], targets?.[name] ?? 0, 0.65);
      manager.setValue(name, this.mouthValues[name]);
    }
    if (!this.audio) this.previousEnergy = 0;
  }
  animate(now) {
    const delta = Math.min(this.clock.getDelta(), 0.05);
    this.mixer.update(delta);
    this.updatePose(now);
    this.updateSeatedClearance();
    this.updateGesture(now);
    this.updateListening();
    this.updateFace(now);
    this.vrm?.update(delta);
    if (this.clipAction) this.renderer.render(this.scene, this.camera);
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
