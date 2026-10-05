import * as THREE from 'three';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { VRMLoaderPlugin, VRMUtils } from '@pixiv/three-vrm';
import { VRMAnimationLoaderPlugin, createVRMAnimationClip } from '@pixiv/three-vrm-animation';

import { standingPose, anchorStandingIdle } from './standing.js';

const VISEMES = ['aa', 'ih', 'ou', 'ee', 'oh'];
const MOOD_EXPRESSIONS = ['happy', 'angry', 'sad', 'relaxed', 'surprised'];

const CLIP_GESTURES = new Set(['nod', 'shrug', 'think', 'wave', 'no', 'laugh', 'clap', 'bow', 'thumbs-up', 'stretch', 'look-around']);
const IDLE_CLIPS = { stand: ['idle', 'idle-2', 'idle-3'], sit: ['sit-idle', 'sit-idle-2'] };

const GAZE_POINTS = {
  camera: [0, 1.25, 6.4],
  away: [-1.8, 1.8, 2.8],
};

function handoverFor(from, to) {
  const fromTracks = new Map((from.userData.poseTracks ?? []).map((track) => [track.name, track]));
  const pairs = (to.userData.poseTracks ?? []).flatMap((track) => track.valueSize === 4 && fromTracks.has(track.name) ? [[fromTracks.get(track.name), track]] : []);
  if (!pairs.length) return { offset: 0, duration: 1.5 };
  const samples = Math.max(2, Math.ceil(to.duration * 60));
  let best = { offset: 0, distance: Infinity };
  for (let sample = 0; sample < samples; sample++) {
    const offset = sample * to.duration / samples;
    let square = 0;
    for (const [left, right] of pairs) {
      const a = left.interpolant.evaluate(from.duration);
      const b = right.interpolant.evaluate(offset);
      square += new THREE.Quaternion().fromArray(a).angleTo(new THREE.Quaternion().fromArray(b)) ** 2;
    }
    const distance = Math.sqrt(square / pairs.length);
    if (distance < best.distance) best = { offset, distance };
  }
  return { offset: best.offset, duration: THREE.MathUtils.clamp(1.5 + best.distance * 4, 1.5, 2.5) };
}

export async function animationClip(data, format, vrm) {
  if (format === 'vrma') {
    const loader = new GLTFLoader();
    loader.register((parser) => new VRMAnimationLoaderPlugin(parser));
    const url = URL.createObjectURL(new Blob([data], { type: 'model/gltf-binary' }));
    try {
      const gltf = await loader.loadAsync(url);
      const animation = gltf.userData.vrmAnimations?.[0];
      if (!animation) throw new Error('VRMA has no animation');
      return createVRMAnimationClip(animation, vrm);
    } finally { URL.revokeObjectURL(url); }
  }
  if (format === 'motion') {
    if (!data || typeof data.name !== 'string' || !Number.isFinite(data.duration) || !Array.isArray(data.tracks)) throw new Error('invalid motion');
    const mirrored = vrm.meta?.metaVersion === '0';
    const tracks = data.tracks.flatMap((track) => {
      if (typeof track?.name !== 'string' || !Array.isArray(track.times) || !Array.isArray(track.values)) throw new Error('invalid motion track');
      const [bone, property] = track.name.split('.');
      if (property !== 'quaternion' && track.name !== 'hips.position') throw new Error('invalid motion track');
      const node = vrm.humanoid?.getNormalizedBoneNode(bone);
      if (!node) return [];
      if (property === 'quaternion') return [new THREE.QuaternionKeyframeTrack(`${node.name}.quaternion`, track.times, mirrored ? track.values.map((v, index) => index % 2 ? v : -v) : track.values)];
      if (!(data.hipsHeight > 0)) throw new Error('motion has no source hips height');
      const scale = Math.abs(vrm.humanoid.normalizedRestPose.hips.position[1]) / data.hipsHeight;
      return [new THREE.VectorKeyframeTrack(`${node.name}.position`, track.times, track.values.map((v, index) => v * scale * (mirrored && index % 3 !== 1 ? -1 : 1)))];
    });
    if (!tracks.some((track) => track.name.endsWith('.quaternion'))) throw new Error('animation has no rotation tracks');
    const clip = new THREE.AnimationClip(data.name, data.duration, tracks);
    clip.userData.poseTracks = tracks.map((track) => ({ name: track.name, valueSize: track.getValueSize(), interpolant: track.createInterpolant() }));
    return clip;
  }
  throw new Error(`unsupported animation format ${format}`);
}

export const MOOD_TABLE = {
  neutral: {},
  curious: { surprised: 0.22 },
  amused: { happy: 0.68, relaxed: 0.12 },
  puzzled: { sad: 0.2, surprised: 0.12 },
  thinking: { relaxed: 0.28 },
  pleased: { happy: 0.76 },
  sad: { sad: 0.78 },
  angry: { angry: 0.78 },
  apologetic: { sad: 0.48 },
  alert: { surprised: 0.35 },
  sleepy: { relaxed: 0.68 },
  relaxed: { relaxed: 0.78 },
  surprised: { surprised: 0.9 },
  skeptical: { angry: 0.18 },
};

const LETTER_VISEMES = { a: 'aa', i: 'ih', u: 'ou', e: 'ee', y: 'ee', o: 'oh', w: 'oh' };

export function transcriptVisemes(text) {
  return [...text.toLowerCase()].map((letter) => LETTER_VISEMES[letter] ?? null);
}

export function loudnessViseme(name, waveform) {
  const amount = THREE.MathUtils.smoothstep(audioEnergy(waveform), 0.018, 0.16) * 0.82;
  return Object.fromEntries(VISEMES.map((viseme) => [viseme, viseme === name ? amount : 0]));
}

export function audioEnergy(waveform) {
  let square = 0;
  for (const sample of waveform) {
    const centered = (sample - 128) / 128;
    square += centered * centered;
  }
  return Math.sqrt(square / waveform.length);
}

function feetOf(vrm) {
  return ['leftFoot', 'rightFoot'].map((name) => vrm.humanoid.getRawBoneNode(name).getWorldPosition(new THREE.Vector3()));
}

function fitScene(vrm) {
  VRMUtils.removeUnnecessaryVertices(vrm.scene);
  VRMUtils.combineSkeletons(vrm.scene);
  const box = new THREE.Box3().setFromObject(vrm.scene);
  const size = box.getSize(new THREE.Vector3());
  const scale = size.y ? 2.7 / size.y : 1;
  vrm.scene.scale.setScalar(scale);
  const ground = new THREE.Box3().setFromObject(vrm.scene).min.y;
  const ankleHeight = Math.min(...feetOf(vrm).map((foot) => foot.y)) - ground;
  vrm.scene.traverse((object) => { object.frustumCulled = false; });
  vrm.humanoid.setNormalizedPose(standingPose(vrm));
  vrm.update(0);
  return ankleHeight;
}

export class PuppetRuntime {
  constructor(canvas, renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true })) {
    this.canvas = canvas;
    this.renderer = renderer;
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
    this.animationActions = new Set();
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
    this.head = null;
    this.saccade = new THREE.Vector3();
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
    this.speech = [];
    this.speechUntil = 0;
    this.mouthValues = Object.fromEntries(VISEMES.map((name) => [name, 0]));
    this.moodName = 'sleepy';
    this.sleeping = true;
    this.waitingForHub = false;
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
  async load(bytes, valid = () => true, beforeCommit = async () => {}, stage = (name, work) => work()) {
    const loader = new GLTFLoader();
    loader.register((parser) => new VRMLoaderPlugin(parser));
    const url = URL.createObjectURL(new Blob([bytes], { type: 'model/gltf-binary' }));
    let gltf;
    try { gltf = await stage('parse', () => loader.loadAsync(url)); }
    finally { URL.revokeObjectURL(url); }
    const vrm = gltf.userData.vrm;
    if (!vrm) throw new Error('file is not a VRM puppet');
    VRMUtils.rotateVRM0(vrm);
    if (!valid()) {
      VRMUtils.deepDispose(vrm.scene);
      return false;
    }
    try { await stage('select', beforeCommit); }
    catch (error) {
      VRMUtils.deepDispose(vrm.scene);
      throw error;
    }
    const ankleHeight = await stage('fit', () => fitScene(vrm));
    this.clear();
    this.vrm = vrm;
    this.ankleHeight = ankleHeight;
    if (vrm.lookAt) vrm.lookAt.target = this.gazeTarget;
    const head = vrm.humanoid?.getNormalizedBoneNode('head');
    this.head = head ? { node: head, base: head.quaternion.clone() } : null;
    for (const expression of vrm.expressionManager?.expressions ?? []) {
      if (!MOOD_EXPRESSIONS.includes(expression.expressionName.toLowerCase())) continue;
      expression.overrideMouth = 'none';
      expression.overrideBlink = 'none';
      expression.overrideLookAt = 'none';
    }
    this.clips.clear();
    this.clipAction = null;
    this.clipFallback = null;
    this.idleClip = null;
    this.pose(this.poseName);
    this.idleRoot.add(vrm.scene);
    await stage('render', () => {
      this.renderer.render(this.scene, this.camera);
      this.renderer.getContext().finish();
    });
    return true;
  }
  async loadClips(entries) {
    const vrm = this.vrm;
    for (const entry of entries) {
      const clip = await animationClip(entry.data, entry.format, vrm);
      if (this.vrm !== vrm) return;
      if (IDLE_CLIPS.stand.includes(entry.action)) anchorStandingIdle(clip, vrm);
      clip.userData.action = entry.action;
      this.clips.set(entry.action, clip);
    }
    this.prepareHandovers();
    if (this.poseName === 'sit') this.playClip('sit', 'sit-idle');
    else if (!this.clipAction) this.playIdle();
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
    for (const clip of this.clips.values()) this.mixer.existingAction(clip, this.vrm.scene)?.stop();
    this.mixer.uncacheRoot(this.vrm.scene);
    this.idleRoot.remove(this.vrm.scene);
    VRMUtils.deepDispose(this.vrm.scene);
    this.vrm = null;
    this.head = null;
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
    this.speech = [];
    this.speechUntil = 0;
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
    if (!['sit', 'stand', 'listen'].includes(name)) throw new Error(`unknown pose ${name}`);
    this.clipGesture = null;
    this.pendingGesture = null;
    const transition = (name === 'sit') !== (this.poseName === 'sit');
    this.poseName = name;
    if (transition) this.playClip(name === 'sit' ? 'sit' : 'stand', name === 'sit' ? 'sit-idle' : 'idle');
  }
  gesture(name) {
    if (!CLIP_GESTURES.has(name)) throw new Error(`unknown gesture ${name}`);
    if (name === 'think') this.setGaze('away', 1800);
    if (this.poseName === 'sit' && this.clipStance(name) === 'stand') {
      this.pose('stand');
      this.pendingGesture = { name };
      return true;
    }
    this.playClip(name, this.poseName === 'sit' ? 'sit-idle' : 'idle');
    this.clipGesture = name;
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
    const duration = handover?.duration ?? (IDLE_CLIPS.stand.includes(name) ? 0.6 : 0.18);
    const scheduled = [...this.clips.values()].map((loaded) => this.mixer.existingAction(loaded, this.vrm.scene)).filter((existing) => existing?.isScheduled());
    for (const outgoing of scheduled) outgoing.setEffectiveWeight(outgoing.getEffectiveWeight()).fadeOut(duration);
    const action = this.mixer.clipAction(clip, this.vrm.scene).reset().setEffectiveWeight(1);
    if (handover) action.time = handover.offset;
    if (scheduled.some((outgoing) => outgoing !== action && outgoing.getEffectiveWeight() > 0)) action.fadeIn(duration);
    action.play();
    (this.animationActions ??= new Set()).add(action);
    action.setLoop(name === fallback ? THREE.LoopRepeat : THREE.LoopOnce, name === fallback ? Infinity : 1);
    action.clampWhenFinished = name !== fallback;
    this.clipAction = action;
    this.clipFallback = fallback;
  }
  waiting(active) {
    const started = active && !this.waitingForHub;
    this.waitingForHub = active;
    if (started && !this.clipGesture && this.clips.has('think')) this.gesture('think');
  }
  speak(text, playAt = performance.now()) {
    if (playAt > this.speechUntil) this.speech = [];
    const start = Math.max(playAt, this.speechUntil);
    const cadence = 72;
    this.speech.push(...transcriptVisemes(text).map((name, index) => ({ name, at: start + index * cadence })));
    this.speechUntil = start + text.length * cadence;
  }
  setGaze(mode, duration, now = performance.now()) {
    this.gazeMode = mode;
    this.gazeUntil = now + duration;
    this.gazeDestination.fromArray(GAZE_POINTS[mode]);
  }
  asleep(value) {
    this.sleeping = value;
    this.mood(value ? 'sleepy' : null);
  }
  mood(name) {
    if (name !== null && !MOOD_TABLE[name]) throw new Error(`unknown mood ${name}`);
    if (this.moodName === name) return;
    this.moodName = name;
  }
  updatePose(now) {
    if (this.clipAction?.isRunning() && this.clipAction.getClip() === this.clips.get(this.idleClip) && now >= this.nextIdleAt) {
      this.playIdle(now);
      return;
    }
    if (this.clipAction && !this.clipAction.isRunning() && this.clipFallback) {
      this.clipFallback = null;
      this.clipGesture = null;
      if (this.pendingGesture) {
        const pending = this.pendingGesture;
        this.pendingGesture = null;
        this.gesture(pending.name);
        return;
      }
      this.playIdle(now);
    }
  }
  updateBasePose(delta) {
    this.head?.node.quaternion.copy(this.head.base);
    this.mixer.update(delta);
    this.head?.base.copy(this.head.node.quaternion);
  }
  updateMood(manager) {
    const mood = MOOD_TABLE[this.moodName];
    for (const expression of manager.expressions) {
      const name = expression.expressionName.toLowerCase();
      if (MOOD_EXPRESSIONS.includes(name)) expression.weight = THREE.MathUtils.lerp(expression.weight, mood?.[name] ?? 0, 0.12);
    }
  }
  updateFace(now) {
    const manager = this.vrm?.expressionManager;
    if (manager) {
      if (!this.sleeping && now >= this.nextBlink && !this.blinkStart) this.blinkStart = now;
      const phase = this.blinkStart ? (now - this.blinkStart) / 180 : 0;
      manager.setValue('blink', this.sleeping ? 1 : Math.sin(Math.min(1, phase) * Math.PI));
      if (phase >= 1) {
        this.blinkStart = 0;
        this.nextBlink = now + 2200 + Math.random() * 4200;
      }
      this.updateMood(manager);
      this.updateMouth(manager, now);
    }
    this.updateGaze(now);
  }
  updateGaze(now) {
    if (!this.vrm?.lookAt) return;
    if (this.gazeMode !== 'camera' && now >= this.gazeUntil) {
      this.gazeMode = 'camera';
      this.nextSaccade = now;
    }
    if (this.gazeMode === 'camera') {
      if (now >= this.nextSaccade) {
        this.saccade.set((Math.random() - 0.5) * 0.24, (Math.random() - 0.5) * 0.12, 0);
        this.nextSaccade = now + 1800 + Math.random() * 3200;
      }
      this.gazeDestination.fromArray(GAZE_POINTS.camera).add(this.saccade);
    } else if (this.gazeMode !== 'camera') this.gazeDestination.fromArray(GAZE_POINTS[this.gazeMode]);
    this.gazePoint.lerp(this.gazeDestination, 0.08);
    this.gazeTarget.position.copy(this.gazePoint);
    const head = this.head?.node;
    if (!head) return;
    const yaw = THREE.MathUtils.clamp(Math.atan2(this.gazePoint.x, this.gazePoint.z) * 0.18, -0.14, 0.14);
    const pitch = THREE.MathUtils.clamp(-Math.atan2(this.gazePoint.y - 1.25, Math.hypot(this.gazePoint.x, this.gazePoint.z)) * 0.14, -0.08, 0.08);
    this.gazeRotation.setFromEuler(new THREE.Euler(pitch, yaw, 0));
    head.quaternion.multiply(this.gazeRotation);
  }
  updateMouth(manager, now = performance.now()) {
    let targets;
    if (this.audio) {
      this.audio.analyser.getByteTimeDomainData(this.audio.waveform);
      while (this.speech[1]?.at <= now) this.speech.shift();
      const speaking = this.speech[0]?.at <= now && now < this.speechUntil;
      const name = speaking ? this.speech[0].name ?? 'aa' : 'aa';
      targets = loudnessViseme(name, this.audio.waveform);
    }
    for (const name of VISEMES) {
      this.mouthValues[name] = THREE.MathUtils.lerp(this.mouthValues[name], targets?.[name] ?? 0, 0.65);
      manager.setValue(name, this.mouthValues[name]);
    }
  }
  plantFeet() {
    const { humanoid, scene } = this.vrm;
    humanoid.update();
    const [left, right] = feetOf(this.vrm);
    scene.position.x -= (left.x + right.x) / 2;
    scene.position.y += this.ankleHeight - Math.min(left.y, right.y);
    scene.position.z -= (left.z + right.z) / 2;
  }
  recordAnimation() {
    if (!this.onAnimation) return;
    const clips = [];
    for (const action of this.animationActions ?? []) {
      if (action.getRoot() !== this.idleRoot && action.getRoot() !== this.vrm?.scene) {
        this.animationActions.delete(action);
        continue;
      }
      const weight = action.enabled && action.isScheduled() ? action.getEffectiveWeight() : 0;
      if (weight > 0) clips.push({ name: action.getClip().userData.action ?? action.getClip().name, weight: Number(weight.toFixed(4)) });
    }
    const state = JSON.stringify(clips);
    if (state === this.animationState) return;
    this.animationState = state;
    const hips = this.vrm?.humanoid?.getRawBoneNode('hips');
    const hipHeight = hips ? Number(hips.getWorldPosition(new THREE.Vector3()).y.toFixed(4)) : null;
    this.onAnimation({ clips, hip_height: hipHeight });
  }
  animate(now = performance.now()) {
    const delta = Math.min(this.clock.getDelta(), 0.05);
    this.updatePose(now);
    this.updateBasePose(delta);
    this.updateFace(now);
    if (this.vrm) this.plantFeet();
    this.vrm?.update(delta);
    this.recordAnimation();
    if (this.vrm) this.renderer.render(this.scene, this.camera);
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
