import assert from 'node:assert/strict';
import { readFile, stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import test from 'node:test';
import { gzipSync } from 'node:zlib';
import { INACTIVITY_MS, NAME, SIGN_OFF, WAKE_PHRASE } from '../docs/identity.js';
import { WIDTH, WINDOW } from '../docs/wake.js';
import { MODEL } from '../docs/speaker.js';
import { launchChromium, renderDom } from '../scripts/chromium.mjs';
import { assessSmoke, canvasAspectMatches, clipClearsStage, evidenceRegion, installSmokeMeasurements, smokeLimits, smokeStatusText, smokeViewports } from './smoke-measurements.js';

test('private smoke uses the same authenticated relay transport as the page', async () => {
  const source = await readFile(new URL('../scripts/smoke.mjs', import.meta.url), 'utf8');
  assert.doesNotMatch(source, /bridge-wasm|net\.connect\(4321/);
  assert.match(source, /voice-web', \['token'\]/);
});

test('connection errors identify transport, authentication transport, and token rejection', async () => {
  const source = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  const dial = source.slice(source.indexOf('async function dial()'), source.indexOf('function waitForAnswer'));
  assert.match(dial, /relay transport failed:/);
  assert.match(dial, /authentication transport failed:/);
  assert.match(dial, /token rejected/);
  assert.doesNotMatch(dial, /wrong token/);
});

test('private smoke poses the puppet directly instead of toggling a conversation on the deployed pair', async () => {
  const source = await readFile(new URL('../scripts/smoke.mjs', import.meta.url), 'utf8');
  assert.match(source, /const transitions = mode === 'private' && !live\n\s+\? \["__smokeRuntime\.pose\('stand'\)", "__smokeRuntime\.pose\('sit'\)"\]\n\s+: live \? \['__smokeSay\(__smokeSpeech\.wake\)', '__smokeSay\(__smokeSpeech\.farewell\)'\]\n\s+: \["document\.querySelector\('#puppet'\)\.click\(\)", "document\.querySelector\('#puppet'\)\.click\(\)"\];/);
  assert.equal(source.match(/#puppet'\)\.click\(\)/g).length, 2);
  assert.match(source, /cdp\.evaluate\(`\$\{transitions\[0\]\}/);
  assert.match(source, /cdp\.evaluate\(`\$\{transitions\[1\]\}/);
});

const zeros = (length) => Buffer.from(new Float32Array(length).buffer).toString('base64');
const testWakeModel = { phrase: WAKE_PHRASE, threshold: 0.5, b2: 0, mean: zeros(WINDOW * WIDTH), scale: zeros(WINDOW * WIDTH), w1: zeros(WINDOW * WIDTH), b1: zeros(1), w2: zeros(1) };
const gzipped = (bytes) => [...gzipSync(Uint8Array.from(bytes))];
const mockWasm = `
const gzippedMotions = ${JSON.stringify(Object.fromEntries(['sit.fbx', 'idle.fbx'].map((id) => { const motion = Buffer.from(JSON.stringify({ clip: id })); return [id, { bytes: gzipped(motion), originalSize: motion.length }]; })))};
const gzippedPuppets = ${JSON.stringify(Object.fromEntries(['42', '43', '44'].map((id) => [id, gzipped([1, 2, Number(id) - 39])])))};
const testWakeModel = ${JSON.stringify(testWakeModel)};
const enc = new TextEncoder();
const dec = new TextDecoder();
const queued = [enc.encode(JSON.stringify({ ok: true }))];
const waiting = [];
globalThis.puppetRequests = [];
globalThis.transferOrder = [];
globalThis.telemetryBatches = [];
globalThis.fleetLines = [];
let lost = false;
function deliver(value) {
  if (lost) return;
  const receiver = waiting.shift();
  if (receiver) receiver.resolve(value);
  else queued.push(value);
}
globalThis.deliverRelay = deliver;
globalThis.loseRelay = (error) => {
  lost = error;
  queued.length = 0;
  for (const receiver of waiting.splice(0)) receiver.reject(error);
};
export default async function initWasm() {}
export async function init() {}
export async function connect() { lost = false; }
export async function send_only(bytes) {
  if (globalThis.relayLatency) await new Promise((resolve) => setTimeout(resolve, relayLatency));
  const frame = dec.decode(bytes);
  (globalThis.sentVerbs ??= []).push(frame.split('\\n', 1)[0]);
  if (frame.startsWith('hub-ack\\n')) {
    (globalThis.hubAcks ??= []).push(JSON.parse(frame.slice(8)).stamp);
    (globalThis.hubAckFrames ??= []).push(JSON.parse(frame.slice(8)));
  }
  if (frame.startsWith('spans\\n')) (globalThis.spanBatches ??= []).push(JSON.parse(frame.slice(6)).spans);
  if (frame.startsWith('delegate\\n')) (globalThis.delegateFrames ??= []).push(JSON.parse(frame.slice(9)));
  if (frame.startsWith('telemetry\\n')) {
    const batch = JSON.parse(frame.slice(frame.indexOf('\\n') + 1));
    globalThis.telemetryBatches.push(batch.events);
    globalThis.onTelemetry?.();
    for (const event of batch.events.filter((item) => item.kind === 'error')) globalThis.fleetLines.push('fleet-error: voice/page — ' + event.name + ': ' + event.message);
    deliver(enc.encode('telemetry-ack\\n' + JSON.stringify({ batch_id: batch.batch_id })));
  }
  else if (frame.startsWith('transcript\\n')) {
    const batch = JSON.parse(frame.slice(frame.indexOf('\\n') + 1));
    deliver(enc.encode('transcript-ack\\n' + JSON.stringify({ session_id: batch.session_id, seq: batch.seq })));
  }
  else if (frame.startsWith('audio\\n')) {
    const boundary = frame.indexOf('\\n', 6);
    const chunk = JSON.parse(frame.slice(6, boundary));
    deliver(enc.encode('audio-ack\\n' + JSON.stringify({ session_id: chunk.session_id, side: chunk.side, seq: chunk.seq })));
  }
  else if (frame === 'puppets') {
    const sendCatalog = () => deliver(enc.encode('puppets\\n' + JSON.stringify({ active: '42', avatars: ['42', '43', '44'].map((id) => ({ id, file: 'model-' + id + '.vrm', size: 3, contentHash: 'hash-' + id, creditLine: '', licenseFlags: { creditRequired: false } })) })));
    if (globalThis.holdPuppetCatalog) globalThis.releasePuppetCatalog = sendCatalog;
    else sendCatalog();
  }
  else if (frame === 'clips') deliver(enc.encode('clips\\n' + JSON.stringify({ clips: [{ action: 'sit', name: 'sit.fbx', format: 'fbx', contentHash: 'sit-hash' }, { action: 'idle', name: 'idle.fbx', format: 'fbx', contentHash: 'idle-hash' }] })));
  else if (frame.startsWith('motion\\n')) {
    const request = JSON.parse(frame.slice(frame.indexOf('\\n') + 1));
    const id = request.id;
    globalThis.transferOrder.push(id);
    const hash = request.clipHash;
    const compressed = Uint8Array.from(gzippedMotions[id].bytes);
    deliver(enc.encode('motion-start\\n' + JSON.stringify({ id, size: compressed.length, originalSize: gzippedMotions[id].originalSize, contentHash: hash, encoding: 'gzip' })));
    const prefix = enc.encode('motion-chunk\\n' + id + '\\n');
    const chunk = new Uint8Array(prefix.length + compressed.length);
    chunk.set(prefix);
    chunk.set(compressed, prefix.length);
    deliver(chunk);
    deliver(enc.encode('motion-end\\n' + id));
  }
  else if (frame.startsWith('puppet\\n')) {
    const id = JSON.parse(frame.slice(frame.indexOf('\\n') + 1)).id;
    globalThis.puppetRequests.push(id);
    globalThis.transferOrder.push(id);
    const compressed = Uint8Array.from(gzippedPuppets[id]);
    deliver(enc.encode('puppet-start\\n' + JSON.stringify({ id, size: compressed.length, originalSize: 3, contentHash: 'hash-' + id, encoding: 'gzip' })));
    const prefix = enc.encode('puppet-chunk\\n' + id + '\\n');
    const chunk = new Uint8Array(prefix.length + compressed.length);
    chunk.set(prefix);
    chunk.set(compressed, prefix.length);
    deliver(chunk);
    deliver(enc.encode('puppet-end\\n' + id));
  } else if (frame.startsWith('puppet-select\\n')) {
    const id = JSON.parse(frame.slice(frame.indexOf('\\n') + 1)).id;
    deliver(enc.encode('puppet-selected\\n' + JSON.stringify({ id })));
  }
  else if (frame === 'wake-model' && globalThis.backendWakeModel === 'unreadable') deliver(enc.encode('error\\nwake model unreadable'));
  else if (frame === 'wake-model' && globalThis.backendWakeModel !== 'unanswered') deliver(enc.encode(globalThis.backendWakeModel === null ? 'wake-model-none' : 'wake-model\\n' + JSON.stringify(globalThis.backendWakeModel ?? testWakeModel)));
  else if (frame.startsWith('offer\\n')) {
    const offer = JSON.parse(frame.slice(6));
    globalThis.lastOffer = offer;
    deliver(enc.encode('answer\\n' + JSON.stringify({ offer_id: offer.id, sdp: 'answer' })));
  }
  else if (frame.startsWith('share\\n')) {
    const metadata = JSON.parse(frame.slice(6, frame.indexOf('\\n', 6)));
    globalThis.sentShare = { metadata, size: bytes.length };
    deliver(enc.encode((metadata.text === 'reject' ? 'share-error\\n' + JSON.stringify({ id: metadata.id, message: 'hub queue is full' }) : 'share-ok\\n' + JSON.stringify({ id: metadata.id, stamp: 'share_stamp', summary: 'A link arrived.' }))));
  }
}
export async function recv() {
  if (lost) throw lost;
  if (queued.length) return queued.shift();
  return new Promise((resolve, reject) => waiting.push({ resolve, reject }));
}
`;

const fakePuppet = `
export class PuppetRuntime {
  constructor(canvas) { this.canvas = canvas; this.calls = []; this.humanoidBone = 0; globalThis.testPuppet = this; }
  async load(bytes, initialClip, valid, beforeCommit) { await beforeCommit(); globalThis.firstVisible = { playable: initialClip?.action ?? 'Standing', order: [...transferOrder] }; this.humanoidBone += initialClip?.bytes.byteLength ?? 0; const context = this.canvas.getContext('2d'); context.fillStyle = '#50c878'; context.fillRect(0, 0, this.canvas.width, this.canvas.height); return true; }
  async loadClips(entries) { const before = this.humanoidBone; this.humanoidBone += entries.length; globalThis.clipMovement = { before, after: this.humanoidBone, loaded: entries.map((entry) => [entry.action, entry.format, entry.data]) }; }
  pose(...args) { this.calls.push(['pose', ...args]); }
  gesture(...args) { this.calls.push(['gesture', ...args]); }
  mood(...args) { this.calls.push(['mood', ...args]); }
  waiting(...args) { this.calls.push(['waiting', ...args]); }
  asleep(...args) { this.calls.push(['asleep', ...args]); }
  listening(...args) { this.calls.push(['listening', ...args]); }
  speak(...args) { this.calls.push(['speak', ...args]); }
  start() {}
  pause() {}
  clear() {}
  async attachAudio() {}
  async detachAudio() {}
  dispose() {}
}
`;

const browserSetup = `
globalThis.emitLive = (event) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(event) }));
globalThis.hear = (delta) => emitLive({ type: 'session.input_transcript.delta', delta });
globalThis.delegateTurn = (id) => emitLive({ type: 'session.delegation.created', event_id: 'event_' + id, offset_ms: 0, delegation: { id, type: 'delegation', target: 'client' } });
globalThis.replyFromHub = (id, stamp, commentary = [], timing_ms = 5, extra = {}, image) => {
  const text = new TextEncoder().encode('hub\\n' + JSON.stringify({ id, commentary, timing_ms, stamp, ...extra }) + (image ? '\\n' : ''));
  const frame = new Uint8Array(text.length + (image?.length ?? 0));
  frame.set(text);
  if (image) frame.set(image, text.length);
  deliverRelay(frame);
};
window.addEventListener('error', (event) => { document.body.dataset.browserError = event.message; });
window.addEventListener('unhandledrejection', (event) => { document.body.dataset.browserError = String(event.reason?.stack || event.reason); });
globalThis.__voiceLoadEmbedder = async () => async (texts) => texts.map((text) => {
  const labels = [/yes|correct|agree|ahead/, /know|either|preference/, /reason|consider|moment|think/, /look|notice|detail/, /hello|goodbye|welcome/, /disagree|reject|incorrect/, /laughing|laughed/, /clap|applaud/, /honor|respect/, /thumbs up|endorsement/, /stretch|loosen/, /around|surroundings/, /sorry|forgive|fault|mistake/, /unexpected|astonish|believe/, /hilarious|funny|joke|laugh/, /delight|excellent|wonderful/, /understand|confus|sense/, /convinced|doubt|question/, /considering|think|perhaps|approach/, /careful|warning|danger/, /exhaust|sleep|drowsy|rest/, /wonder|why|learn/];
  const lower = text.toLowerCase();
  const vector = labels.map((pattern) => pattern.test(lower) ? 1 : 0.001);
  return vector;
});
const puppetCache = new Map();
Object.defineProperty(globalThis, 'caches', { value: { open: async () => ({
  match: async (request) => puppetCache.get(request.url)?.clone(),
  put: async (request, response) => puppetCache.set(request.url, response.clone()),
}) } });
globalThis.microphoneOpens = 0;
Object.defineProperty(navigator, 'mediaDevices', { value: Object.assign(new EventTarget(), { getUserMedia: async () => {
  if (globalThis.microphoneMissing) throw new DOMException('Requested device not found', 'NotFoundError');
  if (globalThis.microphoneRefused) throw new DOMException('Permission denied', 'NotAllowedError');
  const track = Object.assign(new EventTarget(), { enabled: true, readyState: 'live', stop() { this.readyState = 'ended'; } });
  globalThis.testMicrophoneTrack = track;
  microphoneOpens++;
  return { getTracks: () => [track], getAudioTracks: () => [track] };
} }) });
globalThis.endMicrophone = () => {
  testMicrophoneTrack.readyState = 'ended';
  testMicrophoneTrack.dispatchEvent(new Event('ended'));
};
class FakeMediaRecorder extends EventTarget {
  static isTypeSupported(type) { return type === 'audio/webm;codecs=opus'; }
  constructor(stream) { super(); this.stream = stream; this.state = 'inactive'; }
  start() {
    if (this.stream.getAudioTracks().every((track) => track.readyState === 'ended')) throw new DOMException("Failed to execute 'start' on 'MediaRecorder': There was an error starting the MediaRecorder.", 'NotSupportedError');
    this.state = 'recording';
  }
  stop() { this.state = 'inactive'; this.dispatchEvent(new Event('stop')); }
}
globalThis.MediaRecorder = FakeMediaRecorder;
class FakeChannel extends EventTarget {
  constructor() { super(); this.readyState = 'open'; }
  send(value) {
    globalThis.sentLiveEvents.push(JSON.parse(value));
  }
  close() { this.readyState = 'closed'; }
}
class FakePeerConnection extends EventTarget {
  constructor() { super(); this.connectionState = 'new'; this.iceGatheringState = 'complete'; this.localDescription = { sdp: 'offer' }; }
  createDataChannel() { this.channel = new FakeChannel(); globalThis.testChannel = this.channel; return this.channel; }
  async createOffer() { return { type: 'offer', sdp: 'offer' }; }
  async setLocalDescription(description) { this.localDescription = description; }
  async setRemoteDescription() {
    const start = () => this.channel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.started', session: { model: 'gpt-live-1', delegation: { type: 'client' } } }) }));
    if (globalThis.holdSessionStart) globalThis.heldSessionStart = start;
    else queueMicrotask(start);
  }
  addTrack(track) {
    (globalThis.sentTracks ??= []).push(track);
    return { replaceTrack: async (next) => { (globalThis.replacedTracks ??= []).push(next); } };
  }
  close() { this.connectionState = 'closed'; this.dispatchEvent(new Event('connectionstatechange')); }
  async getStats() { return new Map(); }
}
globalThis.RTCPeerConnection = FakePeerConnection;
globalThis.gateStarts = [];
const fakeSpeaker = (stream, voiceprint, heard) => {
  const track = Object.assign(new EventTarget(), { gated: true, enabled: true, readyState: 'live', stop() {} });
  const gate = { input: stream, voiceprint: voiceprint && [...voiceprint], heard, closed: 0, stream: { getTracks: () => [track], getAudioTracks: () => [track] }, close() { this.closed++; } };
  gateStarts.push(gate);
  return gate;
};
globalThis.speakerPrepared = 0;
globalThis.__voiceSpeaker = {
  prepareSpeaker: () => { speakerPrepared++; },
  startSpeakerGate: async (stream, voiceprint, heard) => fakeSpeaker(stream, voiceprint, heard),
  learnVoice: async (stream, heard) => { const gate = fakeSpeaker(stream, null, heard); return { close: () => gate.close() }; },
};
globalThis.__voiceStartSpotter = async (stream, heard, model) => {
  globalThis.testSpotter = { stream, heard, model, closed: 0 };
  return { close() { testSpotter.closed++; } };
};
globalThis.sentLiveEvents = [];
window.addEventListener('load', () => {
  if (localStorage.getItem('voice.token')) return;
  const poll = globalThis.setInterval.bind(globalThis);
  globalThis.statusTextWrites = [];
  new MutationObserver(() => statusTextWrites.push(document.querySelector('#status').textContent)).observe(document.querySelector('#status'), { childList: true, characterData: true, subtree: true });
  document.querySelector('#token').value = btoa(JSON.stringify({ endpoint_id: 'test', secret: 'test' }));
  document.querySelector('#connect').click();
  const ready = poll(() => {
    if (document.querySelector('#puppet').getAttribute('aria-disabled') === 'true') return;
    clearInterval(ready);
    if (globalThis.skipTap) return window.dispatchEvent(new Event('test-ready'));
    document.querySelector('#puppet').click();
    const started = poll(() => {
      const status = document.querySelector('#status').textContent;
      if (document.querySelector('#puppet').getAttribute('aria-pressed') !== 'true' && !status.startsWith('conversation could not start:')) return;
      clearInterval(started);
      document.body.dataset.startTest = status || 'puppet';
      window.dispatchEvent(new Event('test-ready'));
    }, 10);
  }, 10);
});
`;

const appendEntries = `
const append = (type, delta) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type, delta }) }));
for (let index = 0; index < 10; index++) {
  append('session.input_transcript.delta', 'question ' + index);
  delegateTurn('scroll_' + index);
}
`;

async function dumpDom(url, args, budget) {
  const chrome = await launchChromium({ args: ['--headless=new', '--no-sandbox', ...args] });
  try {
    return { stdout: await renderDom(chrome, url, { budget }), stderr: chrome.stderr };
  } finally {
    await chrome.close();
  }
}

async function pageServer(testSetup) {
  const index = (await readFile(new URL('../docs/index.html', import.meta.url), 'utf8'))
    .replace('./relay/botq_dash_wasm.js', '/botq_dash_wasm.js')
    .replace('./puppet.js', '/fake-puppet.js')
    .replace('</head>', () => `<script>${browserSetup}${testSetup}</script></head>`);
  const live = await readFile(new URL('../docs/live.js', import.meta.url));
  const puppetClient = await readFile(new URL('../docs/puppet-client.js', import.meta.url));
  const puppetDrivers = await readFile(new URL('../docs/puppet-drivers.js', import.meta.url));
  const requests = [];
  let destinations = 0, destinationArrived = () => {};
  const server = createServer(async (request, response) => {
    const { pathname: path, searchParams } = new URL(request.url, 'http://localhost');
    requests.push(path);
    if (path === '/panel-link-destination') { destinations++; destinationArrived(); response.writeHead(200, { 'content-type': 'text/html', 'cache-control': 'no-store' }); response.end('<!doctype html><script>window.close()</script>'); return; }
    if (path === '/panel-links-loaded') { while (destinations < Number(searchParams.get('count'))) await new Promise((resolve) => { destinationArrived = resolve; }); response.end(); return; }
    const served = path === '/botq_dash_wasm.js' ? mockWasm : path === '/fake-puppet.js' ? fakePuppet : path === '/puppet-client.js' ? puppetClient : path === '/puppet-drivers.js' ? puppetDrivers : path === '/live.js' ? live : path === '/' ? index : await readFile(new URL(`../docs${path}`, import.meta.url)).catch(() => null);
    if (served === null) { response.writeHead(404); response.end(); return; }
    response.writeHead(200, { 'content-type': path.endsWith('.js') ? 'text/javascript' : path.endsWith('.css') ? 'text/css' : path.endsWith('.woff2') ? 'font/woff2' : 'text/html' });
    response.end(served);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { server, requests, url: `http://127.0.0.1:${server.address().port}/` };
}

async function runPage(testSetup = '', { scale = 1, size = '390,844', budget = 3000, mobile = false } = {}) {
  const { server, requests, url } = await pageServer(testSetup);
  try {
    const { stdout, stderr } = await dumpDom(url, [
      '--disable-gpu',
      '--disable-popup-blocking',
      '--disable-background-timer-throttling',
      '--disable-renderer-backgrounding',
      `--force-device-scale-factor=${scale}`,
      `--window-size=${size}`,
      ...(mobile ? ['--user-agent=Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/140.0 Mobile Safari/537.36'] : []),
    ], budget);
    return { stdout, stderr, requests };
  } finally {
    if (server.listening) await new Promise((resolve) => server.close(resolve));
  }
}

async function driveLifecycle(testSetup, drive) {
  const { server, url } = await pageServer(testSetup);
  const chrome = await launchChromium({ args: ['--headless=new', '--no-sandbox', '--disable-gpu'] });
  const { call, listen, closed } = chrome.devtools;
  let ended = false;
  closed.then(() => { ended = true; });
  const events = [];
  const stop = listen(({ method, params }) => {
    if (method === 'Page.lifecycleEvent') events.push(`lifecycle:${params.name}`);
    else if (method === 'Page.frameNavigated' && !params.frame.parentId) events.push(`navigated:${params.type ?? 'Navigation'}`);
    else if (method === 'Inspector.targetCrashed') events.push('crashed');
  });
  try {
    const { targetId } = await call('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
    const page = (method, params) => call(method, params, sessionId);
    await page('Inspector.enable');
    await page('Page.enable');
    await page('Page.setLifecycleEventsEnabled', { enabled: true });
    const evaluate = async (expression) => {
      const { result, exceptionDetails } = await page('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
      return result.value;
    };
    const poll = async (check) => {
      while (!(await check())) {
        if (ended) throw new Error(`Chromium's browser process ended: ${chrome.stderr}`);
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    };
    const until = (expression) => poll(() => evaluate(expression).catch(() => false));
    const event = (name) => poll(() => events.includes(name));
    await page('Page.navigate', { url });
    return await drive({ call, page, evaluate, until, event, events, targetId });
  } finally {
    stop();
    await chrome.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

async function runPuppetPage(html, { scale = 1, size = '390,844', budget = 3000, mobile = false } = {}) {
  const puppet = await readFile(new URL('../docs/puppet.js', import.meta.url));
  const server = createServer((request, response) => {
    const body = request.url === '/puppet.js' ? puppet : html;
    response.writeHead(200, { 'content-type': request.url === '/puppet.js' ? 'text/javascript' : 'text/html' });
    response.end(body);
  });
  try {
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    return await dumpDom(`http://127.0.0.1:${server.address().port}/`, [
      '--use-angle=swiftshader',
      '--enable-unsafe-swiftshader',
      `--force-device-scale-factor=${scale}`,
      `--window-size=${size}`,
      ...(mobile ? ['--user-agent=Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/140.0 Mobile Safari/537.36'] : []),
    ], budget);
  } finally {
    if (server.listening) await new Promise((resolve) => server.close(resolve));
  }
}

function runGazePage() {
  return runPuppetPage(`<!doctype html><canvas id="puppet" style="width:390px;height:844px"></canvas><script type="module">
import { PuppetRuntime } from '/puppet.js';
const runtime = new PuppetRuntime(document.querySelector('#puppet'));
runtime.pause();
runtime.vrm = { lookAt: {} };
runtime.setGaze('panel', 2200, 0);
for (let frame = 0; frame < 30; frame++) runtime.updateGaze(100 + frame * 16);
const panel = { mode: runtime.gazeMode, x: runtime.gazeTarget.position.x };
Math.random = () => 0.5;
for (let frame = 0; frame < 30; frame++) runtime.updateGaze(2201 + frame * 16);
document.body.dataset.gazeTest = JSON.stringify({ panel, returned: { mode: runtime.gazeMode, x: runtime.gazeTarget.position.x } });
runtime.dispose();
</script>`);
}

function runVisibilityPage() {
  return runPuppetPage(`<!doctype html><canvas id="puppet" style="width:390px;height:844px"></canvas><script type="module">
import { PuppetRuntime } from '/puppet.js';
const runtime = new PuppetRuntime(document.querySelector('#puppet'));
runtime.pause();
let renders = 0;
runtime.renderer.render = () => { renders++; };
runtime.animate(0);
runtime.pause();
const beforeAvatar = renders;
runtime.vrm = { update() {} };
runtime.plantFeet = () => {};
runtime.animate(0);
runtime.pause();
const beforePlayable = renders;
runtime.clipAction = { isRunning: () => true, getClip: () => null };
runtime.animate(16);
runtime.pause();
document.body.dataset.visibilityTest = JSON.stringify({ beforeAvatar, beforePlayable, afterPlayable: renders });
runtime.dispose();
</script>`);
}

test('a new puppet runtime rests seated and asleep', async () => {
  const { stdout, stderr } = await runPuppetPage(`<!doctype html><canvas id="puppet"></canvas><script type="module">
import { PuppetRuntime } from '/puppet.js';
const runtime = new PuppetRuntime(document.querySelector('#puppet'));
document.body.dataset.restTest = JSON.stringify({ pose: runtime.poseName, asleep: runtime.sleeping });
runtime.dispose();
</script>`);
  const encoded = /data-rest-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { pose: 'sit', asleep: true }, stderr);
});

test('headless Chromium renders an avatar before its animation clips arrive', async () => {
  const { stdout, stderr } = await runVisibilityPage();
  const encoded = /data-visibility-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { beforeAvatar: 0, beforePlayable: 1, afterPlayable: 2 }, stderr);
});

async function runLayoutPage() {
  const index = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  const style = /<style>[\s\S]*?<\/style>/.exec(index)[0];
  const main = /<main[\s\S]*?<\/main>/.exec(index)[0].replace('class="hidden"', '');
  return runPuppetPage(`<!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1">${style}</head><body>${main}<script type="module">
import { PuppetRuntime } from '/puppet.js';
const canvas = document.querySelector('#puppet');
const heights = [];
const errors = [];
new ResizeObserver(() => heights.push(canvas.clientHeight)).observe(canvas);
window.addEventListener('error', (event) => errors.push(event.message));
const runtime = new PuppetRuntime(canvas);
setTimeout(() => {
  runtime.dispose();
  const settled = canvas.clientHeight;
  canvas.width = 100;
  canvas.height = 1000;
  document.body.dataset.layoutTest = JSON.stringify({ heights, errors, settled, afterBufferChange: canvas.clientHeight });
}, 1500);
</script></body></html>`, { scale: 1.25, size: '1000,700' });
}

test('the puppet canvas keeps one layout height across resize-observer ticks', async () => {
  const { stdout, stderr } = await runLayoutPage();
  const encoded = /data-layout-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result?.heights.length >= 1, stderr);
  assert.deepEqual(result.heights, result.heights.map(() => result.heights[0]), `heights across observer ticks: ${result.heights.join(' ')}`);
  assert.deepEqual(result.errors, []);
  assert.equal(result.settled, result.heights[0], 'canvas must retain its projected height');
  assert.equal(result.afterBufferChange, result.settled, 'layout height must not follow the drawing buffer');
});

const layoutViewports = [
  ...smokeViewports,
  { name: 'minimum-phone', width: 320, height: 568, scale: 2, mobile: true },
  { name: 'minimum-phone-landscape', width: 568, height: 320, scale: 2, mobile: true },
];

test('smoke assessment rejects every measured browser failure and accepts a clean run', () => {
  const clean = { cls: smokeLimits.cumulativeLayoutShift, moves: [], overlaps: [], heights: [{ canvas: 100, stage: 200 }, { canvas: 101, stage: 201 }], aspects: [{ cssWidth: 300, cssHeight: 400, bufferWidth: 600, bufferHeight: 800 }], stages: [{ top: 100, bottom: 500, height: 400, viewportHeight: 500 }], blankFrames: [], errors: [], telemetryRejections: [] };
  assert.equal(assessSmoke(clean).pass, true);
  for (const frameGaps of [[67], [1000, 5000]]) {
    assert.deepEqual(assessSmoke({ ...clean, frameGaps }), assessSmoke(clean));
  }
  for (const mutation of [
    { cls: smokeLimits.cumulativeLayoutShift + 0.001 },
    { moves: [{}] }, { overlaps: [{}] },
    { heights: [{ canvas: 100, stage: 200 }, { canvas: 102, stage: 200 }] },
    { aspects: [{ cssWidth: 330, cssHeight: 400, bufferWidth: 600, bufferHeight: 800 }] },
    { stages: [{ top: 100, bottom: 501, height: 401, viewportHeight: 500 }] },
    { blankFrames: [1] }, { errors: ['fault'] }, { telemetryRejections: ['rejected'] },
  ]) assert.equal(assessSmoke({ ...clean, ...mutation }).pass, false, JSON.stringify(mutation));
});

test('canvas aspect comparison rejects either non-uniform stretch and accepts restoration', () => {
  const original = { cssWidth: 300, cssHeight: 400, bufferWidth: 600, bufferHeight: 800 };
  assert.equal(canvasAspectMatches(original), true);
  assert.equal(canvasAspectMatches({ ...original, cssWidth: 330 }), false);
  assert.equal(canvasAspectMatches({ ...original, cssHeight: 440 }), false);
  assert.equal(canvasAspectMatches(original), true);
});

test('smoke sampling rejects a stretched canvas and passes after browser geometry is restored', async () => {
  const { stdout, stderr } = await runPuppetPage(`<!doctype html><main><canvas id="puppet" width="600" height="800" style="width:300px;height:400px"></canvas></main><script>
  const smokeLimits = ${JSON.stringify(smokeLimits)};
  const canvasAspectMatches = ${canvasAspectMatches.toString()};
  const smoke = (${installSmokeMeasurements.toString()})();
  smoke.sample();
  const canvas = document.querySelector('#puppet');
  canvas.style.width = '330px';
  smoke.sample();
  canvas.style.width = '300px';
  canvas.style.height = '440px';
  smoke.sample();
  canvas.style.height = '400px';
  smoke.sample();
  document.body.dataset.aspectTest = JSON.stringify(smoke.state.aspects);
  </script></body>`, { size: '500,500' });
  const encoded = /data-aspect-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const aspects = JSON.parse(encoded ?? 'null');
  assert.ok(aspects, `${stdout}\n${stderr}`);
  assert.deepEqual(aspects.map(canvasAspectMatches), [true, false, false, true]);
});

test('evidence crops follow the stage canvas rect and clear it at every smoke viewport', () => {
  for (const viewport of smokeViewports) {
    const page = { x: 0, y: 0, width: viewport.width, height: viewport.height };
    const canvas = { left: viewport.width * 0.32, right: viewport.width * 0.68, top: 0, bottom: viewport.height * 0.86 };
    const region = evidenceRegion({ neutralSilhouette: false, canvas, viewport: page });
    assert.ok(region.width > 0 && region.height > 0, viewport.name);
    assert.equal(clipClearsStage(region, canvas), true, `${viewport.name}: ${JSON.stringify(region)}`);
    const shifted = { ...canvas, left: canvas.left + 40, right: canvas.right + 40 };
    const shiftedRegion = evidenceRegion({ neutralSilhouette: false, canvas: shifted, viewport: page });
    assert.notDeepEqual(shiftedRegion, region, `${viewport.name}: crop must track the canvas, not a fixed width`);
    assert.equal(clipClearsStage(shiftedRegion, shifted), true, viewport.name);
  }
});

test('evidence crops stay within the visible band of a scrolled page', () => {
  const viewport = { x: 0, y: 900, width: 390, height: 844 };
  const canvas = { left: 19.5, right: 370.5, top: 930, bottom: 1600 };
  const region = evidenceRegion({ neutralSilhouette: false, canvas, viewport });
  assert.equal(clipClearsStage(region, canvas), true, JSON.stringify(region));
  assert.ok(region.x >= viewport.x && region.x + region.width <= viewport.x + viewport.width, JSON.stringify(region));
  assert.ok(region.y >= viewport.y && region.y + region.height <= viewport.y + viewport.height, JSON.stringify(region));
  const whole = { x: viewport.x, y: viewport.y, width: viewport.width, height: viewport.height };
  for (const offscreen of [{ left: 19.5, right: 370.5, top: 100, bottom: 200 }, { left: -500.5, right: -100.5, top: 930, bottom: 1600 }])
    assert.deepEqual(evidenceRegion({ neutralSilhouette: false, canvas: offscreen, viewport }), whole, JSON.stringify(offscreen));
});

test('a run that did not load the neutral silhouette refuses a full-bleed stage', () => {
  const viewport = { x: 0, y: 0, width: 1440, height: 900 };
  const canvas = { left: 0, right: 1440, top: 0, bottom: 900 };
  assert.throws(() => evidenceRegion({ neutralSilhouette: false, canvas, viewport }), /neutral silhouette/);
  assert.deepEqual(evidenceRegion({ neutralSilhouette: true, canvas, viewport }), viewport);
});

test('one card holds every control: minimized it shows only Mute mic, expanded the rest, and the page has no heading', async () => {
  const index = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  assert.doesNotMatch(index, /<h1\b/i);
  assert.doesNotMatch(index, /token-toggle/);
  const card = /<section id="controls"[\s\S]*?<\/section>\n<\/div>/.exec(index)[0];
  for (const id of ['mic-mute', 'card-toggle', 'voice-filter', 'voice-print', 'puppet-choice', 'share-text', 'share-target', 'share-file', 'share-remove', 'share-send', 'token', 'connect', 'reenter', 'forget']) assert.match(card, new RegExp(`id="${id}"`), id);
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const toggle = document.querySelector('#card-toggle');
  const shown = (selector) => document.querySelector(selector).getClientRects().length > 0;
  const state = () => ({ expanded: toggle.getAttribute('aria-expanded'), mute: shown('#mic-mute'), rest: ['#voice-print', '#puppet-choice', '#share-send', '#forget'].filter(shown).length });
  const states = [state()];
  toggle.click();
  states.push(state());
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
  states.push(state());
  toggle.click();
  toggle.click();
  states.push(state());
  document.body.dataset.cardTest = JSON.stringify(states);
});
`);
  const encoded = /data-card-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const closed = { expanded: 'false', mute: true, rest: 0 };
  assert.deepEqual(JSON.parse(encoded ?? 'null'), [closed, { expanded: 'true', mute: true, rest: 4 }, closed, closed], stderr);
});

test('the live microphone can mute and resume without ending its session', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const button = document.querySelector('#mic-mute');
  const puppet = document.querySelector('#puppet');
  const states = [];
  const capture = () => states.push({ enabled: testMicrophoneTrack.enabled, muted: button.getAttribute('aria-pressed'), label: button.textContent, live: puppet.getAttribute('aria-pressed'), channel: testChannel.readyState });
  capture();
  button.click();
  capture();
  button.click();
  capture();
  document.body.dataset.microphoneMuteTest = JSON.stringify(states);
});
`);
  const encoded = /data-microphone-mute-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), [
    { enabled: true, muted: 'false', label: 'Mute mic', live: 'true', channel: 'open' },
    { enabled: false, muted: 'true', label: 'Unmute mic', live: 'true', channel: 'open' },
    { enabled: true, muted: 'false', label: 'Mute mic', live: 'true', channel: 'open' },
  ], stderr);
});

const voiceprintSetup = (model = MODEL.sha256, off = false) => `localStorage.setItem('voice.token.voiceprint', JSON.stringify({ model: ${JSON.stringify(model)}, print: [0.5, 0.75]${off ? ', off: true' : ''} }));`;

test('without a voiceprint for the current model a conversation sends the microphone itself', async () => {
  const result = await runWakePage(`
    return { sent: sentTracks.at(-1) === testMicrophoneTrack, gates: gateStarts.length, prepared: speakerPrepared, label: document.querySelector('#voice-print').textContent, toggle: document.querySelector('#voice-filter').classList.contains('hidden') };
  `, { setup: voiceprintSetup('an earlier model') });
  assert.deepEqual(result, { sent: true, gates: 0, prepared: 0, label: 'Learn my voice', toggle: true });
});

test('voice ID switches a live session both ways, keeps the voiceprint, and off sends the microphone itself', async () => {
  const result = await runWakePage(`
    const toggle = document.querySelector('#voice-filter');
    const state = () => ({ label: toggle.textContent, pressed: toggle.getAttribute('aria-pressed'), hidden: toggle.classList.contains('hidden'), gates: gateStarts.length, closed: gateStarts.map((gate) => gate.closed), sent: (globalThis.replacedTracks ?? []).at(-1)?.gated ?? null, print: localStorage.getItem('voice.token.voiceprint') !== null, live: document.querySelector('#puppet').getAttribute('aria-pressed') });
    const on = { ...state(), sent: sentTracks.at(-1).gated === true };
    toggle.click();
    await until(() => (globalThis.replacedTracks ?? []).length === 1);
    const off = { ...state(), sent: replacedTracks.at(-1) === testMicrophoneTrack };
    toggle.click();
    await until(() => replacedTracks.length === 2);
    const back = state();
    toggle.click();
    await until(() => replacedTracks.length === 3);
    await sleepNow();
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    const next = { gates: gateStarts.length, sent: sentTracks.at(-1) === testMicrophoneTrack, stored: JSON.parse(localStorage.getItem('voice.token.voiceprint')) };
    return { on, off, back, next };
  `, { setup: voiceprintSetup() });
  assert.deepEqual(result, {
    on: { label: 'Voice ID on', pressed: 'true', hidden: false, gates: 1, closed: [0], sent: true, print: true, live: 'true' },
    off: { label: 'Voice ID off', pressed: 'false', hidden: false, gates: 1, closed: [1], sent: true, print: true, live: 'true' },
    back: { label: 'Voice ID on', pressed: 'true', hidden: false, gates: 2, closed: [1, 0], sent: true, print: true, live: 'true' },
    next: { gates: 2, sent: true, stored: { model: MODEL.sha256, print: [0.5, 0.75], off: true } },
  });
});

test('a voice ID gate that starts late is closed unless it is still wanted, and never outlives its conversation', async () => {
  const result = await runWakePage(`
    const toggle = document.querySelector('#voice-filter');
    const start = __voiceSpeaker.startSpeakerGate;
    const pending = [];
    __voiceSpeaker.startSpeakerGate = (...args) => new Promise((resolve) => pending.push(() => resolve(start(...args))));
    const settle = async () => { pending.shift()(); await new Promise((resolve) => setTimeout(resolve, 50)); };
    toggle.click();
    await until(() => (globalThis.replacedTracks ?? []).length === 1);
    toggle.click();
    toggle.click();
    toggle.click();
    await settle();
    await settle();
    const settled = { closed: gateStarts.map((gate) => gate.closed), sent: replacedTracks.map((track) => track === testMicrophoneTrack ? 'mic' : gateStarts.findIndex((gate) => gate.stream.getAudioTracks()[0] === track)) };
    toggle.click();
    toggle.click();
    await sleepNow();
    await settle();
    return { settled, released: { closed: gateStarts.map((gate) => gate.closed), replaced: replacedTracks.length } };
  `, { setup: voiceprintSetup() });
  assert.deepEqual(result, {
    settled: { closed: [1, 0, 1], sent: ['mic', 'mic', 1] },
    released: { closed: [1, 1, 1, 1], replaced: 4 },
  });
});

test('forgetting the voice during a conversation stops filtering it at once', async () => {
  const result = await runWakePage(`
    document.querySelector('#voice-print').click();
    await until(() => (globalThis.replacedTracks ?? []).length === 1);
    return { closed: gateStarts[0].closed, sent: replacedTracks[0] === testMicrophoneTrack, hidden: document.querySelector('#voice-filter').classList.contains('hidden'), live: document.querySelector('#puppet').getAttribute('aria-pressed') };
  `, { setup: voiceprintSetup() });
  assert.deepEqual(result, { closed: 1, sent: true, hidden: true, live: 'true' });
});

test('voice ID off persists across reloads with no model loaded, and forgetting the voice clears it', async () => {
  const result = await runWakePage(`
    const toggle = document.querySelector('#voice-filter');
    const loaded = { label: toggle.textContent, gates: gateStarts.length, prepared: speakerPrepared, sent: sentTracks.at(-1) === testMicrophoneTrack };
    await sleepNow();
    document.querySelector('#voice-print').click();
    return { loaded, forgotten: { stored: localStorage.getItem('voice.token.voiceprint'), hidden: toggle.classList.contains('hidden'), label: toggle.textContent } };
  `, { setup: voiceprintSetup(MODEL.sha256, true) });
  assert.deepEqual(result, {
    loaded: { label: 'Voice ID off', gates: 0, prepared: 0, sent: true },
    forgotten: { stored: null, hidden: true, label: 'Voice ID on' },
  });
});

test('learning a voice happens between conversations, pauses wake listening and stores only the voiceprint', async () => {
  const result = await runWakePage(`
    document.querySelector('#voice-print').click();
    const refused = document.querySelector('#status').textContent;
    await sleepNow();
    const spotter = testSpotter;
    const before = spotter.closed;
    document.querySelector('#voice-print').click();
    await until(() => gateStarts.length === 1);
    const learning = gateStarts[0];
    const paused = spotter.closed - before;
    learning.heard({ progress: 0.5 });
    const halfway = document.querySelector('#voice-print').textContent;
    learning.heard({ voiceprint: Float32Array.from([0.5, 0.75]) });
    await until(() => testSpotter !== spotter);
    return { refused, input: learning.input.getAudioTracks()[0] === testMicrophoneTrack, learnedFrom: learning.voiceprint, paused, halfway, label: document.querySelector('#voice-print').textContent, closed: learning.closed, stored: JSON.parse(localStorage.getItem('voice.token.voiceprint')), status: document.querySelector('#status').textContent, keys: Object.keys(localStorage).sort() };
  `, { setup: voiceprintSetup('an earlier model', true) });
  assert.deepEqual(result, { refused: 'learn your voice between conversations', input: true, learnedFrom: null, paused: 1, halfway: 'Stop learning (50%)', label: 'Forget my voice', closed: 1, stored: { model: MODEL.sha256, print: [0.5, 0.75] }, status: 'voice learned: other voices are filtered out', keys: ['voice.audio-diagnostics.v1', 'voice.token', 'voice.token.voiceprint'] });
});

test('a voiceprint filters each conversation, mute still silences the microphone, and a filter failure falls back to it', async () => {
  const result = await runWakePage(`
    const gate = gateStarts[0];
    const filtered = { prepared: speakerPrepared, input: gate.input.getAudioTracks()[0] === testMicrophoneTrack, voiceprint: gate.voiceprint, sent: sentTracks.at(-1).gated === true };
    document.querySelector('#mic-mute').click();
    const muted = testMicrophoneTrack.enabled;
    document.querySelector('#mic-mute').click();
    gate.heard({ score: 0.75, from: 0, to: 16000, ms: 90 });
    gate.heard({ error: 'Error: speaker model download failed: 404\\n    at modelBytes' });
    await until(() => (globalThis.replacedTracks ?? []).length && sessionEvents('speaker-score').length);
    const failed = { replaced: replacedTracks[0] === testMicrophoneTrack, closed: gate.closed, status: document.querySelector('#status').textContent, live: document.querySelector('#puppet').getAttribute('aria-pressed') };
    await sleepNow();
    document.querySelector('#puppet').click();
    await until(() => gateStarts.length === 2);
    return { filtered, muted, failed, scores: sessionEvents('speaker-score').map((event) => JSON.parse(event.detail)), next: gateStarts[1].voiceprint };
  `, { setup: voiceprintSetup() });
  assert.deepEqual(result, {
    filtered: { prepared: 1, input: true, voiceprint: [0.5, 0.75], sent: true },
    muted: false,
    failed: { replaced: true, closed: 1, status: 'speaker filter off, hearing everyone: Error: speaker model download failed: 404', live: 'true' },
    scores: [{ score: 0.75, from: 0, to: 16000, ms: 90 }],
    next: [0.5, 0.75],
  });
});

test('forgetting the voice sends the unfiltered microphone from the next conversation', async () => {
  const result = await runWakePage(`
    await sleepNow();
    document.querySelector('#voice-print').click();
    const forgotten = { stored: localStorage.getItem('voice.token.voiceprint'), label: document.querySelector('#voice-print').textContent, status: document.querySelector('#status').textContent };
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    return { ...forgotten, gates: gateStarts.length, sent: sentTracks.at(-1) === testMicrophoneTrack };
  `, { setup: voiceprintSetup() });
  assert.deepEqual(result, { stored: null, label: 'Learn my voice', status: 'voice forgotten: conversations hear everyone', gates: 1, sent: true });
});

test('the stage has no decorative wall occluders', async () => {
  const index = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  assert.doesNotMatch(index, /main::(?:before|after)/);
});

for (const viewport of layoutViewports) test(`stage UI stays outside the puppet projection at ${viewport.name} size`, async () => {
  const index = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  const style = /<style>[\s\S]*?<\/style>/.exec(index)[0];
  const header = /<header>[\s\S]*?<\/header>/.exec(index)[0];
  const main = /<main[\s\S]*?<\/main>/.exec(index)[0].replace('class="hidden"', '');
  const dock = /<div class="dock">[\s\S]*?\n<\/div>/.exec(index)[0];
  const { stdout, stderr } = await runPuppetPage(`<!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1">${style}</head><body>${header}${main}${dock}<script type="module">
  import { PuppetRuntime } from '/puppet.js';
  const stageHeight = Math.round(visualViewport?.height ?? innerHeight);
  document.querySelector('main').style.setProperty('--stage-height', stageHeight + 'px');
  const runtime = new PuppetRuntime(document.querySelector('#puppet'));
  runtime.pause();
  runtime.camera.updateMatrixWorld();
  const origin = runtime.camera.position.clone().set(0, 0, 0);
  const feet = origin.project(runtime.camera);
  runtime.dispose();
  const rect = (element) => { const value = element.getBoundingClientRect(); return { left: value.left, right: value.right, top: value.top, bottom: value.bottom }; };
  const puppet = rect(document.querySelector('#puppet'));
  const reach = [-0.9, 0.9].map((x) => (origin.clone().set(x, 0, 0).project(runtime.camera).x + 1) / 2 * (puppet.right - puppet.left) + puppet.left);
  const figure = { left: reach[0], right: reach[1], top: puppet.top, bottom: puppet.top + (1 - feet.y) / 2 * (puppet.bottom - puppet.top) };
  const selectors = ['header', '#puppet-credit', '#controls', '.display', '.ledger', '.grip[data-pane="ledger"]', '.grip[data-pane="display"]'];
  const overlaps = (a, b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
  const result = selectors.map((selector) => ({ selector, rect: rect(document.querySelector(selector)) })).filter((item) => overlaps(item.rect, figure));
  const regions = selectors.map((selector) => ({ selector, rect: rect(document.querySelector(selector)) })).filter(({ rect }) => rect.right > rect.left && rect.bottom > rect.top);
  const collisions = regions.flatMap((left, index) => regions.slice(index + 1).filter((right) => overlaps(left.rect, right.rect)).map((right) => ({ left, right })));
  const shown = [...document.querySelectorAll('#controls button, #controls select, #controls textarea')].filter((element) => element.getClientRects().length).map((element) => element.id);
  document.body.dataset.overlapTest = JSON.stringify({ puppet, figure, result, collisions, shown, display: rect(document.querySelector('#display')), ledger: rect(document.querySelector('.ledger')), column: rect(document.querySelector('.control')), controls: rect(document.querySelector('#controls')), stage: rect(document.querySelector('main')), pageHeight: document.documentElement.scrollHeight, viewportWidth: innerWidth, stageHeight });
  </script></body></html>`, { scale: viewport.scale, size: `${viewport.width},${viewport.height}`, mobile: viewport.mobile });
  const encoded = /data-overlap-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.deepEqual(result?.result, [], `${viewport.name}: ${JSON.stringify(result)}\n${stderr}`);
  assert.deepEqual(result.collisions, [], `${viewport.name}: ${JSON.stringify(result.collisions)}`);
  assert.deepEqual(result.shown, ['mic-mute', 'card-toggle'], `${viewport.name} minimized card`);
  assert.ok(result.pageHeight <= viewport.height, `${viewport.name} must remain one screen: ${result.pageHeight}`);
  assert.ok(result.controls.left >= 0 && result.controls.right <= result.viewportWidth && result.controls.bottom <= result.stageHeight, `${viewport.name} card leaves the screen: ${JSON.stringify(result.controls)}`);
  assert.ok(result.figure.bottom > result.puppet.top + 0.8 * (result.puppet.bottom - result.puppet.top), `${viewport.name} projected feet must sit near the canvas bottom: ${JSON.stringify(result.figure)}`);
  for (const pane of [result.display, result.ledger]) assert.ok(pane.top >= result.stage.top && pane.bottom <= result.stage.bottom && pane.right - pane.left > 100 && pane.bottom - pane.top > 80, `${viewport.name} pane escapes the stage or collapses: ${JSON.stringify(pane)}`);
  if (!viewport.mobile) {
    assert.ok(result.controls.top >= result.puppet.bottom, `${viewport.name} card intersects the puppet canvas: ${JSON.stringify(result)}`);
    const wing = Math.min(result.ledger.right - result.ledger.left, result.display.right - result.display.left);
    assert.ok(result.puppet.left <= result.column.left - wing && result.puppet.right >= result.column.right + wing && Math.abs(result.puppet.left + result.puppet.right - result.column.left - result.column.right) <= 1, `${viewport.name} canvas must centre on the stage and reach the outer edge of the narrower pane: ${JSON.stringify(result)}`);
  }
});

for (const viewport of layoutViewports) test(`the status line stays stationary across status changes at ${viewport.name} size`, async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const status = document.querySelector('#status');
  const box = () => { const value = document.querySelector('header').getBoundingClientRect(); return [value.left, value.top, value.width, value.height]; };
  const boxes = {};
  for (const [name, text] of Object.entries({ empty: '', short: 'sent', long: ${JSON.stringify(smokeStatusText)}, longer: ${JSON.stringify(smokeStatusText.repeat(3))} })) { status.textContent = text; boxes[name] = box(); }
  document.body.dataset.headerBoxTest = JSON.stringify({ boxes, viewportWidth: innerWidth });
});
`, { scale: viewport.scale, size: `${viewport.width},${viewport.height}`, mobile: viewport.mobile });
  const encoded = /data-header-box-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result, stderr);
  for (const [name, box] of Object.entries(result.boxes)) assert.ok(box[0] >= 0 && box[0] + box[2] <= result.viewportWidth, `${viewport.name} header overflows with ${name} status text: ${JSON.stringify(box)}`);
  for (const [name, box] of Object.entries(result.boxes)) assert.deepEqual(box, result.boxes.empty, `${viewport.name} status line moved with ${name} status text: ${JSON.stringify(result.boxes)}`);
});

for (const [size, axis, grows] of [['1440,900', 'width', { ledger: 'ArrowRight', display: 'ArrowLeft' }], ['390,844', 'height', { ledger: 'ArrowUp', display: 'ArrowUp' }]]) test(`each pane grip resizes its pane by ${axis}, saves the size, restores it on load and resets on double-click at ${size}`, async () => {
  const suffix = axis === 'width' ? 'w' : 'h';
  const { stdout, stderr } = await runPage(`
localStorage.setItem('voice.token.panes', JSON.stringify({ 'ledger-${suffix}': 130, 'display-${suffix}': 140 }));
window.addEventListener('test-ready', () => {
  const result = {};
  for (const [pane, key] of Object.entries(${JSON.stringify(grows)})) {
    const element = document.querySelector('#' + pane);
    const grip = document.querySelector('.grip[data-pane="' + pane + '"]');
    const size = () => Math.round(element.getBoundingClientRect().${axis});
    const restored = size();
    grip.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
    result[pane] = { restored, grown: size(), saved: JSON.parse(localStorage.getItem('voice.token.panes'))[pane + '-${suffix}'] };
  }
  for (const grip of document.querySelectorAll('.grip')) grip.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
  result.stored = localStorage.getItem('voice.token.panes');
  result.orientation = document.querySelector('.grip').getAttribute('aria-orientation');
  document.body.dataset.gripTest = JSON.stringify(result);
});
`, { size });
  const encoded = /data-grip-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { ledger: { restored: 130, grown: 154, saved: 154 }, display: { restored: 140, grown: 164, saved: 164 }, stored: '{}', orientation: axis === 'width' ? 'vertical' : 'horizontal' }, stderr);
});

for (const stored of ['{', 'null', '{"ledger-w":"wide","display-w":99999}']) test(`stored pane sizes ${stored} neither break the page nor escape the pane limits`, async () => {
  const { stdout, stderr } = await runPage(`
localStorage.setItem('voice.token.panes', ${JSON.stringify(stored)});
window.addEventListener('test-ready', () => {
  const width = (selector) => Math.round(document.querySelector(selector).getBoundingClientRect().width);
  const measured = { ledger: width('#ledger'), display: width('#display'), stage: width('.control') };
  measured.limit = innerWidth - 36 - 32 - 200 - measured.ledger;
  document.querySelector('.grip[data-pane="display"]').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
  measured.saved = JSON.parse(localStorage.getItem('voice.token.panes'))['display-w'];
  measured.full = document.querySelector('#display').classList.contains('full');
  document.body.dataset.storedTest = JSON.stringify(measured);
});
`, { size: '1440,900' });
  const encoded = /data-stored-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result, stderr);
  const huge = stored.includes('99999');
  assert.equal(result.ledger, Math.round(1440 * .22));
  assert.equal(result.display, huge ? result.limit : Math.round(1440 * .24));
  assert.ok(result.stage >= 200, JSON.stringify(result));
  assert.equal(result.saved, huge ? result.limit : Math.round(1440 * .24) + 24);
  assert.equal(result.full, huge);
});

for (const [size, grow, shrink] of [['1440,900', 'ArrowRight', 'ArrowLeft'], ['390,844', 'ArrowUp', 'ArrowDown']]) test(`a pane grows past its default to full screen and comes back by button, Escape and grip at ${size}`, async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const ledger = document.querySelector('#ledger');
  const grip = document.querySelector('.grip[data-pane="ledger"]');
  const button = document.querySelector('.pane-full[data-pane="ledger"]');
  const box = () => { const rect = ledger.getBoundingClientRect(); return Math.round(rect.width * rect.height); };
  const state = () => ({ full: ledger.classList.contains('full'), pressed: button.getAttribute('aria-pressed'), label: button.textContent });
  const initial = box();
  button.click();
  const on = (selector) => { const rect = document.querySelector(selector).getBoundingClientRect(); return document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2)?.closest(selector) !== null; };
  const buttonFull = { ...state(), covers: box() > innerWidth * innerHeight * .8, mute: on('#mic-mute'), exit: on('.pane-full[data-pane="ledger"]') };
  button.click();
  const buttonBack = { ...state(), restored: box() === initial };
  button.click();
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
  const escaped = { ...state(), card: document.querySelector('#controls').classList.contains('open') };
  let steps = 0;
  while (!ledger.classList.contains('full') && steps < 100) { grip.dispatchEvent(new KeyboardEvent('keydown', { key: '${grow}', bubbles: true })); steps++; }
  const grown = { ...state(), grewPastDefault: steps > 2 };
  grip.dispatchEvent(new KeyboardEvent('keydown', { key: '${shrink}', bubbles: true }));
  const stage = document.querySelector('.control').getBoundingClientRect(), dock = document.querySelector('#controls').getBoundingClientRect(), rect = ledger.getBoundingClientRect();
  const gripBack = { ...state(), larger: box() > initial, stageKept: Math.round(innerWidth > 720 ? stage.width : stage.height) >= (innerWidth > 720 ? 200 : 120), clearOfDock: innerWidth > 720 || rect.bottom <= dock.top };
  document.body.dataset.fullTest = JSON.stringify({ buttonFull, buttonBack, escaped, grown, gripBack });
});
`, { size });
  const encoded = /data-full-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const out = { full: false, pressed: 'false', label: 'Full screen' };
  const inn = { full: true, pressed: 'true', label: 'Exit full screen' };
  assert.deepEqual(JSON.parse(encoded ?? 'null'), {
    buttonFull: { ...inn, covers: true, mute: true, exit: true },
    buttonBack: { ...out, restored: true },
    escaped: { ...out, card: false },
    grown: { ...inn, grewPastDefault: true },
    gripBack: { ...out, larger: true, stageKept: true, clearOfDock: true },
  }, stderr);
});

test('Android DPR 3 keeps the visual stage height stable and renders after sixty seconds', async () => {
  const { stdout, stderr } = await runPage(`
  const heights = [];
  window.addEventListener('load', () => new ResizeObserver(() => heights.push(document.querySelector('#puppet').clientHeight)).observe(document.querySelector('#puppet')));
  window.addEventListener('test-ready', () => {
    const initial = document.querySelector('#puppet').clientHeight;
  setTimeout(() => {
      const canvas = document.querySelector('#puppet');
      const pixel = canvas.getContext('2d').getImageData(Math.floor(canvas.width / 2), Math.floor(canvas.height / 2), 1, 1).data;
      document.body.dataset.androidTest = JSON.stringify({ viewportHeight: Math.round(visualViewport.height), stageHeight: document.querySelector('#conversation').clientHeight, heights, initial, settled: canvas.clientHeight, loaded: testPuppet.humanoidBone > 0, visible: pixel[3] > 0 });
  }, 60000);
  });
  `, { scale: 3, size: '390,844', budget: 65000, mobile: true });
  const encoded = /data-android-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result?.loaded && result.visible, `${JSON.stringify(result)}\n${stderr}`);
  assert.equal(result.settled, result.initial);
  assert.ok(result.stageHeight > 0 && result.stageHeight <= result.viewportHeight);
  assert.deepEqual(result.heights, result.heights.map(() => result.settled));
});

test('the real page runtime moves its look-at target to the panel and back over time', async () => {
  const { stdout, stderr } = await runGazePage();
  const encoded = /data-gaze-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.equal(result?.panel.mode, 'panel', stderr);
  assert.ok(result.panel.x > 2.5, stderr);
  assert.equal(result.returned.mode, 'camera', stderr);
  assert.ok(result.returned.x < 0.3, stderr);
});

test('tapping the puppet starts standing and tapping it again stops sitting', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  document.querySelector('#puppet').click();
  setTimeout(() => { document.body.dataset.puppetToggleTest = JSON.stringify(testPuppet.calls.filter(([name]) => name === 'pose').map(([, pose]) => pose)); }, 30);
});
`);
  const observed = /data-start-test="([^"]*)"/.exec(stdout)?.[1] ?? 'start path did not settle';
  assert.equal(observed, 'puppet', `${observed}\n${stderr}`);
  const encoded = /data-puppet-toggle-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), ['stand', 'listen', 'sit'], stderr);
});

for (const viewport of layoutViewports) test(`puppet states have no duplicate text at ${viewport.name} size`, async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const text = () => document.body.innerText;
  const listening = text();
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.input_transcript.delta', delta: 'Where is the report?' }) }));
  delegateTurn('state_1');
  const waiting = text();
  replyFromHub('state_1', 'state_stamp', ['On the display.'], 12);
  setTimeout(() => {
    const answered = text();
    document.querySelector('#puppet').click();
    setTimeout(() => { document.body.dataset.stateTextTest = JSON.stringify({ states: { listening, waiting, answered, ended: text() }, statusTextWrites }); }, 30);
  }, 30);
});
`, { scale: viewport.scale, size: `${viewport.width},${viewport.height}`, mobile: viewport.mobile });
  const encoded = /data-state-text-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result, stderr);
  const states = result.states;
  const removed = ['ready', 'opening microphone', 'live', 'conversation off', 'session ended', 'listening', 'waiting for the hub', 'asleep', 'awake', 'sleep', 'woken'];
  for (const [state, text] of Object.entries(states)) {
    for (const value of removed) assert.equal(text.toLowerCase().includes(value.toLowerCase()), false, `${viewport.name} ${state} exposes ${value}: ${text}`);
  }
  for (const text of result.statusTextWrites) {
    for (const value of removed) assert.equal(text.toLowerCase().includes(value.toLowerCase()), false, `${viewport.name} header exposed ${value}: ${text}`);
  }
  assert.match(states.waiting, /sent to hub: Where is the report\?/);
  assert.match(states.waiting, /hub reply: waiting…/);
  assert.match(states.answered, /sent to hub: Where is the report\?[\s\S]*hub reply: On the display\./);
});

test('a forced page error reaches the fleet catcher line in headless Chromium', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  setTimeout(() => { throw new TypeError('forced page fault'); }, 0);
  setTimeout(() => { document.body.dataset.telemetryErrorTest = JSON.stringify(fleetLines); }, 600);
});
`);
  const encoded = /data-telemetry-error-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), ['fleet-error: voice/page — TypeError: forced page fault'], stderr);
});

test('a forced page error carries browser and WebGPU identity', async () => {
  const { stdout, stderr } = await runPage(`
Object.defineProperty(navigator, 'gpu', { value: undefined });
window.addEventListener('test-ready', () => {
  setTimeout(() => { throw new TypeError('identified page fault'); }, 0);
  setTimeout(() => { document.body.dataset.telemetryIdentityTest = JSON.stringify(telemetryBatches.flat().find((event) => event.message === 'identified page fault')); }, 600);
});
`);
  const encoded = /data-telemetry-identity-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const event = JSON.parse(encoded ?? 'null');
  assert.ok(event, stderr);
  assert.match(event.user_agent, /Chrome/);
  assert.equal(event.webgpu_adapter, false);
});

test('a page error is reported while the WebGPU probe is still pending', async () => {
  const { stdout, stderr } = await runPage(`
Object.defineProperty(navigator, 'gpu', { value: { requestAdapter: () => new Promise(() => {}) } });
window.addEventListener('test-ready', () => {
  setTimeout(() => { throw new TypeError('unprobed page fault'); }, 0);
  setTimeout(() => { document.body.dataset.telemetryPendingProbeTest = JSON.stringify(telemetryBatches.flat().find((event) => event.message === 'unprobed page fault')); }, 600);
});
`);
  const encoded = /data-telemetry-pending-probe-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const event = JSON.parse(encoded ?? 'null');
  assert.ok(event, stderr);
  assert.match(event.user_agent, /Chrome/);
  assert.equal(event.webgpu_adapter, null);
});

test('session open and close arrive as two batched telemetry events', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  document.querySelector('#puppet').click();
  globalThis.onTelemetry = () => {
    const names = telemetryBatches.flat().filter((event) => event.kind === 'session').map((event) => event.name);
    if (names.includes('close')) document.body.dataset.telemetrySessionTest = JSON.stringify(names);
  };
});
`);
  const encoded = /data-telemetry-session-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), ['live-config-sdp-answer', 'live-config-session-started', 'open', 'close'], stderr);
});

test('a fresh page shows the active puppet at rest before anything taps it', async () => {
  const { stdout, stderr } = await runPage(`
globalThis.skipTap = true;
window.addEventListener('test-ready', () => {
  const poll = setInterval(() => {
    if (!globalThis.clipMovement) return;
    clearInterval(poll);
    document.body.dataset.bootTest = JSON.stringify({ requests: puppetRequests, prompt: !document.querySelector('#puppet-prompt').classList.contains('hidden'), pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), calls: testPuppet.calls });
  }, 10);
});
`);
  const encoded = /data-boot-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { requests: ['42'], prompt: false, pressed: 'false', calls: [] }, stderr);
});

test('a late avatar catalog loads once after the voice session is already open', async () => {
  const { stdout, stderr } = await runPage(`
globalThis.holdPuppetCatalog = true;
window.addEventListener('test-ready', () => {
  const before = { pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), transfers: [...transferOrder], spotter: Boolean(testSpotter) };
  releasePuppetCatalog();
  setTimeout(() => { document.body.dataset.lateAvatarTest = JSON.stringify({ before, requests: puppetRequests, pressed: document.querySelector('#puppet').getAttribute('aria-pressed') }); }, 100);
});
`);
  const encoded = /data-late-avatar-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { before: { pressed: 'true', transfers: [], spotter: true }, requests: ['42'], pressed: 'true' }, stderr);
});

test('the page loads the active puppet and its clips without fetching inactive puppets', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => setTimeout(() => {
  document.body.dataset.initialPuppetTest = JSON.stringify({ requests: puppetRequests, cacheKeys: [...puppetCache.keys()].map((url) => new URL(url).pathname.split('/').at(-1)), clipMovement, firstVisible: { ...firstVisible, order: firstVisible.order.slice(0, 1) } });
}, 100));
`);
  const encoded = /data-initial-puppet-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { requests: ['42'], cacheKeys: ['hash-42.vrm', 'sit-hash.motion', 'idle-hash.motion'], clipMovement: { before: 0, after: 2, loaded: [['sit', 'motion', { clip: 'sit.fbx' }], ['idle', 'motion', { clip: 'idle.fbx' }]] }, firstVisible: { playable: 'Standing', order: ['42'] } }, stderr);
});

test('a first switch to an unseen puppet sits right after its first render, with no relay traffic between, and the selector stays live', async () => {
  const { stdout, stderr } = await runPage(`
globalThis.relayLatency = 500;
window.addEventListener('test-ready', function switchOnceSeated() {
  if (!globalThis.clipMovement) return setTimeout(switchOnceSeated, 10);
  const choice = document.querySelector('#puppet-choice');
  let disabled = false;
  new MutationObserver(() => { disabled ||= choice.disabled; }).observe(choice, { attributes: true });
  const booted = firstVisible;
  const seated = clipMovement;
  choice.value = '43';
  choice.dispatchEvent(new Event('change'));
  let rendered;
  const poll = setInterval(() => {
    if (!rendered && firstVisible !== booted) rendered = { at: performance.now(), sent: sentVerbs.length };
    if (!rendered || clipMovement === seated) return;
    clearInterval(poll);
    document.body.dataset.firstSwitchTest = JSON.stringify({ renderToSitMs: Math.round(performance.now() - rendered.at), sentBetween: sentVerbs.slice(rendered.sent), disabled, selected: choice.value });
  }, 4);
});
`, { budget: 12000 });
  const encoded = /data-first-switch-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result, stderr);
  assert.deepEqual(result.sentBetween, [], stderr);
  assert.ok(result.renderToSitMs <= 100, `render to sitting took ${result.renderToSitMs} ms`);
  assert.equal(result.disabled, false);
  assert.equal(result.selected, '43');
});

test('choosing again mid-load supersedes the pending puppet without an error', async () => {
  const { stdout, stderr } = await runPage(`
globalThis.relayLatency = 200;
window.addEventListener('test-ready', function switchOnceSeated() {
  if (!globalThis.clipMovement) return setTimeout(switchOnceSeated, 10);
  const choice = document.querySelector('#puppet-choice');
  choice.value = '43';
  choice.dispatchEvent(new Event('change'));
  setTimeout(() => {
    choice.value = '44';
    choice.dispatchEvent(new Event('change'));
  }, 50);
  const poll = setInterval(() => {
    if (clipMovement.after < 4) return;
    clearInterval(poll);
    setTimeout(() => { document.body.dataset.supersedeTest = JSON.stringify({ selected: choice.value, status: document.querySelector('#status').textContent, requests: puppetRequests, seated: clipMovement.after }); }, 1000);
  }, 10);
});
`, { budget: 12000 });
  const encoded = /data-supersede-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { selected: '44', status: '', requests: ['42', '43', '44'], seated: 4 }, stderr);
});

test('choosing an inactive puppet fetches only that puppet on demand', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const choice = document.querySelector('#puppet-choice');
  choice.value = '43';
  choice.dispatchEvent(new Event('change'));
  setTimeout(() => { document.body.dataset.selectionTest = JSON.stringify({ requests: puppetRequests, selected: choice.value, disabled: choice.disabled, options: [...choice.options].map((option) => [option.value, option.textContent]) }); }, 100);
});
`);
  const encoded = /data-selection-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { requests: ['42', '43'], selected: '43', disabled: false, options: [['42', 'model-42.vrm'], ['43', 'model-43.vrm'], ['44', 'model-44.vrm']] }, stderr);
});

test('authenticated text box sends a URL verbatim and informs an open Live session', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  document.querySelector('#share-text').value = 'https://example.test/a?q=one';
  document.querySelector('#share-send').click();
  setTimeout(() => { document.body.dataset.shareTest = JSON.stringify({ sent: globalThis.sentShare?.metadata?.text, status: document.querySelector('#status').textContent, notices: sentLiveEvents.filter((event) => event.event_id?.startsWith('share_')).map((event) => event.content ?? event.type) }); }, 30);
});
`);
  const encoded = /data-share-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { sent: 'https://example.test/a?q=one', status: 'sent', notices: ['A link arrived.'] }, stderr);
});

test('a rejected submission immediately restores its controls and reports the reason', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  document.querySelector('#share-text').value = 'reject';
  document.querySelector('#share-send').click();
  setTimeout(() => { document.body.dataset.shareErrorTest = JSON.stringify({ status: document.querySelector('#status').textContent, disabled: document.querySelector('#share-send').disabled }); }, 30);
});
`);
  const encoded = /data-share-error-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { status: 'hub queue is full', disabled: false }, stderr);
});

test('a display payload appears newest first and points the puppet while a plain hub reply adds nothing', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  replyFromHub('unknown', 'display_1', [], 1, { display: { markdown: '**Result** details', link: 'https://example.test/result', image: { mime: 'image/png' } } }, Uint8Array.from([137,80,78,71,13,10,26,10]));
  replyFromHub('unknown', 'display_2', [], 1, { display: { markdown: 'Newest' } });
  replyFromHub('unknown', undefined, ['plain'], 1);
  setTimeout(() => { document.body.dataset.displayTest = JSON.stringify({ count: document.querySelectorAll('.display-item').length, first: document.querySelector('.display-item')?.textContent, link: document.querySelector('.display-item:last-child a')?.href, image: Boolean(document.querySelector('.display-item:last-child img')), pointed: testPuppet.calls.some((call) => call[0] === 'gesture' && call[1] === 'point' && call[2] === 'panel') }); }, 40);
});
`);
  const encoded = /data-display-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { count: 2, first: 'Newest', link: 'https://example.test/result', image: true, pointed: true }, stderr);
});

for (const size of ['720,1280', '1440,900']) test(`the delegation log keeps its newest entry in view at ${size}`, async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  ${appendEntries}
  const log = document.querySelector('#log');
  log.dispatchEvent(new Event('scroll'));
  append('session.input_transcript.delta', 'newest');
  document.body.dataset.followTest = JSON.stringify({ overflow: log.scrollHeight - log.clientHeight, below: log.scrollHeight - log.scrollTop - log.clientHeight });
});
`, { size });
  const encoded = /data-follow-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result?.overflow > 1 && result.below <= 1, `${size}: ${encoded}\n${stderr}`);
});

test('the delegation log renders every trace kind and delegation field', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const emit = (event) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(event) }));
  emit({ type: 'session.output_transcript.delta', delta: 'I will inspect the fixture.', start_ms: 0, end_ms: 20 });
  emit({ type: 'session.input_transcript.delta', delta: 'Check the fixture stream.' });
  delegateTurn('fixture');
  replyFromHub('fixture', 'fixture', ['The fixture is complete.'], 42);
  setTimeout(() => { document.body.dataset.delegationLogTest = document.querySelector('#log').innerText; }, 30);
});
`, { size: '1440,900' });
  const observed = /data-delegation-log-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"') ?? '';
  const expected = ['MODEL ALONE', 'spoke: I will inspect the fixture.', 'MODEL HEARD', 'heard: Check the fixture stream.', 'DELEGATED', 'sent to hub: Check the fixture stream.', 'context sent: [{"speaker":"live","text":"I will inspect the fixture."}]', 'hub reply: The fixture is complete.', 'hub timing: 42 ms'];
  for (const value of expected) assert.ok(observed.includes(value), `${value}\n${observed}\n${stderr}`);
});

test('the delegation log does not move when an entry arrives after scrolling to the top', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  ${appendEntries}
  const log = document.querySelector('#log');
  log.scrollTop = 0;
  log.dispatchEvent(new Event('scroll'));
  const before = log.scrollTop;
  append('session.input_transcript.delta', 'newest');
  delegateTurn('scroll_newest');
  document.body.dataset.pauseTest = String(log.scrollTop === before);
});
`);
  const observed = /data-pause-test="([^"]*)"/.exec(stdout)?.[1] ?? 'pause test did not run';
  assert.equal(observed, 'true', `${observed}\n${stderr}`);
});

test('a delegated turn reaches the relay as one trace whose delegate span is its traceparent', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const pause = () => new Promise((resolve) => setTimeout(resolve, 30));
  hear('Check the weather.');
  delegateTurn('dlg');
  await pause();
  replyFromHub('dlg', 'stamp_1', ['Sunny.']);
  await pause();
  emitLive({ type: 'session.output_transcript.delta', delta: 'Sunny.' });
  await new Promise((resolve) => setTimeout(resolve, 200));
  document.body.dataset.stageTest = JSON.stringify({ batches: globalThis.spanBatches ?? [], delegates: globalThis.delegateFrames ?? [] });
});
`);
  const encoded = /data-stage-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const { batches, delegates } = JSON.parse(encoded ?? 'null') ?? {};
  const spans = batches?.flat() ?? [];
  assert.deepEqual(spans.map((span) => span.name).sort(), ['await-speech', 'decide', 'delegate', 'hear', 'turn'], stderr);
  assert.ok(spans.every((span) => span.traceId === spans[0].traceId && span.status === undefined));
  const delegated = spans.find((span) => span.name === 'delegate');
  assert.deepEqual(delegates.map(({ id, traceparent }) => [id, traceparent]), [['dlg', `00-${delegated.traceId}-${delegated.spanId}-01`]]);
});

test('a client delegation hands the hub the exact transcript heard since the last delegation and the visible turns before it', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  hear('Hello there.');
  delegateTurn('item_first');
  emitLive({ type: 'session.output_transcript.delta', delta: 'Hi.', start_ms: 0, end_ms: 10 });
  hear("What's in ");
  hear('the build ');
  hear('queue right now?');
  delegateTurn('item_exact');
  await new Promise((resolve) => setTimeout(resolve, 50));
  document.body.dataset.handoffTest = JSON.stringify(globalThis.delegateFrames ?? []);
});
`);
  const frames = JSON.parse(/data-handoff-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"') ?? 'null');
  assert.ok(frames, stderr);
  assert.deepEqual(frames.map(({ id, text, context }) => ({ id, text, context })), [
    { id: 'item_first', text: 'Hello there.', context: [] },
    { id: 'item_exact', text: "What's in the build queue right now?", context: [{ speaker: 'user', text: 'Hello there.' }, { speaker: 'live', text: 'Hi.' }] },
  ]);
  assert.ok(frames.every((frame) => Number.isFinite(frame.duration_ms) && /^00-[0-9a-f]{32}-[0-9a-f]{16}-01$/.test(frame.traceparent)));
});

test('a delegation appends nothing until its hub reply, which reaches Live as commentary alone, while a display-only push says nothing', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const pause = () => new Promise((resolve) => setTimeout(resolve, 30));
  const told = () => sentLiveEvents.filter(({ type, event_id }) => type.endsWith('.append') && !['identity', 'wake'].includes(event_id)).map(({ type, event_id, delegation_id, content }) => [type, event_id, delegation_id, content]);
  hear('How many jobs are queued?');
  delegateTurn('slow');
  await pause();
  const closed = told();
  replyFromHub('slow', 'push_1');
  await pause();
  const pushed = { told: told().length - closed.length, waiting: testPuppet.calls.filter(([name]) => name === 'waiting').map(([, value]) => value) };
  replyFromHub('slow', 'reply_1', ['Four jobs are queued.'], 180000);
  await pause();
  deliverRelay(new TextEncoder().encode('hub-error\\n' + JSON.stringify({ id: 'slow', message: 'invalid hub reply' })));
  hear('Is the printer busy?');
  delegateTurn('lost');
  await pause();
  deliverRelay(new TextEncoder().encode('hub-error\\n' + JSON.stringify({ id: 'lost', message: 'delegation queue is full' })));
  await pause();
  document.body.dataset.asyncHubTest = JSON.stringify({ closed, pushed, told: told(), acks: globalThis.hubAcks ?? [], waiting: testPuppet.calls.filter(([name]) => name === 'waiting').map(([, value]) => value), log: document.querySelector('#log').innerText });
});
`);
  const encoded = /data-async-hub-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result, stderr);
  const shape = (events) => events.map(([type, event_id, delegation_id]) => [type, event_id, delegation_id]);
  assert.deepEqual(result.closed, []);
  assert.deepEqual(result.pushed, { told: 0, waiting: [true] });
  assert.deepEqual(shape(result.told), [
    ['session.commentary.append', 'hub_reply_1', 'slow'],
    ['session.commentary.append', 'hub_error_lost', 'lost'],
  ]);
  assert.equal(result.told[0][3], 'Four jobs are queued.');
  assert.equal(result.told[1][3], 'The hub request failed.');
  assert.match(result.log, /Is the printer busy\?[\s\S]*delegation queue is full/);
  assert.deepEqual(result.acks, ['push_1', 'reply_1']);
  assert.deepEqual(result.waiting, [true, false, true, false]);
  assert.match(result.log, /sent to hub: How many jobs are queued\?[\s\S]*hub reply: Four jobs are queued\./);
});

test('while a delegation is pending, a turn the model ignores for three seconds goes to the hub, and its reply or failure carries a null delegation id', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const frames = () => (globalThis.delegateFrames ?? []).map(({ id, text }) => ({ id, text }));
  hear('Hello there.');
  await wait(3100);
  const nothingPending = frames().length;
  delegateTurn('item_first');
  hear(' How are you?');
  await wait(1000);
  emitLive({ type: 'session.output_transcript.delta', delta: 'Fine.', start_ms: 0, end_ms: 10 });
  await wait(2500);
  const afterSpeech = frames().length;
  hear(' Also, is the printer');
  await wait(2000);
  hear(' busy right now?');
  await wait(2900);
  const beforeTimeout = frames().length;
  await wait(200);
  const printer = frames().at(-1).id;
  hear(' And the backup?');
  await wait(1000);
  delegateTurn('item_backup');
  hear('?');
  hear(' ');
  await wait(3100);
  const afterModelDelegation = frames().length;
  replyFromHub(printer, 'printer', ['The printer is idle.']);
  hear(' Is the door locked?');
  await wait(3100);
  const door = frames().at(-1).id;
  deliverRelay(new TextEncoder().encode('hub-error\\n' + JSON.stringify({ id: door, message: 'delegation queue is full' })));
  await wait(50);
  document.body.dataset.forwardTest = JSON.stringify({ nothingPending, afterSpeech, beforeTimeout, afterModelDelegation, door, frames: frames(), told: sentLiveEvents.filter(({ event_id }) => event_id === 'hub_printer' || event_id?.startsWith('hub_error_')).map(({ type, event_id, delegation_id, content }) => [type, event_id, delegation_id, content]) });
});
`, { budget: 20000 });
  const result = JSON.parse(/data-forward-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"') ?? 'null');
  assert.ok(result, stderr);
  assert.deepEqual([result.nothingPending, result.afterSpeech, result.beforeTimeout, result.afterModelDelegation], [0, 1, 1, 3]);
  assert.deepEqual(result.frames.map(({ id, text }) => [id.startsWith('turn_') ? 'page' : id, text]), [
    ['item_first', 'Hello there.'],
    ['page', 'How are you?\nAlso, is the printer busy right now?'],
    ['item_backup', 'And the backup?'],
    ['page', '?  Is the door locked?'],
  ]);
  assert.deepEqual(result.told, [
    ['session.commentary.append', 'hub_printer', null, 'The printer is idle.'],
    ['session.commentary.append', 'hub_error_' + result.door, null, 'The hub request failed.'],
  ]);
});

test('a hub reply appends each part as commentary, with a delegation ID only when it answers one', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const pause = () => new Promise((resolve) => setTimeout(resolve, 30));
  hear('How many jobs need attention?');
  delegateTurn('item_mix');
  await pause();
  const replies = {};
  for (const [id, commentary] of [['item_mix', ['Two jobs need attention.', 'Both are on the display.']], ['share_1', ['The link is the release notes.']]]) {
    const before = sentLiveEvents.length;
    replyFromHub(id, id, commentary);
    await pause();
    replies[id] = sentLiveEvents.slice(before).map(({ type, event_id, delegation_id, content }) => [type, event_id, delegation_id, content]);
    emitLive({ type: 'session.output_transcript.delta', delta: commentary[0] });
    await pause();
  }
  document.body.dataset.channelTest = JSON.stringify(replies);
});
`);
  const replies = JSON.parse(/data-channel-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"') ?? 'null');
  assert.ok(replies, stderr);
  assert.deepEqual(replies, {
    item_mix: [
      ['session.commentary.append', 'hub_item_mix', 'item_mix', 'Two jobs need attention.'],
      ['session.commentary.append', 'hub_item_mix_1', 'item_mix', 'Both are on the display.'],
    ],
    share_1: [['session.commentary.append', 'hub_share_1', null, 'The link is the release notes.']],
  });
});

test('output transcript drives mood and delegation drives the waiting pose', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const event = (value) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(value) }));
  event({ type: 'session.output_transcript.delta', delta: 'Sorry, that was my fault.' });
  event({ type: 'session.input_transcript.delta', delta: 'check it' });
  delegateTurn('wait_1');
  setTimeout(() => { document.body.dataset.driverTest = JSON.stringify(testPuppet.calls.filter(([name]) => name === 'mood' || name === 'waiting' || name === 'speak').map(([name, value]) => [name, value])); }, 20);
});
`);
  const encoded = /data-driver-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), [['speak', 'Sorry, that was my fault.'], ['waiting', true], ['mood', 'apologetic']], stderr);
});

test('synthetic input activity drives a bounded listening envelope without a clip gesture', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const event = (type) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type }) }));
  event('input_audio_buffer.speech_started');
  setTimeout(() => event('input_audio_buffer.speech_stopped'), 2350);
  setTimeout(() => {
    const listening = testPuppet.calls.filter(([name]) => name === 'listening').map(([, motion]) => motion);
    document.body.dataset.listeningTest = JSON.stringify({ peak: Math.max(...listening.map(({ amount }) => amount)), nodded: listening.some(({ nod }) => Math.abs(nod) > 0.02), tilted: listening.some(({ tilt }) => Math.abs(tilt) > 0.01), final: listening.at(-1), gestures: testPuppet.calls.filter(([name]) => name === 'gesture') });
  }, 2850);
});
`, { budget: 4000 });
  const encoded = /data-listening-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { peak: 1, nodded: true, tilted: true, final: { amount: 0, lean: 0, nod: 0, tilt: 0 }, gestures: [] }, stderr);
});

test('output transcript can reach a catalog clip action', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.output_transcript.delta', delta: 'Let us applaud that achievement.' }) }));
  setTimeout(() => { document.body.dataset.catalogActionTest = JSON.stringify(testPuppet.calls.filter(([name]) => name === 'gesture')); }, 20);
});
`);
  const encoded = /data-catalog-action-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), [['gesture', 'clap', null]], stderr);
});

test('a classified point is dropped until the display has a target', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.output_transcript.delta', delta: 'Look at the important detail.' }) }));
  setTimeout(() => { document.body.dataset.pointTest = JSON.stringify(testPuppet.calls.filter(([name]) => name === 'gesture')); }, 20);
});
`);
  const encoded = /data-point-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), [], stderr);
});

async function libBytes(paths) {
  const sizes = await Promise.all(paths.filter((path) => path.startsWith('/lib/')).map(async (path) => (await stat(new URL(`../docs${path}`, import.meta.url))).size));
  return sizes.reduce((total, size) => total + size, 0);
}

test('a display renders a mermaid diagram, math and a chart, fetching each renderer only on first use', async () => {
  const markdown = [
    'Loss $L = \\sum_i (y_i - \\hat y_i)^2$ fell.',
    '$$',
    '\\frac{a}{b}',
    '$$',
    '```mermaid',
    'graph LR',
    '  A[hub] --> B[page]',
    '```',
    '```chart bar',
    'step,reward',
    '1,0.5',
    '2,0.9',
    '```',
  ].join('\n');
  const { stdout, stderr, requests } = await runPage(`
window.addEventListener('test-ready', () => {
  const enc = new TextEncoder();
  const before = performance.getEntriesByType('resource').map((entry) => new URL(entry.name).pathname);
  replyFromHub('unknown', 'display', [], 1, { display: { markdown: ${JSON.stringify(markdown)} } });
  const painted = (canvas) => { const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data; let count = 0; for (let index = 3; index < pixels.length; index += 4) if (pixels[index]) count++; return count; };
  setTimeout(() => {
    const item = document.querySelector('.display-item');
    const canvas = item.querySelector('.chart canvas');
    document.body.dataset.renderTest = JSON.stringify({
      before: before.filter((path) => path.startsWith('/lib/')),
      inline: item.querySelector('p .math .katex') !== null,
      block: item.querySelector('.math-block .katex-display') !== null,
      diagram: item.querySelectorAll('.mermaid svg g').length > 0 && item.querySelector('.mermaid code') === null,
      chart: Boolean(canvas) && canvas.width > 0 && painted(canvas) > 100,
      stylesheet: [...document.styleSheets].some((sheet) => /katex-[A-Z0-9]{8}\\.css$/.test(sheet.href ?? '')),
    });
  }, 6000);
});
`, { budget: 9000 });
  const encoded = /data-render-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const { before, ...rendered } = JSON.parse(encoded ?? 'null') ?? {};
  assert.deepEqual(rendered, { inline: true, block: true, diagram: true, chart: true, stylesheet: true }, stderr + stdout.slice(0, 2000));
  assert.ok(await libBytes(before) < 8192, before.join(' '));
  const lazy = requests.filter((path) => /^\/lib\/.+-[A-Z0-9]{8}\.(js|css)$/.test(path));
  assert.ok(lazy.some((path) => path.includes('mermaid')) && lazy.some((path) => path.includes('katex')) && lazy.some((path) => path.includes('auto-')), lazy.join(' '));
});

test('unknown inherited fence names remain code and Mermaid image sources do not load', async () => {
  const markdown = '```constructor\nplain text\n```\n```mermaid\nflowchart LR\nA@{ img: "https://example.invalid/tracker.png" }\n```';
  const { stdout, stderr, requests } = await runPage(`
window.mermaidImageRequests = [];
const NativeImage = window.Image;
window.Image = class extends NativeImage { set src(value) { window.mermaidImageRequests.push(value); super.src = value; } get src() { return super.src; } };
window.addEventListener('test-ready', () => {
  const enc = new TextEncoder();
  replyFromHub('unknown', 'display', [], 1, { display: { markdown: ${JSON.stringify(markdown)} } });
  setTimeout(() => { document.body.dataset.renderTest = JSON.stringify({ code: document.querySelector('.display-item pre code')?.textContent, diagram: document.querySelector('.display-item .mermaid')?.textContent, images: window.mermaidImageRequests }); }, 1000);
});`);
  const encoded = /data-render-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { code: 'plain text', diagram: 'flowchart LR\nA@{ img: "https://example.invalid/tracker.png" }', images: [] }, stderr);
  assert.equal(requests.some((path) => path.includes('tracker')), false);
});

test('a display without diagrams, math or charts fetches no renderer, and prose keeps its dollars and links its bare URLs', async () => {
  const { stdout, stderr, requests } = await runPage(`
window.addEventListener('test-ready', () => {
  const enc = new TextEncoder();
  replyFromHub('unknown', 'display', [], 1, { display: { markdown: 'Spent $9 on the pizza and $3 more, see https://example.test/receipt?id=1). Ended.' } });
  setTimeout(() => {
    const item = document.querySelector('.display-item');
    document.body.dataset.linkTest = JSON.stringify({ text: item.textContent, links: [...item.querySelectorAll('a')].map((a) => [a.href, a.rel, a.target]), math: item.querySelectorAll('.math').length });
  }, 500);
});
`);
  const encoded = /data-link-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { text: 'Spent $9 on the pizza and $3 more, see https://example.test/receipt?id=1). Ended.', links: [['https://example.test/receipt?id=1', 'noopener noreferrer', '']], math: 0 }, stderr);
  assert.ok(await libBytes(requests) < 8192, requests.join(' '));
});

for (const standalone of [false, true]) test(`panel links preserve the page only in standalone mode: ${standalone}`, async () => {
  const { stdout, stderr, requests } = await runPage(`
const originalMatchMedia = window.matchMedia.bind(window);
window.matchMedia = (query) => query === '(display-mode: standalone)' ? { matches: ${standalone} } : originalMatchMedia(query);
window.addEventListener('test-ready', () => {
  const url = location.origin + '/panel-link-destination';
  replyFromHub('unknown', 'display', [], 1, { display: { markdown: '[label](' + url + '#markdown) ' + url + '#bare', link: url + '#payload' } });
  setTimeout(async () => {
    const channel = testChannel;
    const links = [...document.querySelectorAll('.display-item a')];
    const attributes = links.map(link => [link.textContent === 'label' ? 'label' : new URL(link.href).hash, new URL(link.href).hash, link.getAttribute('target'), link.rel]);
    let opened = 0;
    for (const link of links) {
      if (${standalone} && link.target !== '_blank') continue;
      if (!${standalone}) link.href = '#' + new URL(link.href).hash.slice(1);
      else opened++;
      link.click();
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    await fetch('/panel-links-loaded?count=' + opened);
    document.body.dataset.linkTest = JSON.stringify({ attributes, hash: location.hash, active: document.querySelector('#puppet').getAttribute('aria-pressed'), sameSession: channel === testChannel && channel.readyState === 'open' });
  }, 100);
});`);
  const encoded = /data-link-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), {
    attributes: [['label', '#markdown'], ['#bare', '#bare'], ['#payload', '#payload']].map(([label, hash]) => [label, hash, standalone ? '_blank' : null, 'noopener noreferrer']),
    hash: standalone ? '' : '#payload', active: 'true', sameSession: true,
  }, stderr);
  assert.equal(requests.filter(path => path === '/panel-link-destination').length, standalone ? 3 : 0, 'each standalone link loads outside the original page');
});

test('adopted renderer output loses scripts, handlers, remote references and unsafe links in both SVG and HTML', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const { adopt } = await import('/lib/render.js');
  const svg = document.createElement('div');
  adopt(svg, '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" onload="alert(1)"><script>alert(2)<\\/script><style>.a{fill:red}</style><a href="javascript:alert(3)"><text onclick="x()">t</text></a><a href="https://ok.test/"><text>k</text></a><use xlink:href="https://evil.test/x.svg#y"/><use href="#local"/><image href="https://evil.test/a.png"/><foreignObject><div onmouseover="y()">f</div></foreignObject></svg>', 'image/svg+xml');
  const html = document.createElement('div');
  adopt(html, '<span class="katex"><img src="x" onerror="alert(4)"><a href="https://ok.test/" target="_blank">k</a><a href="data:text/html,x">d</a><iframe srcdoc="x"></iframe></span>', 'text/html');
  let unparseable = false;
  try { adopt(document.createElement('div'), '<svg', 'image/svg+xml'); } catch { unparseable = true; }
  document.body.dataset.adoptTest = JSON.stringify({ svg: svg.innerHTML, html: html.innerHTML, unparseable });
});
`);
  const encoded = /data-adopt-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"').replaceAll('&lt;', '<').replaceAll('&gt;', '>').replaceAll('&amp;', '&');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result, stderr);
  assert.equal(result.unparseable, true);
  for (const forbidden of ['script', 'onload', 'onclick', 'onmouseover', 'onerror', 'javascript:', 'evil.test', '<image', '<iframe', 'srcdoc', 'data:']) assert.equal(result.svg.includes(forbidden) || result.html.includes(forbidden), false, forbidden + ': ' + result.svg + result.html);
  for (const kept of ['<style>.a{fill:red}</style>', 'href="https://ok.test/" rel="noopener noreferrer"', 'href="#local"', '<foreignObject><div>f</div></foreignObject>']) assert.ok(result.svg.includes(kept), kept + ': ' + result.svg);
  assert.ok(result.html.includes('href="https://ok.test/" rel="noopener noreferrer"') && result.html.includes('<img>'), result.html);
});

for (const close of ["testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.closed' }) }))", "testChannel.dispatchEvent(new Event('close'))"]) test('provider close offers a fresh session: ' + close, async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  ${close};
  await new Promise(resolve => setTimeout(resolve, 30));
  const offered = document.querySelector('#status').textContent;
  document.querySelector('#puppet').click();
  await new Promise(resolve => setTimeout(resolve, 50));
  document.body.dataset.closeTest = JSON.stringify({ offered, active: document.querySelector('#puppet').getAttribute('aria-pressed') });
});
`);
  const encoded = /data-close-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { offered: 'Session ended — tap to continue', active: 'true' }, stderr);
});

test('animation deltas and input utterances reach session telemetry', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  testPuppet.onAnimation({ clips: [{ name: 'sit', weight: 0.5 }], hip_height: 1.2 });
  for (const event of [{ type: 'input_audio_buffer.speech_started' }, { type: 'session.input_transcript.delta', delta: 'Please stand.' }]) emitLive(event);
  globalThis.onTelemetry = () => {
    const events = telemetryBatches.flat().filter((event) => ['animation', 'input_utterance', 'input_speech_started'].includes(event.name));
    if (events.length >= 3) document.body.dataset.poseTelemetry = JSON.stringify(events);
  };
  onTelemetry();
});
`);
  const encoded = /data-pose-telemetry="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const events = JSON.parse(encoded ?? 'null');
  assert.ok(events, stderr);
  assert.equal(events.length, 3);
  assert.deepEqual(JSON.parse(events.find((event) => event.name === 'animation').detail), { clips: [{ name: 'sit', weight: 0.5 }], hip_height: 1.2 });
  assert.equal(events.find((event) => event.name === 'input_utterance').detail, 'Please stand.');
  assert.ok(events.every((event) => event.session_id === events[0].session_id && event.at > 0));
});

test('display pipe tables preserve rows, inline links and pipes inside code', async () => {
  const markdown = '| Name | Value |\n| :--- | ---: |\n| First | one |\n| Second | two |\nAfter\n\n| Link | Code | Empty |\n| --- | :---: | --- |\n| [docs](https://example.test/docs) | `left|right` | |\n\n| ordinary | prose |\n| not a separator | text |';
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const enc = new TextEncoder();
  replyFromHub('unknown', 'display', [], 1, { display: { markdown: ${JSON.stringify(markdown)} } });
  setTimeout(() => {
    const item = document.querySelector('.display-item');
    document.body.dataset.tableTest = JSON.stringify({
      tables: [...item.querySelectorAll('table')].map((table) => ({
        head: [...table.querySelectorAll('thead tr')].map((row) => [...row.querySelectorAll('th')].map((cell) => cell.textContent)),
        body: [...table.querySelectorAll('tbody tr')].map((row) => [...row.querySelectorAll('td')].map((cell) => cell.textContent)),
      })),
      link: [...item.querySelectorAll('td a')].map((link) => [link.textContent, link.href, link.rel, link.target]),
      code: [...item.querySelectorAll('td code')].map((code) => code.textContent),
      paragraphs: [...item.querySelectorAll('p')].map((paragraph) => paragraph.textContent),
    });
  }, 500);
});`);
  const encoded = /data-table-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), {
    tables: [
      { head: [['Name', 'Value']], body: [['First', 'one'], ['Second', 'two']] },
      { head: [['Link', 'Code', 'Empty']], body: [['docs', 'left|right', '']] },
    ],
    link: [['docs', 'https://example.test/docs', 'noopener noreferrer', '']],
    code: ['left|right'],
    paragraphs: ['After', '| ordinary | prose |', '| not a separator | text |'],
  }, stderr);
});

const untilAsleep = `
const until = async (check) => { while (!check()) await new Promise((resolve) => setTimeout(resolve, 10)); };
const signOff = (delta = ${JSON.stringify(SIGN_OFF)}) => emitLive({ type: 'session.output_transcript.delta', delta });
const count = (verb) => sentVerbs.filter((item) => item === verb).length;
const sessionEvents = (name) => telemetryBatches.flat().filter((event) => event.kind === 'session' && event.name === name);
const sleepNow = async () => {
  document.querySelector('#puppet').click();
  await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false');
  testPuppet.calls.length = 0;
  sentLiveEvents.length = 0;
};
`;

async function runWakePage(script, { setup = '', ...options } = {}) {
  const { stdout, stderr } = await runPage(`${setup}${untilAsleep}
window.addEventListener('test-ready', async () => {
  try { document.body.dataset.wakeTest = JSON.stringify(await (async () => { ${script} })()); }
  catch (error) { document.body.dataset.wakeTest = JSON.stringify({ error: String(error?.stack ?? error) }); }
});
`, options);
  const result = JSON.parse(/data-wake-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"') ?? 'null');
  assert.ok(result && !result.error, `${result?.error}\n${stderr}`);
  return result;
}

test('while asleep, anything short of a wake opens no session, asks the hub nothing and speaks nothing', async () => {
  const result = await runWakePage(`
    await sleepNow();
    const offers = count('offer');
    for (const miss of [0.2, 0.4, 0.6]) testSpotter.heard({ miss });
    testSpotter.heard({ ready: true });
    await new Promise((resolve) => setTimeout(resolve, 300));
    return { offers: count('offer') - offers, delegates: count('delegate'), pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), wakes: sessionEvents('wake').length, spoken: testPuppet.calls.filter(([name]) => name === 'speak').length, live: sentLiveEvents.length };
  `);
  assert.deepEqual(result, { offers: 0, delegates: 0, pressed: 'false', wakes: 0, spoken: 0, live: 0 });
});

test('the wake phrase carries earlier turns and a completed-sleep marker into one new session', async () => {
  const result = await runWakePage(`
    emitLive({ type: 'session.output_transcript.delta', delta: 'The beacon is green.' });
    hear('Goodnight, Corvus.');
    signOff();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false');
    testPuppet.calls.length = 0;
    sentLiveEvents.length = 0;
    const offers = count('offer');
    testSpotter.heard({ wake: 0.93 });
    testSpotter.heard({ wake: 0.95 });
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    testSpotter.heard({ wake: 0.97 });
    await new Promise((resolve) => setTimeout(resolve, 300));
    return {
      offers: count('offer') - offers,
      offer: Object.keys(lastOffer),
      context: lastOffer.context,
      marker: lastOffer.wake,
      wakes: sessionEvents('wake').map((event) => event.detail),
      puppet: testPuppet.calls.filter(([name]) => name === 'asleep' || name === 'pose'),
      live: sentLiveEvents.map(({ type, delegation_id, content }) => ({ type, delegation_id, content })),
    };
  `);
  assert.equal(result.offers, 1);
  assert.deepEqual(result.offer, ['id', 'sdp', 'context', 'wake']);
  assert.equal(result.context.length, 1);
  assert.equal(result.context[0].speaker, 'user');
  assert.match(result.context[0].text, /^Context: Archived transcripts of completed conversations, oldest first/);
  const [conversation, ...others] = JSON.parse(result.context[0].text.split('\n').slice(1).join('\n'));
  assert.deepEqual(others, []);
  assert.deepEqual(conversation.turns, [{ speaker: 'live', text: 'The beacon is green.' }, { speaker: 'user', text: 'Goodnight, Corvus.' }, { speaker: 'live', text: SIGN_OFF }]);
  assert.match(conversation.went_to_sleep, /^about \d+ seconds? ago$/);
  assert.match(result.marker, /^The previous conversation ended and you went to sleep about \d+ seconds? ago\. You have just been woken for a new conversation\. Any earlier goodbye or request to sleep was already completed\./);
  assert.deepEqual(result.wakes, ['0.930']);
  assert.deepEqual(result.puppet, [['asleep', false], ['pose', 'stand'], ['pose', 'listen']]);
  assert.deepEqual(result.live, [
    { type: 'session.instructions.append', delegation_id: null, content: `Your name is ${NAME}. Earlier turns are memory of completed conversations, not results for this conversation. If the user asks the hub or requests a fresh or current check, always delegate again, even if an earlier turn seems to answer it. Never speak an earlier hub answer as a new result; wait for the new application reply. If a hub reply asks the user a question, delegate the user's answer; if the user changes the subject instead, handle that turn as a new request. When the user asks you to sleep or signals that the conversation is over, for example with a goodbye, "that'll be all", or a hint that it is bedtime, end your reply with "${SIGN_OFF}" and do not delegate, even while a hub request is pending. Never say "${SIGN_OFF}" at any other time; it ends the conversation.` },
    { type: 'session.commentary.append', delegation_id: null, content: `Context: ${NAME} was just woken.` },
  ]);
});

for (const [label, endScript] of [['the sign-off', 'signOff()'], ['a tap', "document.querySelector('#puppet').click()"]]) test(`a tap as soon as ${label} seats the puppet opens a fresh session that hears the microphone`, async () => {
  const result = await runWakePage(`
    const ended = testChannel;
    testPuppet.calls.length = 0;
    ${endScript};
    await until(() => testPuppet.calls.some(([name, value]) => name === 'asleep' && value));
    const offers = count('offer');
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    hear('Can you hear me?');
    await until(() => sessionEvents('input_utterance').some((event) => event.session_id === lastOffer.id));
    return { offers: count('offer') - offers, fresh: testChannel !== ended, microphone: sentTracks.at(-1).readyState, heard: sessionEvents('input_utterance').filter((event) => event.session_id === lastOffer.id).map((event) => event.detail) };
  `);
  assert.deepEqual(result, { offers: 1, fresh: true, microphone: 'live', heard: ['Can you hear me?'] });
});

test('a misheard wake phrase is a logged miss that leaves it asleep', async () => {
  const result = await runWakePage(`
    await sleepNow();
    const offers = count('offer');
    testSpotter.heard({ miss: 0.61 });
    await until(() => sessionEvents('wake-miss').length);
    return { offers: count('offer') - offers, pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), misses: sessionEvents('wake-miss').map((event) => event.detail) };
  `);
  assert.deepEqual(result, { offers: 0, pressed: 'false', misses: ['0.610'] });
});

const demoModel = JSON.parse(await readFile(new URL('../scripts/wake-demo.json', import.meta.url), 'utf8'));
test('the spotter listens with the model the backend serves', async () => {
  const result = await runWakePage(`
    await until(() => globalThis.testSpotter);
    return { threshold: testSpotter.model.threshold, errors: telemetryBatches.flat().filter((event) => event.kind === 'error').map((event) => event.message) };
  `, { setup: `globalThis.backendWakeModel = ${JSON.stringify({ ...demoModel, threshold: 0.9 })};` });
  assert.deepEqual(result, { threshold: 0.9, errors: [] });
});

for (const [name, served, error] of [
  ['without a backend model', null, 'the backend has no wake model'],
  ['with a backend model for another phrase', { ...demoModel, phrase: 'another phrase' }, 'the backend wake model listens for another phrase: another phrase'],
  ['with a backend model missing its weights', { phrase: WAKE_PHRASE, threshold: 0.9 }, 'invalid wake model'],
  ['when the backend cannot read its model', 'unreadable', 'wake model unreadable'],
]) test(`${name} the spotter stays off, says so without retrying, and a tap still opens a session`, async () => {
  const result = await runWakePage(`
    const errors = () => telemetryBatches.flat().filter((event) => event.kind === 'error').map((event) => event.message);
    await sleepNow();
    await until(() => errors().length);
    const requests = count('wake-model');
    await new Promise((resolve) => setTimeout(resolve, 1000));
    const retried = count('wake-model') - requests;
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    return { spotter: Boolean(globalThis.testSpotter), errors: [...new Set(errors())], retried };
  `, { setup: `globalThis.backendWakeModel = ${JSON.stringify(served)};` });
  assert.deepEqual(result, { spotter: false, errors: [error], retried: 0 });
});

test('a wake model that arrives after a long wait still starts the spotter', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await new Promise((resolve) => setTimeout(resolve, 60000));
    deliverRelay(new TextEncoder().encode('wake-model\\n' + ${JSON.stringify(JSON.stringify(testWakeModel))}));
    await until(() => globalThis.testSpotter);
    return { errors: telemetryBatches.flat().filter((event) => event.kind === 'error').map((event) => event.message) };
  `, { setup: "globalThis.backendWakeModel = 'unanswered';", budget: 70000 });
  assert.deepEqual(result, { errors: [] });
});

test('a relay lost before the wake model arrives leaves the spotter off and says why', async () => {
  const result = await runWakePage(`
    await sleepNow();
    loseRelay(new Error('relay closed'));
    await until(() => telemetryBatches.flat().some((event) => event.kind === 'error'));
    return { spotter: Boolean(globalThis.testSpotter), errors: telemetryBatches.flat().filter((event) => event.kind === 'error').map((event) => event.message) };
  `, { setup: "globalThis.backendWakeModel = 'unanswered';" });
  assert.deepEqual(result, { spotter: false, errors: ['relay closed'] });
});

test('a reconnect that fails asks for no wake model on a connection nobody reads', async () => {
  const result = await runWakePage(`
    await sleepNow();
    loseRelay(new Error('relay closed'));
    await until(() => telemetryBatches.flat().some((event) => event.kind === 'error'));
    const requests = count('wake-model');
    document.querySelector('#puppet').click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    deliverRelay(new TextEncoder().encode(JSON.stringify({ ok: false })));
    await until(() => document.querySelector('#status').textContent.includes('token rejected'));
    return { requests: count('wake-model') - requests };
  `, { setup: "globalThis.backendWakeModel = 'unanswered';" });
  assert.deepEqual(result, { requests: 0 });
});

test('a reconnect cancelled while it dials still listens for the wake phrase once it connects', async () => {
  const result = await runWakePage(`
    await sleepNow();
    loseRelay(new Error('relay closed'));
    await until(() => telemetryBatches.flat().some((event) => event.kind === 'error'));
    globalThis.backendWakeModel = undefined;
    const requests = count('wake-model');
    document.querySelector('#puppet').click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    document.querySelector('#puppet').click();
    deliverRelay(new TextEncoder().encode(JSON.stringify({ ok: true })));
    await until(() => globalThis.testSpotter);
    return { requests: count('wake-model') - requests };
  `, { setup: "globalThis.backendWakeModel = 'unanswered';" });
  assert.deepEqual(result, { requests: 1 });
});

test('the model speaking the sign-off ends the session once speech goes quiet and the puppet falls asleep listening again', async () => {
  const result = await runWakePage(`
    const { LivePlayback } = await import('/live-playback.js');
    let quiet;
    LivePlayback.prototype.quiet = () => new Promise((resolve) => { quiet = resolve; });
    hear('That will be all.');
    signOff('Goodnight. ' + ${JSON.stringify(SIGN_OFF.slice(0, 8).toUpperCase())});
    signOff(${JSON.stringify(SIGN_OFF.slice(8).replace(/\.$/, '!'))});
    await new Promise((resolve) => setTimeout(resolve, 200));
    const speaking = document.querySelector('#puppet').getAttribute('aria-pressed');
    quiet?.();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false' && sessionEvents('close').length);
    return {
      speaking,
      sleeps: sessionEvents('sleep').map((event) => event.detail),
      delegates: count('delegate'),
      live: sentLiveEvents.map((event) => event.event_id),
      puppet: testPuppet.calls.filter(([name]) => name === 'asleep').at(-1),
      microphone: { enabled: testMicrophoneTrack.enabled, button: document.querySelector('#mic-mute').disabled },
      rewoken: await (async () => { testSpotter.heard({ wake: 0.9 }); await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true'); return true; })(),
    };
  `);
  assert.deepEqual(result, { speaking: 'true', sleeps: ['sign-off'], delegates: 0, live: ['identity', 'wake'], puppet: ['asleep', true], microphone: { enabled: true, button: false }, rewoken: true });
});

test('a goodbye from the user or a partial sign-off keeps the session open', async () => {
  const result = await runWakePage(`
    hear('Goodnight, Corvus. Go to sleep.');
    signOff(${JSON.stringify(SIGN_OFF.split(' ').slice(0, -1).join(' ') + '.')});
    hear(' ${SIGN_OFF}');
    await new Promise((resolve) => setTimeout(resolve, 300));
    return { pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), sleeps: sessionEvents('sleep').length };
  `);
  assert.deepEqual(result, { pressed: 'true', sleeps: 0 });
});

test('a sign-off with a delegation outstanding ends the session and its late hub reply is never spoken', async () => {
  const result = await runWakePage(`
    const { LivePlayback } = await import('/live-playback.js');
    const quiets = [];
    LivePlayback.prototype.quiet = () => new Promise((resolve) => quiets.push(resolve));
    hear('Check the test beacon.');
    delegateTurn('pending');
    await until(() => count('delegate'));
    hear('Corvus, go to sleep.');
    signOff();
    await until(() => quiets.length === 1);
    hear(' Goodnight.');
    await new Promise((resolve) => setTimeout(resolve, 3100));
    replyFromHub('pending', 'late', ['The test beacon is amber.']);
    await until(() => globalThis.hubAcks?.length);
    quiets[0]();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false' && sessionEvents('close').length);
    return { delegates: count('delegate'), sleeps: sessionEvents('sleep').map((event) => event.detail), appended: sentLiveEvents.filter((event) => event.event_id?.includes('late')).length, acks: hubAcks };
  `, { budget: 6000 });
  assert.deepEqual(result, { delegates: 1, sleeps: ['sign-off'], appended: 0, acks: ['late'] });
});

test('a sign-off split by user speech still ends the session, and more speech after it sleeps only once', async () => {
  const result = await runWakePage(`
    signOff(${JSON.stringify(SIGN_OFF.slice(0, 8))});
    hear('Mm.');
    signOff(${JSON.stringify(SIGN_OFF.slice(8))});
    signOff(' Sleep well.');
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false' && sessionEvents('close').length);
    return { sleeps: sessionEvents('sleep').map((event) => event.detail) };
  `);
  assert.deepEqual(result, { sleeps: ['sign-off'] });
});

test('a session woken after a sign-off stays awake through its greeting', async () => {
  const result = await runWakePage(`
    signOff();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false' && sessionEvents('close').length);
    testSpotter.heard({ wake: 0.95 });
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    emitLive({ type: 'session.output_transcript.delta', delta: 'Hi, I am listening.' });
    await new Promise((resolve) => setTimeout(resolve, 300));
    return { pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), sleeps: sessionEvents('sleep').map((event) => event.detail) };
  `);
  assert.deepEqual(result, { pressed: 'true', sleeps: ['sign-off'] });
});

test('the screen stays on only while awake, and a lock the browser drops on hide returns with the page', async () => {
  const result = await runWakePage(`
    await until(() => held() === 1 && !document.querySelector('#puppet-choice').disabled);
    const awake = { held: held(), requests: screenLocks.length };
    showPage(false);
    await until(() => held() === 0);
    const hidden = { held: held(), requests: screenLocks.length };
    showPage(true);
    await until(() => held() === 1);
    const returned = { held: held(), requests: screenLocks.length };
    await sleepNow();
    await until(() => held() === 0);
    showPage(false);
    showPage(true);
    await new Promise((resolve) => setTimeout(resolve, 100));
    const asleep = { held: held(), requests: screenLocks.length };
    globalThis.holdSessionStart = true;
    document.querySelector('#puppet').click();
    await until(() => held() === 1 && globalThis.heldSessionStart);
    const starting = { held: held(), requests: screenLocks.length, pressed: document.querySelector('#puppet').getAttribute('aria-pressed') };
    globalThis.holdSessionStart = false;
    heldSessionStart();
    await sleepNow();
    holdScreenLock = true;
    document.querySelector('#puppet').click();
    await until(() => grantScreenLock);
    await sleepNow();
    grantScreenLock();
    await new Promise((resolve) => setTimeout(resolve, 100));
    return { awake, hidden, returned, asleep, starting, late: { held: held(), requests: screenLocks.length }, types: [...new Set(screenLocks.map((lock) => lock.type))] };
  `, { setup: `
    const screenLocks = [];
    let pageHidden = false;
    Object.defineProperty(document, 'hidden', { get: () => pageHidden });
    Object.defineProperty(document, 'visibilityState', { get: () => pageHidden ? 'hidden' : 'visible' });
    let holdScreenLock = false, grantScreenLock;
    Object.defineProperty(navigator, 'wakeLock', { value: { request: async (type) => {
      if (pageHidden) throw new DOMException('The requesting page is not visible', 'NotAllowedError');
      const lock = Object.assign(new EventTarget(), { type, released: false, async release() {
        if (lock.released) return;
        lock.released = true;
        lock.dispatchEvent(new Event('release'));
      } });
      screenLocks.push(lock);
      if (holdScreenLock) await new Promise((resolve) => { grantScreenLock = resolve; });
      return lock;
    } } });
    const held = () => screenLocks.filter((lock) => !lock.released).length;
    const showPage = (visible) => {
      pageHidden = !visible;
      if (!visible) for (const lock of screenLocks) lock.release();
      document.dispatchEvent(new Event('visibilitychange'));
    };
  ` });
  assert.deepEqual(result, {
    awake: { held: 1, requests: 1 },
    hidden: { held: 0, requests: 1 },
    returned: { held: 1, requests: 2 },
    asleep: { held: 0, requests: 2 },
    starting: { held: 1, requests: 3, pressed: 'false' },
    late: { held: 0, requests: 4 },
    types: ['screen'],
  });
});

test('inactivity sleeps after the generous window even while a hub request is pending', async () => {
  const result = await runWakePage(`
    hear('Take your time.');
    delegateTurn('slow');
    await new Promise((resolve) => setTimeout(resolve, ${INACTIVITY_MS - 60000}));
    const before = document.querySelector('#puppet').getAttribute('aria-pressed');
    await new Promise((resolve) => setTimeout(resolve, 65000));
    return { before, after: document.querySelector('#puppet').getAttribute('aria-pressed'), sleeps: sessionEvents('sleep').map((event) => event.detail) };
  `, { budget: INACTIVITY_MS + 30000 });
  assert.deepEqual(result, { before: 'true', after: 'false', sleeps: ['inactivity'] });
});

test('muting the microphone persists while asleep and into the next session', async () => {
  const result = await runWakePage(`
    const state = () => ({ enabled: testMicrophoneTrack.enabled, pressed: document.querySelector('#mic-mute').getAttribute('aria-pressed'), disabled: document.querySelector('#mic-mute').disabled, live: document.querySelector('#puppet').getAttribute('aria-pressed') });
    document.querySelector('#mic-mute').click();
    await sleepNow();
    const asleep = state();
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    const awake = state();
    document.querySelector('#mic-mute').click();
    return { asleep, awake, unmuted: state() };
  `);
  assert.deepEqual(result, {
    asleep: { enabled: false, pressed: 'true', disabled: false, live: 'false' },
    awake: { enabled: false, pressed: 'true', disabled: false, live: 'true' },
    unmuted: { enabled: true, pressed: 'false', disabled: false, live: 'true' },
  });
});

test('a muted microphone runs no wake spotter, not even after a session ends, and unmuting starts one that hears the phrase', async () => {
  const result = await runWakePage(`
    await sleepNow();
    const listening = testSpotter;
    const before = listening.closed;
    document.querySelector('#mic-mute').click();
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    await sleepNow();
    await new Promise((resolve) => setTimeout(resolve, 300));
    const muted = { closed: listening.closed, replaced: testSpotter !== listening };
    document.querySelector('#mic-mute').click();
    await until(() => !testSpotter.closed);
    testSpotter.heard({ wake: 0.9 });
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true' && sessionEvents('wake').length);
    return { before, muted, wakes: sessionEvents('wake').map((event) => event.detail) };
  `);
  assert.deepEqual(result, { before: 0, muted: { closed: 1, replaced: false }, wakes: ['0.900'] });
});

test('a reconnect while the microphone is muted starts no wake spotter until unmute', async () => {
  const result = await runWakePage(`
    await sleepNow();
    document.querySelector('#mic-mute').click();
    loseRelay(new Error('relay closed'));
    await until(() => document.querySelector('#status').textContent.includes('connection lost'));
    document.querySelector('#puppet').click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    document.querySelector('#puppet').click();
    deliverRelay(new TextEncoder().encode(JSON.stringify({ ok: true })));
    await new Promise((resolve) => setTimeout(resolve, 300));
    const spotter = !testSpotter.closed;
    document.querySelector('#mic-mute').click();
    await until(() => !testSpotter.closed);
    return { spotter };
  `);
  assert.deepEqual(result, { spotter: false });
});

test('the spotter stops throughout a conversation and restarts when it ends', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => !testSpotter.closed);
    const asleep = testSpotter;
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    await new Promise((resolve) => setTimeout(resolve, 300));
    const mid = { closed: asleep.closed, replaced: testSpotter !== asleep, running: !testSpotter.closed };
    document.querySelector('#mic-mute').click();
    document.querySelector('#mic-mute').click();
    await new Promise((resolve) => setTimeout(resolve, 300));
    const unmuted = { closed: asleep.closed, replaced: testSpotter !== asleep, running: !testSpotter.closed };
    await sleepNow();
    await until(() => !testSpotter.closed);
    return { mid, unmuted, restarted: testSpotter !== asleep };
  `);
  assert.deepEqual(result, { mid: { closed: 1, replaced: false, running: false }, unmuted: { closed: 1, replaced: false, running: false }, restarted: true });
});

for (const [phase, enter, status] of [
  ['while asleep', '', ''],
  ['while a session starts', "globalThis.holdSessionStart = true; document.querySelector('#puppet').click(); await until(() => globalThis.heldSessionStart);", 'microphone lost'],
  ['during a session', "document.querySelector('#puppet').click(); await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');", 'microphone lost'],
]) test(`a microphone that ends ${phase} is reopened, and the next session sends and records the live one`, async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => globalThis.testSpotter && !testSpotter.closed);
    const spotter = testSpotter;
    const opens = microphoneOpens;
    ${enter}
    endMicrophone();
    await until(() => sessionEvents('microphone-ended').length && testSpotter !== spotter && !testSpotter.closed);
    const lost = { status: document.querySelector('#status').textContent, pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), events: sessionEvents('microphone-ended').length, opens: microphoneOpens - opens, closed: spotter.closed, replaced: testSpotter !== spotter, running: !testSpotter.closed, hears: testSpotter.stream.getAudioTracks()[0].readyState };
    globalThis.holdSessionStart = false;
    const tracks = globalThis.sentTracks?.length ?? 0;
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true' && sentTracks?.length > tracks);
    return { lost, sends: sentTracks.at(-1).readyState, errors: fleetLines };
  `);
  assert.deepEqual(result, { lost: { status, pressed: 'false', events: 1, opens: 1, closed: 1, replaced: true, running: true, hears: 'live' }, sends: 'live', errors: [] });
});

test('a microphone that cannot reopen after it ends comes back on the next device change', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => globalThis.testSpotter && !testSpotter.closed);
    const opens = microphoneOpens;
    globalThis.microphoneMissing = true;
    endMicrophone();
    await new Promise((resolve) => setTimeout(resolve, 300));
    const missing = { opens: microphoneOpens - opens, running: !testSpotter.closed };
    globalThis.microphoneMissing = false;
    navigator.mediaDevices.dispatchEvent(new Event('devicechange'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    return { missing, returned: { opens: microphoneOpens - opens, running: !testSpotter.closed, hears: testSpotter.stream.getAudioTracks()[0].readyState }, errors: fleetLines };
  `);
  assert.deepEqual(result, { missing: { opens: 0, running: false }, returned: { opens: 1, running: true, hears: 'live' }, errors: ['fleet-error: voice/page — NotFoundError: Requested device not found'] });
});

test('a refused microphone shows on the status line, asleep or tapped, as a session event and no page error, and clears once allowed', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => globalThis.testSpotter && !testSpotter.closed);
    const status = () => document.querySelector('#status').textContent;
    globalThis.microphoneRefused = true;
    endMicrophone();
    await new Promise((resolve) => setTimeout(resolve, 300));
    const asleep = { status: status(), refusals: sessionEvents('microphone-refused').map((event) => event.detail), running: !testSpotter.closed };
    document.querySelector('#mic-mute').click();
    document.querySelector('#puppet').click();
    await new Promise((resolve) => setTimeout(resolve, 300));
    const tapped = { status: status(), pressed: document.querySelector('#puppet').getAttribute('aria-pressed') };
    globalThis.microphoneRefused = false;
    document.querySelector('#mic-mute').click();
    await new Promise((resolve) => setTimeout(resolve, 300));
    return { asleep, tapped, allowed: { status: status(), running: !testSpotter.closed, hears: testSpotter.stream.getAudioTracks()[0].readyState }, errors: fleetLines };
  `);
  const refused = "microphone not allowed — enable it in this site's settings";
  assert.deepEqual(result, { asleep: { status: refused, refusals: ['Permission denied'], running: false }, tapped: { status: refused, pressed: 'false' }, allowed: { status: '', running: true, hears: 'live' }, errors: [] });
});

test('with no microphone at load, mute is available at once and holds when a device arrives', async () => {
  const result = await runWakePage(`
    const button = document.querySelector('#mic-mute');
    const before = { opens: microphoneOpens, disabled: button.disabled };
    button.click();
    globalThis.microphoneMissing = false;
    navigator.mediaDevices.dispatchEvent(new Event('devicechange'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    const arrived = { opens: microphoneOpens, pressed: button.getAttribute('aria-pressed') };
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    return { before, arrived, sends: { state: sentTracks.at(-1).readyState, enabled: sentTracks.at(-1).enabled } };
  `, { setup: 'globalThis.microphoneMissing = true;' });
  assert.deepEqual(result, { before: { opens: 0, disabled: false }, arrived: { opens: 0, pressed: 'true' }, sends: { state: 'live', enabled: false } });
});

test('a mute set while no microphone is open holds, and the next session opens the microphone muted', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => globalThis.testSpotter && !testSpotter.closed);
    const opens = microphoneOpens;
    globalThis.microphoneMissing = true;
    endMicrophone();
    await new Promise((resolve) => setTimeout(resolve, 300));
    document.querySelector('#mic-mute').click();
    globalThis.microphoneMissing = false;
    navigator.mediaDevices.dispatchEvent(new Event('devicechange'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    const button = document.querySelector('#mic-mute');
    const muted = { opens: microphoneOpens - opens, running: !testSpotter.closed, pressed: button.getAttribute('aria-pressed'), disabled: button.disabled };
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    return { muted, sends: { state: sentTracks.at(-1).readyState, enabled: sentTracks.at(-1).enabled } };
  `);
  assert.deepEqual(result, { muted: { opens: 0, running: false, pressed: 'true', disabled: false }, sends: { state: 'live', enabled: false } });
});

test('a running spotter survives relay loss and the wake phrase reconnects', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => globalThis.testSpotter && !testSpotter.closed);
    const before = testSpotter;
    loseRelay(new Error('relay closed'));
    await until(() => document.querySelector('#status').textContent.includes('connection lost'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    const afterLoss = { closed: before.closed, replaced: testSpotter !== before, running: !testSpotter.closed };
    const offers = count('offer');
    before.heard({ wake: 0.95 });
    await new Promise((resolve) => setTimeout(resolve, 100));
    deliverRelay(new TextEncoder().encode(JSON.stringify({ ok: true })));
    await new Promise((resolve) => setTimeout(resolve, 2500));
    return { afterLoss, pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), offers: count('offer') - offers };
  `, { budget: 15000 });
  assert.deepEqual(result.afterLoss, { closed: 0, replaced: false, running: true });
  assert.equal(result.pressed, 'true');
  assert.equal(result.offers, 1);
});

for (const control of ['reenter', 'forget']) test(control + ' stops the spotter and disables mute before waiting for uploads', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => !testSpotter.closed);
    await new Promise((resolve) => setTimeout(resolve, 0));
    const asleep = testSpotter;
    document.querySelector('#${control}').click();
    await Promise.resolve();
    return { closed: asleep.closed, replaced: testSpotter !== asleep, mute: document.querySelector('#mic-mute').disabled };
  `);
  assert.deepEqual(result, { closed: 1, replaced: false, mute: true });
});

test('relay loss during a conversation restarts the listener and its wake reconnects', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => globalThis.testSpotter && !testSpotter.closed);
    const asleep = testSpotter;
    document.querySelector('#puppet').click();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    await new Promise((resolve) => setTimeout(resolve, 300));
    const mid = { closed: asleep.closed, replaced: testSpotter !== asleep, running: !testSpotter.closed };
    loseRelay(new Error('relay closed'));
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false');
    await new Promise((resolve) => setTimeout(resolve, 2000));
    const live = testSpotter;
    const afterLoss = { closed: live.closed, replaced: live !== asleep, running: !live.closed, status: document.querySelector('#status').textContent };
    const offers = count('offer');
    if (!live.closed) {
      live.heard({ wake: 0.95 });
      await new Promise((resolve) => setTimeout(resolve, 100));
      deliverRelay(new TextEncoder().encode(JSON.stringify({ ok: true })));
      await new Promise((resolve) => setTimeout(resolve, 2500));
    }
    return { mid, afterLoss, pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), offers: count('offer') - offers };
  `, { budget: 15000 });
  assert.deepEqual(result.mid, { closed: 1, replaced: false, running: false });
  assert.equal(result.afterLoss.running, true);
  assert.equal(result.afterLoss.replaced, true);
  assert.equal(result.pressed, 'true');
  assert.equal(result.offers, 1);
});

test('a failed wake reconnect restarts the listener for another wake', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => globalThis.testSpotter && !testSpotter.closed);
    const asleep = testSpotter;
    loseRelay(new Error('relay closed'));
    await until(() => document.querySelector('#status').textContent.includes('connection lost'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    const afterLoss = { closed: asleep.closed, replaced: testSpotter !== asleep, running: !testSpotter.closed };
    asleep.heard({ wake: 0.95 });
    await new Promise((resolve) => setTimeout(resolve, 100));
    loseRelay(new Error('relay still down'));
    await until(() => document.querySelector('#status').textContent.includes('could not start'));
    await new Promise((resolve) => setTimeout(resolve, 2000));
    const live = testSpotter;
    const afterFailedReconnect = { closed: live.closed, replaced: live !== asleep, running: !live.closed, status: document.querySelector('#status').textContent };
    const offers = count('offer');
    if (!live.closed) {
      live.heard({ wake: 0.95 });
      await new Promise((resolve) => setTimeout(resolve, 100));
      deliverRelay(new TextEncoder().encode(JSON.stringify({ ok: true })));
      await new Promise((resolve) => setTimeout(resolve, 2500));
    }
    return { afterLoss, afterFailedReconnect, pressed: document.querySelector('#puppet').getAttribute('aria-pressed'), offers: count('offer') - offers };
  `, { budget: 20000 });
  assert.deepEqual(result.afterLoss, { closed: 0, replaced: false, running: true });
  assert.equal(result.afterFailedReconnect.running, true);
  assert.equal(result.afterFailedReconnect.replaced, true);
  assert.equal(result.pressed, 'true');
  assert.equal(result.offers, 1);
});

for (const loseRefresh of [false, true]) test('a cached model remains wakeable during ' + (loseRefresh ? 'failed' : 'pending') + ' reconnect refresh', async () => {
  const result = await runWakePage(`
    await sleepNow();
    await until(() => globalThis.testSpotter && !testSpotter.closed);
    const model = testSpotter.model;
    loseRelay(new Error('relay closed'));
    await until(() => document.querySelector('#status').textContent.includes('connection lost'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    globalThis.backendWakeModel = 'unanswered';
    testSpotter.heard({ wake: 0.95 });
    await new Promise((resolve) => setTimeout(resolve, 100));
    const requests = count('wake-model');
    deliverRelay(new TextEncoder().encode(JSON.stringify({ ok: true })));
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    await new Promise((resolve) => setTimeout(resolve, 300));
    if (${loseRefresh}) loseRelay(new Error('refresh lost'));
    else await sleepNow();
    await until(() => !testSpotter.closed);
    const cached = testSpotter.model === model;
    const refreshed = count('wake-model') - requests;
    if (!${loseRefresh}) {
      deliverRelay(new TextEncoder().encode('wake-model\\n' + JSON.stringify({ ...model, threshold: 0.9 })));
      await new Promise((resolve) => setTimeout(resolve, 100));
      document.querySelector('#mic-mute').click();
      document.querySelector('#mic-mute').click();
      await until(() => !testSpotter.closed && testSpotter.model.threshold === 0.9);
    }
    return { cached, refreshed, running: !testSpotter.closed, threshold: testSpotter.model.threshold };
  `, { budget: 15000 });
  assert.deepEqual(result, { cached: true, refreshed: 1, running: true, threshold: loseRefresh ? 0.5 : 0.9 });
});

test('provider configuration reaches private telemetry with only allowlisted fields', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  for (const type of ['session.created', 'session.updated']) emitLive({ type, session: { model: 'live-model', instructions: 'private instructions', delegation: { type: 'client' } } });
  emitLive({ type: 'session.delegation.created', delegation: { id: 'd', type: 'delegation', target: 'client', instructions: 'private state' } });
  setTimeout(() => { document.body.dataset.configTelemetry = JSON.stringify(telemetryBatches.flat().filter((event) => event.name?.startsWith('live-config-'))); }, 600);
});
`);
  const encoded = /data-config-telemetry="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const events = JSON.parse(encoded ?? 'null');
  assert.ok(events, stderr);
  assert.ok(events.every((event) => event.session_id && /^[a-zA-Z0-9_-]+$/.test(event.name)));
  const records = Object.fromEntries(events.map((event) => [event.name, JSON.parse(event.detail)]));
  assert.deepEqual(records['live-config-sdp-answer'], {});
  assert.deepEqual(records['live-config-session-started'], { session: { model: 'gpt-live-1', delegation: { type: 'client' } } });
  for (const type of ['created', 'updated']) assert.deepEqual(records['live-config-session-' + type], { session: { model: 'live-model', delegation: { type: 'client' } } });
  assert.deepEqual(records['live-config-session-delegation-created'], { delegation: { type: 'delegation', target: 'client' } });
  assert.doesNotMatch(JSON.stringify(events), /private instructions|private state/);
});

test('a woken session receives only the fresh hub reply', async () => {
  const result = await runWakePage(`
    emitLive({ type: 'session.output_transcript.delta', delta: 'The test beacon is amber.' });
    signOff();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false');
    testSpotter.heard({ wake: 0.95 });
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    hear('Check the test beacon.');
    delegateTurn('fresh');
    await until(() => count('delegate') > 0);
    replyFromHub('fresh', 'fresh', ['The test beacon is violet.']);
    await until(() => sentLiveEvents.some((event) => event.event_id === 'hub_fresh'));
    return { sent: delegateFrames.at(-1).text, context: lastOffer.context, appended: sentLiveEvents.filter((event) => event.type.endsWith('.append') && !['identity', 'wake'].includes(event.event_id)).map((event) => event.event_id), reply: sentLiveEvents.find((event) => event.event_id === 'hub_fresh') };
  `);
  assert.equal(result.sent, 'Check the test beacon.');
  assert.ok(result.context.some((turn) => turn.text.includes('amber')));
  assert.deepEqual(result.appended, ['hub_fresh']);
  assert.equal(result.reply.type, 'session.commentary.append');
  assert.equal(result.reply.delegation_id, 'fresh');
  assert.equal(result.reply.content, 'The test beacon is violet.');
  assert.doesNotMatch(result.reply.content, /amber/);
});

test('a first hub reply reaches the model as it arrives, even mid-utterance; each follow-up waits until the model has spoken the reply before it, and barge-in drops the follow-ups still waiting', async () => {
  const result = await runWakePage(`
    const { LivePlayback } = await import('/live-playback.js');
    const quiet = [];
    LivePlayback.prototype.quiet = () => new Promise((resolve) => quiet.push(resolve));
    const replies = () => sentLiveEvents.filter(event => event.event_id?.startsWith('hub_ordered')).map(event => [event.delegation_id, event.content]);
    const shown = () => document.querySelector('#display-items').textContent;
    hear('Check the test beacon.');
    delegateTurn('first');
    await until(() => count('delegate'));
    hear(' While that runs, let me tell you');
    replyFromHub('first', 'ordered_first', ['First part.', 'Second part.'], 5, { first: true });
    replyFromHub('first', 'ordered_second', ['Slide two.'], 5, { display: { markdown: 'Slide two picture' } });
    replyFromHub('first', 'ordered_third', ['Slide three.']);
    replyFromHub('other', 'ordered_other', ['Other answer.'], 5, { first: true });
    await until(() => globalThis.hubAcks?.includes('ordered_other'));
    const held = { replies: replies(), shown: shown().includes('Slide two picture') };
    hear(' about the garden.');
    emitLive({ type: 'session.output_transcript.delta', delta: 'First part.' });
    const quiets = quiet.length;
    quiet.shift()();
    await until(() => replies().length === 4);
    const next = { replies: replies().slice(3), shown: shown().includes('Slide two picture') };
    emitLive({ type: 'session.output_transcript.delta', delta: 'Slide two.' });
    hear('Stop there.');
    await until(() => sessionEvents('hub-reply-dropped').length);
    return { held, quiets, next, final: replies().length, dropped: sessionEvents('hub-reply-dropped').map((event) => JSON.parse(event.detail)), acks: hubAcks, log: document.querySelector('#log').innerText.includes('hub reply: First part. Second part. Slide two.') };
  `);
  assert.deepEqual(result, {
    held: { replies: [['first', 'First part.'], ['first', 'Second part.'], [null, 'Other answer.']], shown: false },
    quiets: 1,
    next: { replies: [['first', 'Slide two.']], shown: true },
    final: 4,
    dropped: [{ id: 'first', stamp: 'ordered_third', reason: 'barge-in' }],
    acks: ['ordered_first', 'ordered_second', 'ordered_third', 'ordered_other'],
    log: true,
  });
});

test('a spoken hub reply that reaches an asleep page is acknowledged as unspoken', async () => {
  const result = await runWakePage(`
    await sleepNow();
    replyFromHub('late', 'late_stamp', ['Too late.'], 5, { first: true });
    await until(() => globalThis.hubAckFrames?.length);
    return hubAckFrames;
  `);
  assert.deepEqual(result, [{ id: 'late', stamp: 'late_stamp', unspoken: 'Live is asleep' }]);
});

test('leaving the app keeps the delegation log and whether Live was awake through a freeze, a discard-and-reload and a reload, but not a crash in view', async () => {
  const lifecycle = `for (const name of ['visibilitychange', 'freeze', 'resume', 'pagehide', 'pageshow']) document.addEventListener(name, () => sessionStorage.setItem('lifecycle', JSON.stringify([...JSON.parse(sessionStorage.getItem('lifecycle') ?? '[]'), name === 'visibilitychange' ? document.visibilityState : name])), true);`;
  const result = await driveLifecycle(untilAsleep + lifecycle, async ({ call, page, evaluate, until, event, events, targetId }) => {
    const { windowId } = await call('Browser.getWindowForTarget', { targetId });
    const state = () => evaluate(`({ log: document.querySelector('#log').innerText, pressed: document.querySelector('#puppet').getAttribute('aria-pressed') })`);
    const pressed = (value) => until(`document.querySelector('#puppet').getAttribute('aria-pressed') === '${value}'`);
    const tap = (value) => evaluate(`document.querySelector('#puppet').click(); 0`).then(() => pressed(value));
    const hide = async () => {
      await call('Browser.setWindowBounds', { windowId, bounds: { windowState: 'minimized' } });
      // The hidden page saves in its visibilitychange handler; seeing it hidden orders that save before any later kill.
      await until(`document.visibilityState === 'hidden'`);
    };
    const freeze = () => page('Page.setWebLifecycleState', { state: 'frozen' });
    const back = async () => {
      await page('Page.setWebLifecycleState', { state: 'active' });
      await call('Browser.setWindowBounds', { windowId, bounds: { windowState: 'normal' } });
      await until(`document.visibilityState === 'visible'`);
    };
    const reload = async () => {
      events.length = 0;
      await page('Page.reload');
      await event('lifecycle:load');
      await until(`document.querySelector('#puppet').getAttribute('aria-disabled') === 'false'`);
    };
    const discardAndReturn = async () => {
      events.length = 0;
      for (const { id, type } of (await call('SystemInfo.getProcessInfo')).processInfo) if (type === 'renderer') process.kill(id, 'SIGKILL');
      await event('crashed');
      await call('Browser.setWindowBounds', { windowId, bounds: { windowState: 'normal' } });
      await reload();
    };
    const restores = () => evaluate(`sessionEvents('restore').map((event) => JSON.parse(event.detail).awake)`);
    const offers = () => evaluate(`count('offer')`);
    const memory = () => evaluate(`JSON.parse(lastOffer.context[0].text.split('\\n').slice(1).join('\\n')).flatMap((conversation) => conversation.turns.map((turn) => turn.text))`);
    await until(`document.body.dataset.startTest === 'puppet'`);
    await evaluate(`hear('Where is the beacon?'); emitLive({ type: 'session.output_transcript.delta', delta: 'The beacon is green.' }); 0`);
    const awake = await state();
    events.length = 0;
    await hide();
    await freeze();
    await back();
    const frozen = { ...(await state()), lifecycle: await evaluate(`JSON.parse(sessionStorage.getItem('lifecycle'))`), navigated: events.filter((name) => name.startsWith('navigated')) };
    await hide();
    await evaluate(`hear('And the tower?'); 0`);
    await freeze();
    await discardAndReturn();
    await pressed('true');
    await until(`sessionEvents('restore').length === 1`);
    const reopened = { ...(await state()), memory: await memory(), restores: await restores() };
    await tap('false');
    const asleep = await state();
    await hide();
    await freeze();
    await discardAndReturn();
    await until(`sessionEvents('restore').length === 1`);
    await new Promise((resolve) => setTimeout(resolve, 500));
    const slept = { ...(await state()), offers: await offers() };
    await until(`globalThis.testSpotter?.closed === 0`);
    await hide();
    await evaluate(`testSpotter.heard({ wake: 0.9 }); 0`);
    await pressed('true');
    await freeze();
    await discardAndReturn();
    await pressed('true');
    await until(`sessionEvents('restore').length === 1`);
    const wokenHidden = await restores();
    await hide();
    await freeze();
    await back();
    await tap('false');
    const sleptInView = await state();
    await reload();
    await until(`sessionEvents('restore').length === 1`);
    await new Promise((resolve) => setTimeout(resolve, 500));
    const reloadedInView = { ...(await state()), restores: await restores(), offers: await offers() };
    await tap('true');
    await hide();
    await freeze();
    await back();
    await tap('false');
    await discardAndReturn();
    await new Promise((resolve) => setTimeout(resolve, 500));
    const killedInView = { ...(await state()), restores: await restores(), offers: await offers() };
    return { awake, frozen, reopened, asleep, slept, wokenHidden, sleptInView, reloadedInView, killedInView };
  });
  const log = /^MODEL HEARD\nheard: Where is the beacon\?\nMODEL ALONE\nspoke: The beacon is green\.\nMODEL HEARD\nheard: And the tower\?/;
  const withoutCost = (text) => text.replaceAll(/\nsession: [^\n]*/g, '');
  assert.equal(result.awake.pressed, 'true');
  assert.deepEqual(result.frozen, { ...result.awake, lifecycle: ['hidden', 'freeze', 'resume', 'visible'], navigated: [] });
  assert.match(result.reopened.log, log);
  assert.deepEqual(result.reopened.memory, ['Where is the beacon?', 'The beacon is green.', 'And the tower?']);
  assert.deepEqual(result.reopened.restores, [true]);
  assert.equal(result.asleep.pressed, 'false');
  assert.deepEqual(result.slept, { log: withoutCost(result.asleep.log), pressed: 'false', offers: 0 });
  assert.deepEqual(result.wokenHidden, [true]);
  assert.deepEqual(result.reloadedInView, { log: withoutCost(result.sleptInView.log), pressed: 'false', restores: [false], offers: 0 });
  assert.deepEqual(result.killedInView, { log: '', pressed: 'false', restores: [], offers: 0 });
});
