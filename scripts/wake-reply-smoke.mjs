import { spawn, execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import { assessWakeReplies, spokenReply } from '../test/wake-reply-measurements.js';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { WAKE_PHRASE } from '../docs/identity.js';
import { RATE } from '../docs/wake.js';
import { serveDevelopment } from './dev.mjs';

const root = path.resolve(new URL('..', import.meta.url).pathname);
const [,, outDir, speechFile] = process.argv;
if (!outDir) throw new Error('usage: node scripts/wake-reply-smoke.mjs OUTPUT [SPEECH_JSON]');
await mkdir(outDir, { recursive: true });
const speech = speechFile ? JSON.parse(await readFile(speechFile, 'utf8')) : {};
if (!speechFile) {
  const { HELD_OUT, synthesize } = await import('./wake.mjs');
  for (const [key, text] of Object.entries({ wake: WAKE_PHRASE, request: 'Ask the hub for the current test beacon status.' })) {
    const [clip] = await synthesize(HELD_OUT[0], 1, [text]);
    speech[key] = { text, audio: Buffer.from(clip.buffer, clip.byteOffset, clip.byteLength).toString('base64') };
  }
  await writeFile(path.join(outDir, 'speech.json'), JSON.stringify(speech));
}
for (const key of ['wake', 'request']) if (!speech[key]?.audio) throw new Error('missing speech: ' + key);
const CHROMIUM = process.env.CHROMIUM_BIN ?? 'chromium';
const RIG = path.resolve(outDir);
const replies = ['amber', 'violet', 'silver', 'crimson', 'golden', 'indigo', 'turquoise', 'copper', 'magenta', 'ivory', 'emerald'].map((color) => 'The test beacon is ' + color + '.');
await mkdir(outDir, { recursive: true });

const t0 = Date.now();
const say = (...parts) => console.log(`${((Date.now() - t0) / 1000).toFixed(1).padStart(7)} ${parts.join(' ')}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const relayModule = /import wbgInit,.*?from '([^']+)'/.exec(await readFile(path.join(root, 'docs/index.html'), 'utf8'))?.[1];
if (!relayModule) throw new Error('page relay module was not found');
const server = await serveDevelopment({ port: 0, wasmRoot: path.join(root, 'test/fixtures/wake-reply') });
const commit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root }).toString().trim();
say('serving', commit, server.url);
const debugPort = await new Promise((r) => { const l = net.createServer().listen(0, '127.0.0.1', () => { const p = l.address().port; l.close(() => r(p)); }); });
const profile = path.join(RIG, `profile-${process.pid}`);
const chrome = spawn(CHROMIUM, ['--headless=new', '--no-sandbox', '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--hide-scrollbars', '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--autoplay-policy=no-user-gesture-required', '--window-size=1280,800', `--user-data-dir=${profile}`, `--remote-debugging-port=${debugPort}`, '--remote-debugging-address=127.0.0.1', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
let chromeErr = ''; chrome.on('error', (error) => { chromeErr = String(error); }); chrome.stderr.on('data', (c) => { chromeErr = (chromeErr + c).slice(-4000); });
await writeFile(path.join(outDir, `chrome-${process.pid}.pid`), String(chrome.pid));

const sourceHashes = Object.fromEntries(await Promise.all(['docs/index.html', 'docs/live.js', 'docs/live-playback.js', 'test/fixtures/wake-reply/botq_dash_wasm.js'].map(async (file) => [file, createHash('sha256').update(await readFile(path.join(root, file))).digest('hex')])));
const capture = { rt: [], frames: [], heard: [], marks: [] };
const consoleErrors = [];
let ws;
let drain = async () => {};
try {
let page;
for (let i = 0; !page && i < 600; i++) { try { page = (await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json()).find((t) => t.type === 'page'); } catch {} if (!page) await sleep(100); }
if (!page) throw new Error('no page target: ' + chromeErr);
const browser = await (await fetch(`http://127.0.0.1:${debugPort}/json/version`)).json();
ws = new WebSocket(browser.webSocketDebuggerUrl);
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
let id = 0; const pending = new Map(); let session;
ws.onclose = () => { for (const { reject } of pending.values()) reject(new Error('browser disconnected')); pending.clear(); };
ws.onmessage = ({ data }) => {
  const m = JSON.parse(data);
  if (pending.has(m.id)) { const { resolve, reject } = pending.get(m.id); pending.delete(m.id); m.error ? reject(new Error(m.error.message)) : resolve(m); }
  else if (m.method === 'Runtime.exceptionThrown') consoleErrors.push({ at: Date.now(), text: m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text });
  else if (m.method === 'Runtime.consoleAPICalled' && ['error', 'warning'].includes(m.params.type)) consoleErrors.push({ at: Date.now(), text: m.params.args.map((v) => v.value ?? v.description).join(' ') });
};
const call = (method, params = {}, sessionId = session) => new Promise((resolve, reject) => { const n = ++id; pending.set(n, { resolve, reject }); ws.send(JSON.stringify({ id: n, method, params, sessionId })); });
const evaluate = async (expression) => { const m = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (m.result.exceptionDetails) throw new Error(m.result.exceptionDetails.exception?.description ?? m.result.exceptionDetails.text); return m.result.result.value; };
session = (await call('Target.attachToTarget', { targetId: page.id, flatten: true })).result.sessionId;
await call('Page.enable'); await call('Runtime.enable');
await call('Page.addScriptToEvaluateOnNewDocument', { source: `
  globalThis.__wakeReplyRelayModule = ${JSON.stringify(relayModule)};
  globalThis.__rt = [];
  globalThis.__frames ??= [];
  globalThis.__heard = [];
  globalThis.__speech = ${JSON.stringify(Object.fromEntries(Object.entries(speech).filter(([, v]) => v.audio).map(([k, v]) => [k, v.audio])))};
  const microphone = new AudioContext();
  const destination = microphone.createMediaStreamDestination();
  const floor = microphone.createConstantSource();
  floor.offset.value = 0;
  floor.connect(destination);
  floor.start();
  navigator.mediaDevices.getUserMedia = async () => destination.stream;
  globalThis.__say = async (key) => {
    const samples = new Float32Array(Uint8Array.from(atob(__speech[key]), (c) => c.charCodeAt(0)).buffer);
    const source = microphone.createBufferSource();
    source.buffer = microphone.createBuffer(1, samples.length, ${RATE});
    source.buffer.copyToChannel(samples, 0);
    source.connect(destination);
    const started = Date.now();
    source.start();
    await new Promise((resolve) => { source.onended = resolve; });
    return { key, started, ended: Date.now() };
  };
  const SpotterWorker = Worker;
  globalThis.Worker = class extends SpotterWorker {
    constructor(...args) { super(...args); this.addEventListener('message', ({ data }) => __heard.push({ ...data, at: Date.now() })); }
  };
  let channels = 0; globalThis.__peers = [];
  const Peer = RTCPeerConnection;
  globalThis.RTCPeerConnection = class extends Peer {
    createDataChannel(...a) {
      const ch = super.createDataChannel(...a); __peers.push(this);
      const index = channels++;
      ch.addEventListener('open', () => __rt.push({ at: Date.now(), ch: index, dir: 'state', data: 'open' }));
      ch.addEventListener('close', () => __rt.push({ at: Date.now(), ch: index, dir: 'state', data: 'close' }));
      ch.addEventListener('message', ({ data }) => { __rt.push({ at: Date.now(), ch: index, dir: 'in', data: String(data).slice(0, 30000) }); });
      const send = ch.send.bind(ch);
      ch.send = (d) => { __rt.push({ at: Date.now(), ch: index, dir: 'out', data: String(d).slice(0, 30000) }); return send(d); };
      return ch;
    }
  };
` });

const state = () => evaluate("JSON.stringify({ pressed: document.querySelector('#puppet')?.getAttribute('aria-pressed'), status: document.querySelector('#status')?.textContent })").then(JSON.parse);
const until = async (expr, ms, what) => { const dl = Date.now() + ms; while (Date.now() < dl) { try { if (await evaluate(expr)) return true; } catch {} await sleep(100); } throw new Error(`timeout: ${what}`); };
drain = async () => {
  const got = JSON.parse(await evaluate('JSON.stringify({ rt: __rt.splice(0), frames: __frames.splice(0), heard: __heard.splice(0) })'));
  for (const it of got.rt) { let event; try { event = JSON.parse(it.data); } catch { event = { raw: it.data }; } capture.rt.push({ at: it.at, ch: it.ch, dir: it.dir, event }); }
  capture.frames.push(...got.frames);
  capture.heard.push(...got.heard);
};
const mark = (kind, data = {}) => { capture.marks.push({ at: Date.now(), kind, ...data }); say(kind, JSON.stringify(data).slice(0, 200)); };
const readyCount = () => capture.heard.filter((m) => m.ready).length;
const started = (ch) => capture.rt.find((c) => c.ch === ch && c.dir === 'in' && c.event.type === 'session.started');
const offerErrors = () => capture.frames.filter((f) => f.verb === 'offer-error');
const lastActivity = (ch) => capture.rt.filter((c) => c.ch === ch && c.dir === 'in' && ['session.output_transcript.delta', 'session.delegation.created', 'session.input_transcript.delta'].includes(c.event.type)).at(-1)?.at ?? 0;
const pressed = async () => (await state()).pressed === 'true';
const waitPressed = async (want, ms) => { const dl = Date.now() + ms; while (Date.now() < dl) { await drain(); if ((await pressed()) === want) return Date.now(); await sleep(150); } return null; };

async function load(url) {
  await evaluate('globalThis.__stale = true').catch(() => {});
  await call('Page.navigate', { url });
  await until("!globalThis.__stale && document.querySelector('#puppet')?.getAttribute('aria-disabled')==='false'", 240000, 'page ready');
  mark('page-ready');
  const dl = Date.now() + 300000;
  while (readyCount() < 1) { await drain(); const failure = capture.heard.find((m) => m.error); if (failure) throw new Error(`spotter: ${failure.error}`); if (Date.now() > dl) throw new Error('spotter never ready'); await sleep(200); }
  mark('spotter-ready', { n: readyCount() });
}
const wakes = () => capture.heard.filter((m) => m.wake).length;
async function wake(label, ch) {
  for (let attempt = 1; attempt <= 3; attempt++) {
    const errors = offerErrors().length;
    const before = wakes();
    const said = await evaluate("__say('wake')");
    mark('said', { label, key: 'wake', attempt, started: said.started, ended: said.ended });
    const dl = Date.now() + 6000;
    while (wakes() === before && Date.now() < dl) { await drain(); await sleep(150); }
    if (wakes() === before) { mark('no-wake', { label, attempt }); await sleep(2000); continue; }
    const opened = await waitPressed(true, 60000);
    await drain();
    if (offerErrors().length > errors) throw new Error(`offer refused: ${offerErrors().at(-1).text}`);
    if (opened && started(ch)) { mark('opened', { label, ch, startedAt: started(ch).at }); return; }
    throw new Error(`the wake fired but ${label} did not start`);
  }
  throw new Error(`wake phrase did not fire for ${label}`);
}
async function settle(ch, quietMs, maxMs) {
  const start = Date.now();
  const dl = start + maxMs;
  while (Date.now() < dl) {
    await drain();
    if (!(await pressed())) return 'closed';
    const spoke = capture.rt.some((c) => c.ch === ch && c.dir === 'in' && c.event.type === 'session.output_transcript.delta');
    if (spoke && Date.now() - lastActivity(ch) > quietMs) return 'quiet';
    await sleep(200);
  }
  return 'timeout';
}
  await load(server.url);
  for (let ch = 0; ch < replies.length; ch++) {
    await evaluate('globalThis.__lastHubRequest = null');
    await wake(`session-${ch}`, ch);
    const greeting = await settle(ch, 3000, 30000);
    if (greeting !== 'quiet') throw new Error(`greeting: ${greeting}`);
    mark('request-start', { ch, reply: replies[ch] });
    await evaluate("__say('request')");
    await settle(ch, 3000, 15000);
    await evaluate(`__deliverHubReply(${JSON.stringify(replies[ch])}, 'reply-${ch}')`);
    await drain();
    const delivered = capture.frames.findLast((frame) => frame.verb === 'hub');
    await sleep(12000);
    await settle(ch, 3000, 30000);
    await drain();
    const audio = await evaluate(`__peers[${ch}].getStats().then((stats) => [...stats.values()].filter((s) => s.type === 'inbound-rtp' && s.kind === 'audio').map(({ bytesReceived, totalSamplesReceived }) => ({ bytesReceived, totalSamplesReceived })))`);
    mark('reply-end', { ch, audio });
    const transcript = spokenReply(capture, ch, JSON.parse(delivered.text.slice(4)).stamp, Date.now());
    say('REPLY', ch, JSON.stringify({ expected: replies[ch], transcript }));
    assert.equal(transcript.trim(), replies[ch], `session ${ch}: fresh reply must be spoken verbatim`);
    mark('sleep-start', { ch });
    await evaluate("document.querySelector('#puppet').click()");
    if (!await waitPressed(false, 20000)) throw new Error('session did not sleep through the page control');
    await until("document.querySelector('#puppet')?.getAttribute('aria-label') === 'start conversation'", 30000, 'wakeable');
    await sleep(2000);
    await drain();
    await writeFile(path.join(outDir, 'capture.json'), JSON.stringify({ commit, sourceHashes, replies, capture, consoleErrors }, null, 2));
  }
} finally {
  await drain().catch(() => {});
  await writeFile(path.join(outDir, 'capture.json'), JSON.stringify({ commit, sourceHashes, replies, capture, consoleErrors }, null, 2));
  ws?.close();
  if (chrome.pid && chrome.exitCode === null && chrome.signalCode === null) {
    const exited = new Promise((resolve) => chrome.once('exit', resolve));
    chrome.kill('SIGKILL');
    await exited;
  }
  await server.close();
  await rm(profile, { recursive: true, force: true });
}
const rows = assessWakeReplies({ replies, capture });
await writeFile(path.join(outDir, 'verdict.json'), JSON.stringify(rows, null, 2));
console.log(`PASS: ${rows.length - 1} carried-context wakes spoke their exact fresh replies`);
