import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, mkdir, mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { createServer } from 'node:http';
import { join } from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';

const execute = promisify(execFile);

const mockWasm = `
const enc = new TextEncoder();
const dec = new TextDecoder();
const queued = [enc.encode(JSON.stringify({ ok: true }))];
const waiting = [];
function deliver(value) {
  const resolve = waiting.shift();
  if (resolve) resolve(value);
  else queued.push(value);
}
export default async function initWasm() {}
export async function init() {}
export async function connect() {}
export async function send_only(bytes) {
  const frame = dec.decode(bytes);
  if (frame === 'puppets') deliver(enc.encode('puppets\\n' + JSON.stringify({ active: '42', avatars: [{ id: '42', creditLine: '', licenseFlags: { creditRequired: false } }] })));
  else if (frame.startsWith('puppet\\n')) {
    deliver(enc.encode('puppet-start\\n{"id":"42","size":3}'));
    const prefix = enc.encode('puppet-chunk\\n42\\n');
    const chunk = new Uint8Array(prefix.length + 3);
    chunk.set(prefix);
    chunk.set([1, 2, 3], prefix.length);
    deliver(chunk);
    deliver(enc.encode('puppet-end\\n42'));
  } else if (frame.startsWith('puppet-select\\n')) deliver(enc.encode('puppet-selected\\n{"id":"42"}'));
  else if (frame.startsWith('offer\\n')) {
    const offer = JSON.parse(frame.slice(6));
    deliver(enc.encode('answer\\n' + JSON.stringify({ offer_id: offer.id, sdp: 'answer', cap_seconds: 60 })));
  }
}
export async function recv() {
  if (queued.length) return queued.shift();
  return new Promise((resolve) => waiting.push(resolve));
}
`;

const fakePuppet = `
export class PuppetRuntime {
  constructor() { this.calls = []; globalThis.testPuppet = this; }
  async load(bytes, valid, beforeCommit) { await beforeCommit(); return valid(); }
  pose(...args) { this.calls.push(['pose', ...args]); }
  gesture(...args) { this.calls.push(['gesture', ...args]); }
  look(...args) { this.calls.push(['look', ...args]); }
  mood(...args) { this.calls.push(['mood', ...args]); }
  waiting(...args) { this.calls.push(['waiting', ...args]); }
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
const puppetCache = new Map();
Object.defineProperty(globalThis, 'caches', { value: { open: async () => ({
  match: async (request) => puppetCache.get(request.url)?.clone(),
  put: async (request, response) => puppetCache.set(request.url, response.clone()),
}) } });
const stream = { getTracks: () => [{ stop() {} }] };
Object.defineProperty(navigator, 'mediaDevices', { value: { getUserMedia: async () => stream } });
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
  document.querySelector('#token').value = btoa(JSON.stringify({ endpoint_id: 'test', secret: 'test' }));
  document.querySelector('#connect').click();
  const ready = poll(() => {
    if (document.querySelector('#status').textContent !== 'ready') return;
    clearInterval(ready);
    document.querySelector('#toggle').click();
    const started = poll(() => {
      const status = document.querySelector('#status').textContent;
      if (status !== 'live' && !status.startsWith('conversation could not start:')) return;
      clearInterval(started);
      document.body.dataset.startTest = status;
      window.dispatchEvent(new Event('test-ready'));
    }, 10);
  }, 10);
});
`;

const appendEntries = `
const append = (type, delta) => testChannel.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ type, delta }) }));
for (let index = 0; index < 5; index++) {
  append('session.input_transcript.delta', 'heard ' + index);
  append('session.output_transcript.delta', 'spoken ' + index);
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

async function runPage(testSetup = '') {
  const index = (await readFile(new URL('../docs/index.html', import.meta.url), 'utf8'))
    .replace('https://bddap-bot.github.io/botq/botq_dash_wasm.js', '/botq_dash_wasm.js')
    .replace('./puppet.js', '/fake-puppet.js')
    .replace('</head>', `<script>${browserSetup}${testSetup}</script></head>`);
  const live = await readFile(new URL('../docs/live.js', import.meta.url));
  const puppetClient = await readFile(new URL('../docs/puppet-client.js', import.meta.url));
  const puppetTools = await readFile(new URL('../docs/puppet-tools.js', import.meta.url));
  const scratch = await mkdtemp(join(process.cwd(), '.chromium-'));
  const profile = join(scratch, 'profile');
  const temporary = join(scratch, 'tmp');
  await Promise.all([mkdir(profile), mkdir(temporary)]);
  const server = createServer((request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    const body = path === '/botq_dash_wasm.js' ? mockWasm : path === '/fake-puppet.js' ? fakePuppet : path === '/puppet-client.js' ? puppetClient : path === '/puppet-tools.js' ? puppetTools : path === '/live.js' ? live : index;
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
      '--window-size=390,844',
      `--user-data-dir=${profile}`,
      '--virtual-time-budget=3000',
      '--dump-dom',
      `http://127.0.0.1:${server.address().port}/`,
    ], { timeout: 15000, killSignal: 'SIGKILL', env: { ...process.env, TMPDIR: temporary } });
    return { stdout, stderr };
  } finally {
    if (server.listening) await new Promise((resolve) => server.close(resolve));
    await rm(scratch, { recursive: true, force: true });
  }
}

test('the page toggle completes its start path in headless Chromium', async () => {
  const { stdout, stderr } = await runPage();
  const observed = /data-start-test="([^"]*)"/.exec(stdout)?.[1] ?? 'start path did not settle';
  assert.equal(observed, 'live', `${observed}\n${stderr}`);
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
