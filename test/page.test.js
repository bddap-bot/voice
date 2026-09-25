import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, mkdir, mkdtemp, readFile, readdir, rm, stat } from 'node:fs/promises';
import { constants } from 'node:fs';
import { createServer } from 'node:http';
import { join } from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';
import { INACTIVITY_MS, NAME, WAKE_PHRASE } from '../docs/identity.js';
import { WIDTH, WINDOW } from '../docs/wake.js';
import { assessSmoke, canvasAspectMatches, clipClearsStage, evidenceRegion, installSmokeMeasurements, smokeLimits, smokeStatusText, smokeViewports } from './smoke-measurements.js';

const execute = promisify(execFile);


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
const mockWasm = `
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
  lost = true;
  queued.length = 0;
  for (const receiver of waiting.splice(0)) receiver.reject(error);
};
export default async function initWasm() {}
export async function init() {}
export async function connect() { lost = false; }
export async function send_only(bytes) {
  const frame = dec.decode(bytes);
  (globalThis.sentVerbs ??= []).push(frame.split('\\n', 1)[0]);
  if (frame.startsWith('hub-ack\\n')) (globalThis.hubAcks ??= []).push(JSON.parse(frame.slice(8)).stamp);
  if (frame.startsWith('spans\\n')) (globalThis.spanBatches ??= []).push(JSON.parse(frame.slice(6)).spans);
  if (frame.startsWith('delegate\\n')) (globalThis.delegateFrames ??= []).push(JSON.parse(frame.slice(9)));
  if (frame.startsWith('telemetry\\n')) {
    const batch = JSON.parse(frame.slice(frame.indexOf('\\n') + 1));
    globalThis.telemetryBatches.push(batch.events);
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
  else if (frame === 'puppets') deliver(enc.encode('puppets\\n' + JSON.stringify({ active: '42', avatars: ['42', '43', '44'].map((id) => ({ id, size: 3, contentHash: 'hash-' + id, creditLine: '', licenseFlags: { creditRequired: false } })) })));
  else if (frame === 'clips') deliver(enc.encode('clips\\n' + JSON.stringify({ clips: [{ action: 'sit', name: 'sit.fbx', format: 'fbx', contentHash: 'sit-hash' }, { action: 'idle', name: 'idle.fbx', format: 'fbx', contentHash: 'idle-hash' }] })));
  else if (frame.startsWith('track\\n')) {
    const request = JSON.parse(frame.slice(frame.indexOf('\\n') + 1));
    const id = request.id;
    globalThis.transferOrder.push(id);
    const hash = request.modelHash + '-' + request.clipHash;
    const compressed = new Uint8Array(await new Response(new Blob([Uint8Array.from([7, 8, 9])]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer());
    deliver(enc.encode('track-start\\n' + JSON.stringify({ id, size: compressed.length, originalSize: 3, contentHash: hash, encoding: 'gzip' })));
    const prefix = enc.encode('track-chunk\\n' + id + '\\n');
    const chunk = new Uint8Array(prefix.length + compressed.length);
    chunk.set(prefix);
    chunk.set(compressed, prefix.length);
    deliver(chunk);
    deliver(enc.encode('track-end\\n' + id));
  }
  else if (frame.startsWith('puppet\\n')) {
    const id = JSON.parse(frame.slice(frame.indexOf('\\n') + 1)).id;
    globalThis.puppetRequests.push(id);
    globalThis.transferOrder.push(id);
    const compressed = new Uint8Array(await new Response(new Blob([Uint8Array.from([1, 2, Number(id) - 39])]).stream().pipeThrough(new CompressionStream('gzip'))).arrayBuffer());
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
  if (queued.length) return queued.shift();
  return new Promise((resolve, reject) => waiting.push({ resolve, reject }));
}
`;

const fakePuppet = `
export class PuppetRuntime {
  constructor(canvas) { this.canvas = canvas; this.calls = []; this.humanoidBone = 0; globalThis.testPuppet = this; }
  async load(bytes, initialClip, valid, beforeCommit) { await beforeCommit(); globalThis.firstVisible = { playable: initialClip.action, order: [...transferOrder] }; this.humanoidBone += initialClip.bytes.byteLength; const context = this.canvas.getContext('2d'); context.fillStyle = '#50c878'; context.fillRect(0, 0, this.canvas.width, this.canvas.height); return valid(); }
  async loadClips(entries) { const before = this.humanoidBone; this.humanoidBone += entries.reduce((sum, entry) => sum + entry.bytes.byteLength, 0); globalThis.clipMovement = { before, after: this.humanoidBone, loaded: entries.map((entry) => [entry.action, entry.format]) }; }
  pose(...args) { this.calls.push(['pose', ...args]); }
  gesture(...args) { this.calls.push(['gesture', ...args]); }
  look(...args) { this.calls.push(['look', ...args]); }
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
globalThis.emitTool = (id, name, args) => {
  const emit = (event) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'response.event', delegation_id: id, event }) }));
  emit({ type: 'response.created', response: { id, output: [] } });
  emit({ type: 'response.output_item.done', item: { type: 'function_call', call_id: id, name, arguments: JSON.stringify(args) } });
  emit({ type: 'response.completed', response: { id, output: [] } });
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
const microphoneTrack = { enabled: true, stop() {} };
const stream = { getTracks: () => [microphoneTrack], getAudioTracks: () => [microphoneTrack] };
globalThis.testMicrophoneTrack = microphoneTrack;
Object.defineProperty(navigator, 'mediaDevices', { value: { getUserMedia: async () => stream } });
class FakeMediaRecorder extends EventTarget {
  static isTypeSupported(type) { return type === 'audio/webm;codecs=opus'; }
  constructor() { super(); this.state = 'inactive'; }
  start() { this.state = 'recording'; }
  stop() { this.state = 'inactive'; this.dispatchEvent(new Event('stop')); }
}
globalThis.MediaRecorder = FakeMediaRecorder;
class FakeChannel extends EventTarget {
  constructor() { super(); this.readyState = 'open'; }
  send(value) {
    const event = JSON.parse(value);
    globalThis.sentLiveEvents.push(event);
    if (event.type === 'session.close') queueMicrotask(() => this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.closed', usage: { seconds: 0 } }) })));
  }
  close() { this.readyState = 'closed'; }
}
class FakePeerConnection {
  constructor() { this.iceGatheringState = 'complete'; this.localDescription = { sdp: 'offer' }; }
  createDataChannel() { this.channel = new FakeChannel(); globalThis.testChannel = this.channel; return this.channel; }
  async createOffer() { return { type: 'offer', sdp: 'offer' }; }
  async setLocalDescription(description) { this.localDescription = description; }
  async setRemoteDescription() { queueMicrotask(() => this.channel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.started', session: { delegation: { type: 'responses', responses: { tools: [{ type: 'function', name: 'hub' }] } } } }) }))); }
  addTrack() {}
  close() {}
}
globalThis.RTCPeerConnection = FakePeerConnection;
globalThis.__voiceStartSpotter = async (stream, heard, model) => {
  globalThis.testSpotter = { stream, heard, model, closed: 0 };
  return { close() { testSpotter.closed++; } };
};
globalThis.sentLiveEvents = [];
window.addEventListener('load', () => {
  const poll = globalThis.setInterval.bind(globalThis);
  globalThis.statusTextWrites = [];
  new MutationObserver(() => statusTextWrites.push(document.querySelector('#status').textContent)).observe(document.querySelector('#status'), { childList: true, characterData: true, subtree: true });
  document.querySelector('#token').value = btoa(JSON.stringify({ endpoint_id: 'test', secret: 'test' }));
  document.querySelector('#connect').click();
  const ready = poll(() => {
    if (document.querySelector('#puppet').getAttribute('aria-disabled') === 'true') return;
    clearInterval(ready);
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
  emitTool('scroll_' + index, 'hub', { text: 'question ' + index });
}
`;

async function chromiumExecutable() {
  if (process.env.CHROMIUM_BIN) {
    await access(process.env.CHROMIUM_BIN, constants.X_OK);
    return process.env.CHROMIUM_BIN;
  }
  const candidates = ['chromium', 'chromium-browser', 'google-chrome', 'google-chrome-stable'];
  try {
    const stores = await readdir('/nix/store');
    candidates.push(...stores.filter((name) => name.includes('-chromium-')).map((name) => `/nix/store/${name}/bin/chromium`));
  } catch {}
  for (const candidate of candidates) {
    const paths = candidate.includes('/') ? [candidate] : (process.env.PATH ?? '').split(':').map((directory) => join(directory, candidate));
    for (const path of paths) {
      try {
        await access(path, constants.X_OK);
        return path;
      } catch {}
    }
  }
  throw new Error('headless Chromium is required; set CHROMIUM_BIN');
}

async function runPage(testSetup = '', { scale = 1, size = '390,844', budget = 3000, mobile = false } = {}) {
  const index = (await readFile(new URL('../docs/index.html', import.meta.url), 'utf8'))
    .replace('https://bddap-bot.github.io/botq/botq_dash_wasm.js', '/botq_dash_wasm.js')
    .replace('./puppet.js', '/fake-puppet.js')
    .replace('</head>', () => `<script>${browserSetup}${testSetup}</script></head>`);
  const live = await readFile(new URL('../docs/live.js', import.meta.url));
  const puppetClient = await readFile(new URL('../docs/puppet-client.js', import.meta.url));
  const puppetTools = await readFile(new URL('../docs/puppet-tools.js', import.meta.url));
  const puppetDrivers = await readFile(new URL('../docs/puppet-drivers.js', import.meta.url));
  const scratch = await mkdtemp(join(process.cwd(), '.chromium-'));
  const profile = join(scratch, 'profile');
  const temporary = join(scratch, 'tmp');
  await Promise.all([mkdir(profile), mkdir(temporary)]);
  const requests = [];
  const server = createServer(async (request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    requests.push(path);
    const served = path === '/botq_dash_wasm.js' ? mockWasm : path === '/fake-puppet.js' ? fakePuppet : path === '/puppet-client.js' ? puppetClient : path === '/puppet-drivers.js' ? puppetDrivers : path === '/puppet-tools.js' ? puppetTools : path === '/live.js' ? live : path === '/' ? index : await readFile(new URL(`../docs${path}`, import.meta.url)).catch(() => null);
    if (served === null) { response.writeHead(404); response.end(); return; }
    response.writeHead(200, { 'content-type': path.endsWith('.js') ? 'text/javascript' : path.endsWith('.css') ? 'text/css' : path.endsWith('.woff2') ? 'font/woff2' : 'text/html' });
    response.end(served);
  });
  try {
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    const executable = await chromiumExecutable();
    const { stdout, stderr } = await execute(executable, [
      '--headless=new',
      '--no-sandbox',
      '--disable-gpu',
      `--force-device-scale-factor=${scale}`,
      `--window-size=${size}`,
      ...(mobile ? ['--user-agent=Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/140.0 Mobile Safari/537.36'] : []),
      `--user-data-dir=${profile}`,
      `--virtual-time-budget=${budget}`,
      '--dump-dom',
      `http://127.0.0.1:${server.address().port}/`,
    ], { timeout: 15000, killSignal: 'SIGKILL', env: { ...process.env, TMPDIR: temporary } });
    return { stdout, stderr, requests };
  } finally {
    if (server.listening) await new Promise((resolve) => server.close(resolve));
    await rm(scratch, { recursive: true, force: true });
  }
}

async function runPuppetPage(html, { scale = 1, size = '390,844', budget = 3000, mobile = false } = {}) {
  const puppet = await readFile(new URL('../docs/puppet.js', import.meta.url));
  const scratch = await mkdtemp(join(process.cwd(), '.chromium-'));
  const profile = join(scratch, 'profile');
  const temporary = join(scratch, 'tmp');
  await Promise.all([mkdir(profile), mkdir(temporary)]);
  const server = createServer((request, response) => {
    const body = request.url === '/puppet.js' ? puppet : html;
    response.writeHead(200, { 'content-type': request.url === '/puppet.js' ? 'text/javascript' : 'text/html' });
    response.end(body);
  });
  try {
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    const executable = await chromiumExecutable();
    return await execute(executable, [
      '--headless=new',
      '--no-sandbox',
      '--use-angle=swiftshader',
      '--enable-unsafe-swiftshader',
      `--force-device-scale-factor=${scale}`,
      `--window-size=${size}`,
      `--user-data-dir=${profile}`,
      `--virtual-time-budget=${budget}`,
      ...(mobile ? ['--user-agent=Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/140.0 Mobile Safari/537.36'] : []),
      '--dump-dom',
      `http://127.0.0.1:${server.address().port}/`,
    ], { timeout: 15000, killSignal: 'SIGKILL', env: { ...process.env, TMPDIR: temporary } });
  } finally {
    if (server.listening) await new Promise((resolve) => server.close(resolve));
    await rm(scratch, { recursive: true, force: true });
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
runtime.vrm = { update() {} };
runtime.animate(0);
runtime.pause();
const beforePlayable = renders;
runtime.clipAction = { isRunning: () => true, getClip: () => null };
runtime.animate(16);
runtime.pause();
document.body.dataset.visibilityTest = JSON.stringify({ beforePlayable, afterPlayable: renders });
runtime.dispose();
</script>`);
}

test('headless Chromium renders no puppet frame until an animation clip is playable', async () => {
  const { stdout, stderr } = await runVisibilityPage();
  const encoded = /data-visibility-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { beforePlayable: 0, afterPlayable: 1 }, stderr);
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

test('token entry is controlled by one compact settings button and the page has no heading', async () => {
  const index = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  assert.doesNotMatch(index, /<h1\b/i);
  assert.match(index, /<button id="token-toggle"[^>]+aria-controls="token-panel"[^>]+aria-expanded="true">⚙<\/button>/);
  assert.match(index, /<section id="token-panel"[\s\S]*?<div id="entry"[\s\S]*?<div id="saved"/);
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const toggle = document.querySelector('#token-toggle');
  const panel = document.querySelector('#token-panel');
  const states = [{ expanded: toggle.getAttribute('aria-expanded'), hidden: panel.classList.contains('hidden') }];
  toggle.click();
  states.push({ expanded: toggle.getAttribute('aria-expanded'), hidden: panel.classList.contains('hidden') });
  toggle.click();
  states.push({ expanded: toggle.getAttribute('aria-expanded'), hidden: panel.classList.contains('hidden') });
  document.body.dataset.tokenPanelTest = JSON.stringify(states);
});
`);
  const encoded = /data-token-panel-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), [
    { expanded: 'false', hidden: true },
    { expanded: 'true', hidden: false },
    { expanded: 'false', hidden: true },
  ], stderr);
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

test('the stage has no decorative wall occluders', async () => {
  const index = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  assert.doesNotMatch(index, /main::(?:before|after)/);
});

for (const viewport of layoutViewports) test(`stage UI stays outside the puppet projection at ${viewport.name} size`, async () => {
  const index = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  const style = /<style>[\s\S]*?<\/style>/.exec(index)[0];
  const main = /<main[\s\S]*?<\/main>/.exec(index)[0].replace('class="hidden"', '');
  const chrome = '<header><span id="status"></span><button id="token-toggle">⚙</button></header><section class="token-panel hidden"><div id="saved" class="saved"><span>device authenticated</span><button>Forget token</button></div></section>';
  const { stdout, stderr } = await runPuppetPage(`<!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1">${style}</head><body>${chrome}${main}<script type="module">
  import { PuppetRuntime } from '/puppet.js';
  const stageHeight = Math.round(visualViewport?.height ?? innerHeight);
  document.documentElement.style.setProperty('--visual-viewport-height', stageHeight + 'px');
  document.querySelector('main').style.setProperty('--stage-height', stageHeight + 'px');
  const runtime = new PuppetRuntime(document.querySelector('#puppet'));
  runtime.pause();
  runtime.camera.updateMatrixWorld();
  const origin = runtime.camera.position.clone().set(0, 0, 0);
  const feet = origin.project(runtime.camera);
  runtime.dispose();
  const rect = (element) => { const value = element.getBoundingClientRect(); return { left: value.left, right: value.right, top: value.top, bottom: value.bottom }; };
  const puppet = rect(document.querySelector('#puppet'));
  const figure = { ...puppet, bottom: puppet.top + (1 - feet.y) / 2 * (puppet.bottom - puppet.top) };
  const selectors = ['header', '#saved', '.puppet-picker', '#puppet-credit', '#elapsed', '#mic-mute', '.share', '.display', '.ledger'];
  const overlaps = (a, b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
  const intrusions = () => selectors.map((selector) => ({ selector, rect: rect(document.querySelector(selector)) })).filter((item) => overlaps(item.rect, figure));
  const regions = ['header', '#saved', '#puppet', '.puppet-picker', '#puppet-credit', '#elapsed', '#mic-mute', '.share', '.display', '.ledger'].map((selector) => ({ selector, rect: rect(document.querySelector(selector)) })).filter(({ rect }) => rect.right > rect.left && rect.bottom > rect.top);
  const collisions = regions.flatMap((left, index) => regions.slice(index + 1).filter((right) => overlaps(left.rect, right.rect)).map((right) => ({ left, right })));
  const result = intrusions();
  const display = rect(document.querySelector('#display'));
  const ledger = rect(document.querySelector('.ledger'));
  const share = rect(document.querySelector('.share'));
  const shareControls = [...document.querySelectorAll('.share textarea, .share-actions label, .share-actions button, #share-image')].filter((element) => !element.classList.contains('hidden')).map(rect);
  document.querySelector('#display').classList.add('fresh');
  const fresh = { result: intrusions(), display: rect(document.querySelector('#display')) };
  const stage = rect(document.querySelector('main'));
  document.body.dataset.overlapTest = JSON.stringify({ puppet, figure, result, fresh, collisions, display, ledger, share, shareControls, pageHeight: document.documentElement.scrollHeight, viewportHeight: innerHeight, stageHeight, stage });
  </script></body></html>`, { scale: viewport.scale, size: `${viewport.width},${viewport.height}`, mobile: viewport.mobile });
  const encoded = /data-overlap-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.deepEqual(result?.result, [], `${viewport.name}: ${JSON.stringify(result)}\n${stderr}`);
  assert.deepEqual(result.fresh.result, [], `${viewport.name} with a fresh display: ${JSON.stringify(result)}`);
  assert.deepEqual(result.collisions, [], `${viewport.name}: ${JSON.stringify(result.collisions)}`);
  assert.equal(stdout.includes('tap to stand and start voice'), false, `${viewport.name} retains the old button copy`);
  assert.equal(/<button[^>]+id="toggle"/.test(stdout), false, `${viewport.name} retains the old button`);
  const puppetWidth = result.puppet.right - result.puppet.left;
  assert.ok(result.figure.bottom > result.puppet.top + 0.8 * (result.puppet.bottom - result.puppet.top), `${viewport.name} projected feet must sit near the canvas bottom: ${JSON.stringify(result.figure)}`);
  if (!viewport.mobile) {
    for (const wing of [result.display, result.ledger]) assert.ok(wing.top >= 200 && wing.bottom <= result.stageHeight - 160, `${viewport.name} panel escapes its shared layout bounds: ${JSON.stringify(wing)}`);
    assert.ok(result.pageHeight <= viewport.height, `${viewport.name} must remain one screen: ${result.pageHeight}`);
    assert.equal(result.puppet.top, 0, `${viewport.name} puppet must start at the top of the stage`);
    assert.equal(result.puppet.bottom, result.stageHeight - 90, `${viewport.name} puppet must end above the desk: ${result.puppet.bottom} of ${result.stageHeight}`);
    assert.ok(result.share.top >= result.puppet.bottom, `${viewport.name} share control intersects the puppet canvas: ${JSON.stringify(result)}`);
    for (const wing of [result.display, result.ledger]) assert.ok(wing.right - wing.left < puppetWidth, `${viewport.name} wing wider than the puppet: ${JSON.stringify(wing)}`);
    assert.ok(result.fresh.display.right - result.fresh.display.left > result.display.right - result.display.left, `${viewport.name} fresh display must grow: ${JSON.stringify(result.fresh.display)}`);
  } else {
    for (const control of result.shareControls) assert.ok(control.left >= result.share.left && control.right <= result.share.right && control.top >= result.share.top && control.bottom <= result.share.bottom, `${viewport.name} share control is clipped: ${JSON.stringify({ control, share: result.share })}`);
    assert.deepEqual(result.fresh.display, result.display, 'phone display must not move when fresh');
  }
});

for (const viewport of layoutViewports) test(`the token control stays within its stage column and stationary across status changes at ${viewport.name} size`, async () => {
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
  assert.ok(result.boxes.empty[2] <= Math.min(result.viewportWidth * .32 - 44, 430), `${viewport.name} token control exceeds its stage column: ${JSON.stringify(result.boxes.empty)}`);
  for (const [name, box] of Object.entries(result.boxes)) assert.ok(box[2] < result.viewportWidth, `${viewport.name} header overflows with ${name} status text: ${JSON.stringify(box)}`);
  for (const [name, box] of Object.entries(result.boxes)) assert.deepEqual(box, result.boxes.empty, `${viewport.name} token control moved with ${name} status text: ${JSON.stringify(result.boxes)}`);
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
  emitTool('state_1', 'hub', { text: 'Where is the report?' });
  const waiting = text();
  deliverRelay(new TextEncoder().encode('hub\\n' + JSON.stringify({ id: 'state_1', reply: 'On the display.', timing_ms: 12, stamp: 'state_stamp' })));
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
  setTimeout(() => { document.body.dataset.telemetrySessionTest = JSON.stringify(telemetryBatches.flat().filter((event) => event.kind === 'session').map((event) => event.name)); }, 300);
});
`);
  const encoded = /data-telemetry-session-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), ['open', 'close'], stderr);
});

test('the page loads a clip, moves a humanoid bone, and preloads inactive puppets', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => setTimeout(() => {
  document.body.dataset.preloadTest = JSON.stringify({ requests: puppetRequests, cacheKeys: [...puppetCache.keys()].map((url) => new URL(url).pathname.split('/').at(-1)), clipMovement, firstVisible });
}, 100));
`);
  const encoded = /data-preload-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { requests: ['42', '43', '44'], cacheKeys: ['hash-42-idle-hash.json', 'hash-42.vrm', 'hash-42-sit-hash.json', 'hash-43.vrm', 'hash-44.vrm'], clipMovement: { before: 3, after: 6, loaded: [['sit', 'tracks']] }, firstVisible: { playable: 'idle', order: ['idle.fbx', '42'] } }, stderr);
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
  const freshMs = Number(/DISPLAY_FRESH_MS = (\d+)/.exec(await readFile(new URL('../docs/index.html', import.meta.url), 'utf8'))[1]);
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const enc = new TextEncoder();
  const metadata = enc.encode('display\\n' + JSON.stringify({ markdown: '**Result** details', link: 'https://example.test/result', image: { mime: 'image/png' } }) + '\\n');
  const image = Uint8Array.from([137,80,78,71,13,10,26,10]);
  const frame = new Uint8Array(metadata.length + image.length);
  frame.set(metadata); frame.set(image, metadata.length);
  deliverRelay(enc.encode('hub\\n' + JSON.stringify({ id: 'unknown', reply: 'plain', timing_ms: 1 })));
  deliverRelay(frame);
  deliverRelay(enc.encode('display\\n' + JSON.stringify({ markdown: 'Newest' }) + '\\n'));
  const fresh = () => document.querySelector('#display').classList.contains('fresh');
  setTimeout(() => {
    const arrived = fresh();
    setTimeout(() => { document.body.dataset.displayTest = JSON.stringify({ count: document.querySelectorAll('.display-item').length, first: document.querySelector('.display-item')?.textContent, link: document.querySelector('.display-item:last-child a')?.href, image: Boolean(document.querySelector('.display-item:last-child img')), pointed: testPuppet.calls.some((call) => call[0] === 'gesture' && call[1] === 'point' && call[2] === 'panel'), arrived, settled: !fresh() }); }, ${freshMs + 1000});
  }, 40);
});
`, { budget: freshMs + 5000 });
  const encoded = /data-display-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { count: 2, first: 'Newest', link: 'https://example.test/result', image: true, pointed: true, arrived: true, settled: true }, stderr);
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
  emitTool('fixture', 'hub', { text: 'Check the fixture stream.' });
  deliverRelay(new TextEncoder().encode('hub\\n' + JSON.stringify({ id: 'fixture', reply: 'The fixture is complete.', timing_ms: 42, stamp: 'fixture' })));
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
  emitTool('scroll_newest', 'hub', { text: 'newest' });
  document.body.dataset.pauseTest = String(log.scrollTop === before);
});
`);
  const observed = /data-pause-test="([^"]*)"/.exec(stdout)?.[1] ?? 'pause test did not run';
  assert.equal(observed, 'true', `${observed}\n${stderr}`);
});

test('completed puppet tool calls execute locally and are acknowledged to Live', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  emitTool('call_1', 'perform', { steps: [{ mood: 'amused' }] });
  setTimeout(() => {
    document.body.dataset.toolTest = JSON.stringify({ calls: testPuppet.calls.filter(([name]) => name === 'mood'), events: sentLiveEvents.filter(({ type }) => type === 'response.item.create' || type === 'response.create').map(({ type }) => type) });
  }, 20);
});
`);
  const encoded = /data-tool-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { calls: [['mood', 'amused']], events: ['response.item.create', 'response.create'] }, stderr);
});

test('a delegated turn reaches the relay as one trace whose hub call carries the traceparent', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const pause = () => new Promise((resolve) => setTimeout(resolve, 30));
  const event = (value) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(value) }));
  const round = (id, item) => {
    for (const nested of [{ type: 'response.created', response: { id, output: [] } }, { type: 'response.output_item.done', item }, { type: 'response.completed', response: { id, output: [] } }]) event({ type: 'response.event', delegation_id: 'dlg', event: nested });
  };
  event({ type: 'session.input_transcript.delta', delta: 'Nod, then check the weather.' });
  event({ type: 'session.delegation.created', delegation: { id: 'dlg', target: 'responses' } });
  round('r1', { type: 'function_call', call_id: 'nod_1', name: 'perform', arguments: JSON.stringify({ steps: [{ gesture: 'nod' }] }) });
  await pause();
  round('r2', { type: 'function_call', call_id: 'hub_1', name: 'hub', arguments: JSON.stringify({ text: 'check the weather' }) });
  await pause();
  deliverRelay(new TextEncoder().encode('hub\\n' + JSON.stringify({ id: 'hub_1', reply: 'Sunny.', timing_ms: 5, stamp: 'stamp_1' })));
  await pause();
  round('r3', { type: 'message', content: [] });
  await pause();
  event({ type: 'session.output_transcript.delta', delta: 'Sunny.' });
  await new Promise((resolve) => setTimeout(resolve, 200));
  document.body.dataset.stageTest = JSON.stringify({ batches: globalThis.spanBatches ?? [], delegates: globalThis.delegateFrames ?? [] });
});
`);
  const encoded = /data-stage-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const { batches, delegates } = JSON.parse(encoded ?? 'null') ?? {};
  const spans = batches?.flat() ?? [];
  assert.deepEqual(spans.map((span) => span.name).sort(), ['await-speech', 'decide', 'hear', 'respond', 'respond', 'respond', 'resume', 'resume', 'tool', 'tool', 'turn'], stderr);
  assert.ok(spans.every((span) => span.traceId === spans[0].traceId && span.status === undefined));
  const hub = spans.find((span) => span.name === 'tool' && span.attributes.some(({ key, value }) => key === 'tool.name' && value.stringValue === 'hub'));
  assert.deepEqual(delegates.map(({ id, traceparent }) => [id, traceparent]), [['hub_1', `00-${hub.traceId}-${hub.spanId}-01`]]);
});

test('a hub call returns at once and the hub reply reaches Live later as commentary, while a display-only push says nothing', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const pause = () => new Promise((resolve) => setTimeout(resolve, 30));
  const relay = (verb, value) => deliverRelay(new TextEncoder().encode(verb + '\\n' + JSON.stringify(value)));
  const told = () => sentLiveEvents.filter(({ type, event_id }) => type === 'session.commentary.append' && event_id.startsWith('hub_')).map(({ delegation_id, content }) => [delegation_id, content]);
  emitTool('slow', 'hub', { text: 'How many jobs are queued?' });
  await pause();
  const returned = sentLiveEvents.filter(({ type }) => type === 'response.item.create' || type === 'response.create').map(({ type, item }) => item ? [item.call_id, JSON.parse(item.output).ok] : type);
  relay('hub', { id: 'slow', reply: '', timing_ms: 5, stamp: 'push_1' });
  await pause();
  const pushed = { told: told(), waiting: testPuppet.calls.filter(([name]) => name === 'waiting').map(([, value]) => value) };
  relay('hub', { id: 'slow', reply: 'Four jobs are queued.', timing_ms: 180000, stamp: 'reply_1' });
  await pause();
  relay('hub-error', { id: 'slow', message: 'invalid hub reply' });
  emitTool('lost', 'hub', { text: 'Is the printer busy?' });
  await pause();
  relay('hub-error', { id: 'lost', message: 'delegation queue is full' });
  await pause();
  document.body.dataset.asyncHubTest = JSON.stringify({ returned, pushed, told: told(), acks: globalThis.hubAcks ?? [], waiting: testPuppet.calls.filter(([name]) => name === 'waiting').map(([, value]) => value), log: document.querySelector('#log').innerText });
});
`);
  const encoded = /data-async-hub-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result, stderr);
  assert.deepEqual(result.returned, [['slow', true], 'response.create']);
  assert.deepEqual(result.pushed, { told: [], waiting: [true] });
  assert.deepEqual(result.told, [[null, 'Four jobs are queued.'], [null, 'The hub request failed.']]);
  assert.match(result.log, /Is the printer busy\?[\s\S]*delegation queue is full/);
  assert.deepEqual(result.acks, ['push_1', 'reply_1']);
  assert.deepEqual(result.waiting, [true, false, true, false]);
  assert.match(result.log, /sent to hub: How many jobs are queued\?[\s\S]*hub reply: Four jobs are queued\./);
});

test('output transcript drives mood and delegation drives the waiting pose', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const event = (value) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(value) }));
  event({ type: 'session.output_transcript.delta', delta: 'Sorry, that was my fault.' });
  event({ type: 'session.input_transcript.delta', delta: 'check it' });
  emitTool('wait_1', 'hub', { text: 'check it' });
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
  deliverRelay(enc.encode('display\\n' + JSON.stringify({ markdown: ${JSON.stringify(markdown)} }) + '\\n'));
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
  deliverRelay(enc.encode('display\\n' + JSON.stringify({ markdown: ${JSON.stringify(markdown)} }) + '\\n'));
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
  deliverRelay(enc.encode('display\\n' + JSON.stringify({ markdown: 'Spent $9 on the pizza and $3 more, see https://example.test/receipt?id=1). Ended.' }) + '\\n'));
  setTimeout(() => {
    const item = document.querySelector('.display-item');
    document.body.dataset.linkTest = JSON.stringify({ text: item.textContent, links: [...item.querySelectorAll('a')].map((a) => [a.href, a.rel]), math: item.querySelectorAll('.math').length });
  }, 500);
});
`);
  const encoded = /data-link-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { text: 'Spent $9 on the pizza and $3 more, see https://example.test/receipt?id=1). Ended.', links: [['https://example.test/receipt?id=1', 'noopener noreferrer']], math: 0 }, stderr);
  assert.ok(await libBytes(requests) < 8192, requests.join(' '));
});

test('adopted renderer output loses scripts, handlers, remote references and unsafe links in both SVG and HTML', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const { adopt } = await import('/lib/render.js');
  const svg = document.createElement('div');
  adopt(svg, '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" onload="alert(1)"><script>alert(2)<\\/script><style>.a{fill:red}</style><a href="javascript:alert(3)"><text onclick="x()">t</text></a><a href="https://ok.test/"><text>k</text></a><use xlink:href="https://evil.test/x.svg#y"/><use href="#local"/><image href="https://evil.test/a.png"/><foreignObject><div onmouseover="y()">f</div></foreignObject></svg>', 'image/svg+xml');
  const html = document.createElement('div');
  adopt(html, '<span class="katex"><img src="x" onerror="alert(4)"><a href="https://ok.test/">k</a><a href="data:text/html,x">d</a><iframe srcdoc="x"></iframe></span>', 'text/html');
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
  for (const kept of ['<style>.a{fill:red}</style>', 'href="https://ok.test/" target="_blank" rel="noopener noreferrer"', 'href="#local"', '<foreignObject><div>f</div></foreignObject>']) assert.ok(result.svg.includes(kept), kept + ': ' + result.svg);
  assert.ok(result.html.includes('href="https://ok.test/" target="_blank" rel="noopener noreferrer"') && result.html.includes('<img>'), result.html);
});

test('Responses delegation holds early speech until the final backend response', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  const emit = (event) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(event) }));
  emit({ type: 'session.delegation.created', delegation: { id: 'd', target: 'responses' } });
  emit({ type: 'session.output_transcript.delta', delta: 'Okay.' });
  const early = testPuppet.calls.filter(([name]) => name === 'speak').length;
  const nested = (event) => emit({ type: 'response.event', delegation_id: 'd', event });
  nested({ type: 'response.created', response: { id: 'r1' } });
  nested({ type: 'response.output_item.done', item: { type: 'function_call', call_id: 'a', name: 'perform', arguments: '{"steps":[{"mood":"amused"}]}' } });
  nested({ type: 'response.completed', response: { id: 'r1', output: [] } });
  await new Promise(resolve => setTimeout(resolve, 10));
  const pending = testPuppet.calls.filter(([name]) => name === 'speak').length;
  nested({ type: 'response.created', response: { id: 'r2' } });
  nested({ type: 'response.completed', response: { id: 'r2', output: [] } });
  setTimeout(() => { document.body.dataset.playbackTest = JSON.stringify({ early, pending, calls: testPuppet.calls.filter(([name]) => name === 'mood' || name === 'speak').map(([name, value]) => [name, value]) }); }, 20);
});
`);
  const encoded = /data-playback-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { early: 0, pending: 0, calls: [['mood', 'amused'], ['speak', 'Okay.']] }, stderr);
});

for (const close of ["testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.closed' }) }))", "testChannel.dispatchEvent(new Event('close'))"]) test('provider close offers a fresh session carrying history: ' + close, async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', async () => {
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.input_transcript.delta', delta: 'Remember the earlier question.' }) }));
  ${close};
  await new Promise(resolve => setTimeout(resolve, 30));
  const offered = document.querySelector('#status').textContent;
  document.querySelector('#puppet').click();
  await new Promise(resolve => setTimeout(resolve, 50));
  document.body.dataset.resumeTest = JSON.stringify({ offered, active: document.querySelector('#puppet').getAttribute('aria-pressed'), context: lastOffer.context });
});
`);
  const encoded = /data-resume-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { offered: 'Session ended — tap to continue', active: 'true', context: [{ speaker: 'user', text: 'Remember the earlier question.' }] }, stderr);
});

test('perform requests reach the trace while animation deltas and input utterances reach session telemetry', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.delegation.created', delegation: { id: 'pose_probe', target: 'responses' } }) }));
  emitTool('pose_probe', 'perform', { steps: [{ pose: 'sit' }] });
  testPuppet.onAnimation({ clips: [{ name: 'sit', weight: 0.5 }], hip_height: 1.2 });
  for (const event of [{ type: 'input_audio_buffer.speech_started' }, { type: 'session.input_transcript.delta', delta: 'Please stand.' }]) {
    testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(event) }));
  }
  setTimeout(() => { document.body.dataset.poseTelemetry = JSON.stringify({ events: telemetryBatches.flat().filter((event) => ['tool_call', 'animation', 'input_utterance', 'input_speech_started'].includes(event.name)), tools: (globalThis.spanBatches ?? []).flat().filter((span) => span.name === 'tool') }); }, 600);
});
`);
  const encoded = /data-pose-telemetry="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const { events, tools } = JSON.parse(encoded ?? 'null') ?? {};
  assert.ok(events, stderr);
  assert.equal(events.length, 3);
  assert.deepEqual(tools.map((span) => Object.fromEntries(span.attributes.map(({ key, value }) => [key, value.stringValue]))), [{ 'tool.name': 'perform', 'tool.arguments': '{"steps":[{"pose":"sit"}]}' }]);
  assert.deepEqual(JSON.parse(events.find((event) => event.name === 'animation').detail), { clips: [{ name: 'sit', weight: 0.5 }], hip_height: 1.2 });
  assert.equal(events.find((event) => event.name === 'input_utterance').detail, 'Please stand.');
  assert.ok(events.every((event) => event.session_id === events[0].session_id && event.at > 0));
});

test('display pipe tables preserve rows, inline links and pipes inside code', async () => {
  const markdown = '| Name | Value |\n| :--- | ---: |\n| First | one |\n| Second | two |\nAfter\n\n| Link | Code | Empty |\n| --- | :---: | --- |\n| [docs](https://example.test/docs) | `left|right` | |\n\n| ordinary | prose |\n| not a separator | text |';
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const enc = new TextEncoder();
  deliverRelay(enc.encode('display\\n' + JSON.stringify({ markdown: ${JSON.stringify(markdown)} }) + '\\n'));
  setTimeout(() => {
    const item = document.querySelector('.display-item');
    document.body.dataset.tableTest = JSON.stringify({
      tables: [...item.querySelectorAll('table')].map((table) => ({
        head: [...table.querySelectorAll('thead tr')].map((row) => [...row.querySelectorAll('th')].map((cell) => cell.textContent)),
        body: [...table.querySelectorAll('tbody tr')].map((row) => [...row.querySelectorAll('td')].map((cell) => cell.textContent)),
      })),
      link: [...item.querySelectorAll('td a')].map((link) => [link.textContent, link.href, link.rel]),
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
    link: [['docs', 'https://example.test/docs', 'noopener noreferrer']],
    code: ['left|right'],
    paragraphs: ['After', '| ordinary | prose |', '| not a separator | text |'],
  }, stderr);
});

const untilAsleep = `
const until = async (check) => { while (!check()) await new Promise((resolve) => setTimeout(resolve, 10)); };
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

test('the wake phrase wakes the puppet into one session that learns only its name, its way back to sleep, and that it was woken', async () => {
  const result = await runWakePage(`
    await sleepNow();
    const offers = count('offer');
    testSpotter.heard({ wake: 0.93 });
    testSpotter.heard({ wake: 0.95 });
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true');
    testSpotter.heard({ wake: 0.97 });
    await new Promise((resolve) => setTimeout(resolve, 300));
    return {
      offers: count('offer') - offers,
      wakes: sessionEvents('wake').map((event) => event.detail),
      puppet: testPuppet.calls.filter(([name]) => name === 'asleep' || name === 'pose'),
      live: sentLiveEvents.map((event) => event.type === 'session.update' ? { type: event.type, tools: event.session.delegation.responses.tools.map((tool) => tool.name) } : { type: event.type, delegation_id: event.delegation_id, content: event.content }),
    };
  `);
  assert.equal(result.offers, 1);
  assert.deepEqual(result.wakes, ['0.930']);
  assert.deepEqual(result.puppet, [['asleep', false], ['pose', 'stand'], ['pose', 'listen']]);
  assert.deepEqual(result.live, [
    { type: 'session.update', tools: ['hub', 'sleep'] },
    { type: 'session.instructions.append', delegation_id: null, content: `Your name is ${NAME}. The Responses backend can end this session, which puts you back to sleep.` },
    { type: 'session.commentary.append', delegation_id: null, content: `Context: ${NAME} was just woken.` },
  ]);
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

test('the sleep tool ends the session once speech goes quiet and the puppet falls asleep listening again', async () => {
  const result = await runWakePage(`
    const { LivePlayback } = await import('/live-playback.js');
    let quiet;
    LivePlayback.prototype.quiet = () => new Promise((resolve) => { quiet = resolve; });
    emitTool('farewell', 'sleep', {});
    await new Promise((resolve) => setTimeout(resolve, 200));
    const speaking = document.querySelector('#puppet').getAttribute('aria-pressed');
    quiet?.();
    await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'false' && sessionEvents('close').length);
    return {
      speaking,
      sleeps: sessionEvents('sleep').map((event) => event.detail),
      puppet: testPuppet.calls.filter(([name]) => name === 'asleep').at(-1),
      microphone: { enabled: testMicrophoneTrack.enabled, button: document.querySelector('#mic-mute').disabled },
      rewoken: await (async () => { testSpotter.heard({ wake: 0.9 }); await until(() => document.querySelector('#puppet').getAttribute('aria-pressed') === 'true'); return true; })(),
    };
  `);
  assert.deepEqual(result, { speaking: 'true', sleeps: ['farewell'], puppet: ['asleep', true], microphone: { enabled: true, button: false }, rewoken: true });
});

test('inactivity sleeps after the generous window even while a hub request is pending', async () => {
  const result = await runWakePage(`
    emitTool('slow', 'hub', { text: 'Take your time.' });
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
