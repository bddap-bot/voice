import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, mkdir, mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { createServer } from 'node:http';
import { join } from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';
import { assessSmoke, smokeLimits, smokeViewports, transitionFrameSampler } from './smoke-measurements.js';

const execute = promisify(execFile);

test('transition frame sampling excludes work outside each transition window', () => {
  const callbacks = new Map();
  let nextId = 0;
  const sampler = transitionFrameSampler([], 50, (callback) => {
    callbacks.set(++nextId, callback);
    return nextId;
  }, (id) => callbacks.delete(id));
  sampler.start();
  const first = callbacks.get(1);
  callbacks.delete(1);
  first(0);
  assert.equal(callbacks.size, 1);
  sampler.stop();
  assert.equal(callbacks.size, 0);
  sampler.start();
  assert.equal(callbacks.size, 1);
  sampler.stop();
  assert.equal(callbacks.size, 0);
});

const mockWasm = `
const enc = new TextEncoder();
const dec = new TextDecoder();
const queued = [enc.encode(JSON.stringify({ ok: true }))];
const waiting = [];
globalThis.puppetRequests = [];
globalThis.transferOrder = [];
globalThis.telemetryBatches = [];
globalThis.fleetLines = [];
function deliver(value) {
  const resolve = waiting.shift();
  if (resolve) resolve(value);
  else queued.push(value);
}
globalThis.deliverRelay = deliver;
export default async function initWasm() {}
export async function init() {}
export async function connect() {}
export async function send_only(bytes) {
  const frame = dec.decode(bytes);
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
  else if (frame.startsWith('offer\\n')) {
    const offer = JSON.parse(frame.slice(6));
    deliver(enc.encode('answer\\n' + JSON.stringify({ offer_id: offer.id, sdp: 'answer', cap_seconds: 60 })));
  }
  else if (frame.startsWith('share\\n')) {
    const metadata = JSON.parse(frame.slice(6, frame.indexOf('\\n', 6)));
    globalThis.sentShare = { metadata, size: bytes.length };
    deliver(enc.encode((metadata.text === 'reject' ? 'share-error\\n' + JSON.stringify({ id: metadata.id, message: 'hub queue is full' }) : 'share-ok\\n' + JSON.stringify({ id: metadata.id, stamp: 'share_stamp', summary: 'A link arrived.' }))));
  }
}
export async function recv() {
  if (queued.length) return queued.shift();
  return new Promise((resolve) => waiting.push(resolve));
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
  listening(...args) { this.calls.push(['listening', ...args]); }
  start() {}
  pause() {}
  clear() {}
  async attachAudio() {}
  async detachAudio() {}
  dispose() {}
}
`;

const browserSetup = `
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
const stream = { getTracks: () => [{ stop() {} }] };
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
  async setRemoteDescription() { queueMicrotask(() => this.channel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.started' }) }))); }
  addTrack() {}
  close() {}
}
globalThis.RTCPeerConnection = FakePeerConnection;
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
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.delegation.created', delegation: { id: 'scroll_' + index } }) }));
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
    .replace('</head>', `<script>${browserSetup}${testSetup}</script></head>`);
  const live = await readFile(new URL('../docs/live.js', import.meta.url));
  const puppetClient = await readFile(new URL('../docs/puppet-client.js', import.meta.url));
  const puppetTools = await readFile(new URL('../docs/puppet-tools.js', import.meta.url));
  const puppetDrivers = await readFile(new URL('../docs/puppet-drivers.js', import.meta.url));
  const scratch = await mkdtemp(join(process.cwd(), '.chromium-'));
  const profile = join(scratch, 'profile');
  const temporary = join(scratch, 'tmp');
  await Promise.all([mkdir(profile), mkdir(temporary)]);
  const server = createServer((request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    const body = path === '/botq_dash_wasm.js' ? mockWasm : path === '/fake-puppet.js' ? fakePuppet : path === '/puppet-client.js' ? puppetClient : path === '/puppet-drivers.js' ? puppetDrivers : path === '/puppet-tools.js' ? puppetTools : path === '/live.js' ? live : index;
    response.writeHead(200, { 'content-type': path.endsWith('.js') ? 'text/javascript' : 'text/html' });
    response.end(body);
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
    return { stdout, stderr };
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
runtime.clipAction = { isRunning: () => true };
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

const layoutViewports = smokeViewports;

test('smoke assessment rejects every measured browser failure and accepts a clean run', () => {
  const clean = { cls: smokeLimits.cumulativeLayoutShift, moves: [], overlaps: [], heights: [{ canvas: 100, stage: 200 }, { canvas: 101, stage: 201 }], blankFrames: [], frameGaps: [], errors: [], telemetryRejections: [] };
  assert.equal(assessSmoke(clean).pass, true);
  for (const mutation of [
    { cls: smokeLimits.cumulativeLayoutShift + 0.001 },
    { moves: [{}] }, { overlaps: [{}] },
    { heights: [{ canvas: 100, stage: 200 }, { canvas: 102, stage: 200 }] },
    { blankFrames: [1] }, { frameGaps: [51] }, { errors: ['fault'] }, { telemetryRejections: ['rejected'] },
  ]) assert.equal(assessSmoke({ ...clean, ...mutation }).pass, false, JSON.stringify(mutation));
});

for (const viewport of layoutViewports) test(`stage UI stays outside the puppet projection at ${viewport.name} size`, async () => {
  const index = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  const style = /<style>[\s\S]*?<\/style>/.exec(index)[0];
  const main = /<main[\s\S]*?<\/main>/.exec(index)[0].replace('class="hidden"', '');
  const chrome = '<header><h1>voice</h1><span id="status"></span></header><section id="saved" class="saved"><span>device authenticated</span><button>Forget token</button></section>';
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
  const selectors = ['header', '#saved', '.puppet-picker', '#puppet-credit', '#elapsed', '.share', '.display', '.ledger'];
  const overlaps = (a, b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
  const intrusions = () => selectors.map((selector) => ({ selector, rect: rect(document.querySelector(selector)) })).filter((item) => overlaps(item.rect, figure));
  const result = intrusions();
  const display = rect(document.querySelector('#display'));
  const ledger = rect(document.querySelector('.ledger'));
  document.querySelector('#display').classList.add('fresh');
  const fresh = { result: intrusions(), display: rect(document.querySelector('#display')) };
  document.body.dataset.overlapTest = JSON.stringify({ puppet, figure, result, fresh, display, ledger, pageHeight: document.documentElement.scrollHeight, stageHeight });
  </script></body></html>`, { scale: viewport.scale, size: `${viewport.width},${viewport.height}`, mobile: viewport.mobile });
  const encoded = /data-overlap-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.deepEqual(result?.result, [], `${viewport.name}: ${JSON.stringify(result)}\n${stderr}`);
  assert.deepEqual(result.fresh.result, [], `${viewport.name} with a fresh display: ${JSON.stringify(result)}`);
  assert.equal(stdout.includes('tap to stand and start voice'), false, `${viewport.name} retains the old button copy`);
  assert.equal(/<button[^>]+id="toggle"/.test(stdout), false, `${viewport.name} retains the old button`);
  const puppetWidth = result.puppet.right - result.puppet.left;
  assert.ok(result.figure.bottom > result.puppet.top + 0.8 * (result.puppet.bottom - result.puppet.top), `${viewport.name} projected feet must sit near the canvas bottom: ${JSON.stringify(result.figure)}`);
  if (viewport.name !== 'phone') {
    assert.ok(result.pageHeight <= viewport.height, `${viewport.name} must remain one screen: ${result.pageHeight}`);
    assert.equal(result.puppet.top, 0, `${viewport.name} puppet must start at the top of the stage`);
    assert.equal(result.puppet.bottom, result.stageHeight, `${viewport.name} puppet must take the full stage height: ${result.puppet.bottom} of ${result.stageHeight}`);
    for (const wing of [result.display, result.ledger]) assert.ok(wing.right - wing.left < puppetWidth, `${viewport.name} wing wider than the puppet: ${JSON.stringify(wing)}`);
    assert.ok(result.fresh.display.right - result.fresh.display.left > result.display.right - result.display.left, `${viewport.name} fresh display must grow: ${JSON.stringify(result.fresh.display)}`);
  } else {
    assert.ok(result.pageHeight > viewport.height, 'phone controls should continue below the first screen');
    assert.deepEqual(result.fresh.display, result.display, 'phone display must not move when fresh');
  }
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
      document.body.dataset.androidTest = JSON.stringify({ viewportHeight: Math.round(visualViewport.height), stageHeight: document.querySelector('#conversation').clientHeight - 940, heights, initial, settled: canvas.clientHeight, loaded: testPuppet.humanoidBone > 0, visible: pixel[3] > 0 });
  }, 60000);
  });
  `, { scale: 3, size: '390,844', budget: 65000, mobile: true });
  const encoded = /data-android-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  const result = JSON.parse(encoded ?? 'null');
  assert.ok(result?.loaded && result.visible, `${JSON.stringify(result)}\n${stderr}`);
  assert.equal(result.settled, result.initial);
  assert.equal(result.stageHeight, result.viewportHeight);
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
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.delegation.created', delegation: { id: 'state_1' } }) }));
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
  const removed = ['ready', 'opening microphone', 'live', 'conversation off', 'session ended', 'listening', 'waiting for the hub', 'waiting…'];
  for (const [state, text] of Object.entries(states)) {
    for (const value of removed) assert.equal(text.toLowerCase().includes(value.toLowerCase()), false, `${viewport.name} ${state} exposes ${value}: ${text}`);
  }
  for (const text of result.statusTextWrites) {
    for (const value of removed) assert.equal(text.toLowerCase().includes(value.toLowerCase()), false, `${viewport.name} header exposed ${value}: ${text}`);
  }
  assert.match(states.waiting, /question: Where is the report\?/);
  assert.doesNotMatch(states.waiting, /answer:/);
  assert.match(states.answered, /question: Where is the report\?[\s\S]*answer: On the display\./);
});

test('a forced page error reaches the fleet catcher line in headless Chromium', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  setTimeout(() => { throw new TypeError('forced page fault'); }, 0);
  setTimeout(() => { document.body.dataset.telemetryErrorTest = JSON.stringify(fleetLines); }, 300);
});
`);
  const encoded = /data-telemetry-error-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), ['fleet-error: voice/page — TypeError: forced page fault'], stderr);
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
  setTimeout(() => { document.body.dataset.shareTest = JSON.stringify({ sent: globalThis.sentShare?.metadata?.text, status: document.querySelector('#status').textContent, notices: sentLiveEvents.filter((event) => event.event_id?.startsWith('share_')).map((event) => event.item?.content?.[0]?.text ?? event.type) }); }, 30);
});
`);
  const encoded = /data-share-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { sent: 'https://example.test/a?q=one', status: 'sent', notices: ['A link arrived.', 'response.create'] }, stderr);
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

test('the delegation log keeps the last of ten appended entries visible', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  ${appendEntries}
  const log = document.querySelector('#log');
  const last = log.lastElementChild;
  const lastBox = last.getBoundingClientRect();
  const logBox = log.getBoundingClientRect();
  document.body.dataset.followTest = String(lastBox.bottom <= logBox.bottom + 1 && lastBox.top >= logBox.top - 1);
});
`);
  const observed = /data-follow-test="([^"]*)"/.exec(stdout)?.[1] ?? 'follow test did not run';
  assert.equal(observed, 'true', `${observed}\n${stderr}`);
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
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'session.delegation.created', delegation: { id: 'scroll_newest' } }) }));
  document.body.dataset.pauseTest = String(log.scrollTop === before);
});
`);
  const observed = /data-pause-test="([^"]*)"/.exec(stdout)?.[1] ?? 'pause test did not run';
  assert.equal(observed, 'true', `${observed}\n${stderr}`);
});

test('completed puppet tool calls execute locally and are acknowledged to Live', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type: 'response.output_item.done', item: { type: 'function_call', call_id: 'call_1', name: 'mood', arguments: '{"name":"amused"}' } }) }));
  setTimeout(() => {
    document.body.dataset.toolTest = JSON.stringify({ calls: testPuppet.calls.filter(([name]) => name === 'mood'), events: sentLiveEvents.filter(({ type }) => type === 'conversation.item.create' || type === 'response.create').map(({ type }) => type) });
  }, 20);
});
`);
  const encoded = /data-tool-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), { calls: [['mood', 'amused']], events: ['conversation.item.create', 'response.create'] }, stderr);
});

test('output transcript drives mood and delegation drives the waiting pose', async () => {
  const { stdout, stderr } = await runPage(`
window.addEventListener('test-ready', () => {
  const event = (value) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(value) }));
  event({ type: 'session.output_transcript.delta', delta: 'Sorry, that was my fault.' });
  event({ type: 'session.input_transcript.delta', delta: 'check it' });
  event({ type: 'session.delegation.created', delegation: { id: 'wait_1' } });
  setTimeout(() => { document.body.dataset.driverTest = JSON.stringify(testPuppet.calls.filter(([name]) => name === 'mood' || name === 'waiting')); }, 20);
});
`);
  const encoded = /data-driver-test="([^"]*)"/.exec(stdout)?.[1]?.replaceAll('&quot;', '"');
  assert.deepEqual(JSON.parse(encoded ?? 'null'), [['waiting', true], ['mood', 'apologetic']], stderr);
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
