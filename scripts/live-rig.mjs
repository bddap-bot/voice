import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { RATE } from '../docs/wake.js';
import { launchChromium, livePageArgs, openPage } from './chromium.mjs';
import { developmentConfig, serveDevelopment } from './dev.mjs';

const root = path.resolve(new URL('..', import.meta.url).pathname);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

export async function rigSpeech(outDir, speechFile, spoken) {
  await mkdir(outDir, { recursive: true });
  let speech;
  if (speechFile) speech = JSON.parse(await readFile(speechFile, 'utf8'));
  else {
    const { heldOutSpeech } = await import('./wake.mjs');
    const audio = await heldOutSpeech(Object.values(spoken));
    speech = Object.fromEntries(Object.entries(spoken).map(([key, text], index) => [key, { text, audio: audio[index] }]));
    await writeFile(path.join(outDir, 'speech.json'), JSON.stringify(speech));
  }
  for (const [key, text] of Object.entries(spoken)) if (speech[key]?.text !== text || !speech[key].audio) throw new Error('missing speech: ' + key);
  return speech;
}

export async function runRig({ outDir, speech, page, extra = {} }, scenario) {
  const t0 = Date.now();
  const now = () => Date.now() - t0;
  const say = (...parts) => console.log(`${(now() / 1000).toFixed(1).padStart(7)} ${parts.join(' ')}`);
  const relayModule = /import wbgInit,.*?from '([^']+)'/.exec(await readFile(path.join(root, 'docs/main.js'), 'utf8'))?.[1];
  if (!relayModule) throw new Error('page relay module was not found');
  const relayFixture = await readFile(path.join(root, 'test/fixtures/wake-reply/botq_dash_wasm.js'));
  const commit = execFileSync('git', ['describe', '--always', '--dirty', '--exclude=*', '--abbrev=40'], { cwd: root }).toString().trim();
  const config = await developmentConfig();
  const sourceHashes = { 'test/fixtures/wake-reply/botq_dash_wasm.js': sha256(relayFixture) };
  const capture = { rt: [], frames: [], heard: [], marks: [] };
  const consoleErrors = [];
  let server, pageUrl, chrome;
  const record = () => writeFile(path.join(outDir, 'capture.json'), JSON.stringify({ ...extra, commit, page: pageUrl, sourceHashes, capture, consoleErrors }, null, 2));
  let drain = async () => {};

  async function run() {
    const relayUrl = new URL(relayModule, pageUrl).href;
    const pageFiles = execFileSync('git', ['ls-files', 'docs/*.html', 'docs/*.js', 'docs/relay/*'], { cwd: root }).toString().split('\n').filter((file) => /^docs\/(relay\/)?[^/]+$/.test(file) && !['docs/config.js', 'docs/sw.js'].includes(file));
    for (const file of pageFiles) {
      const response = await fetch(new URL(file.slice(5), pageUrl), { cache: 'no-store' });
      sourceHashes[file] = sha256(await readFile(path.join(root, file)));
      if (!response.ok || sha256(Buffer.from(await response.arrayBuffer())) !== sourceHashes[file]) throw new Error(`${pageUrl} does not serve this checkout's ${file}`);
    }
    say('serving', commit, pageUrl);
    const { call, evaluate } = await openPage(chrome.devtools.call);
    chrome.devtools.listen((m) => {
      if (m.method === 'Fetch.requestPaused') {
        const { requestId, request } = m.params;
        const relay = request.url === relayUrl;
        chrome.devtools.call(relay ? 'Fetch.fulfillRequest' : 'Fetch.continueRequest', relay ? { requestId, responseCode: 200, responseHeaders: [{ name: 'Content-Type', value: 'text/javascript' }], body: relayFixture.toString('base64') } : { requestId }, m.sessionId).catch(() => {});
      }
      else if (m.method === 'Runtime.exceptionThrown') consoleErrors.push({ at: now(), text: m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text });
      else if (m.method === 'Runtime.consoleAPICalled' && ['error', 'warning'].includes(m.params.type)) consoleErrors.push({ at: now(), text: m.params.args.map((v) => v.value ?? v.description).join(' ') });
    });
    await call('Page.enable'); await call('Runtime.enable');
    await call('Fetch.enable', { patterns: [{ urlPattern: `${relayUrl}*` }] });
    await call('Page.addScriptToEvaluateOnNewDocument', { source: `
  localStorage.setItem('voice.token', ${JSON.stringify(config.token)});
  globalThis.__wakeReplyRelayModule = ${JSON.stringify(`${relayUrl}?relay`)};
  globalThis.__rt = [];
  globalThis.__frames ??= [];
  globalThis.__heard = [];
  globalThis.__speech = ${JSON.stringify(Object.fromEntries(Object.entries(speech).map(([k, v]) => [k, v.audio])))};
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
    source.start();
    await new Promise((resolve) => { source.onended = resolve; });
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
      for (const it of got.rt) { let event; try { event = JSON.parse(it.data); } catch { event = { raw: it.data }; } capture.rt.push({ at: it.at - t0, ch: it.ch, dir: it.dir, event }); }
      capture.frames.push(...got.frames.map((frame) => ({ ...frame, at: frame.at - t0 })));
      capture.heard.push(...got.heard.map((message) => ({ ...message, at: message.at - t0 })));
    };
    const mark = (kind, data = {}) => { const at = now(); capture.marks.push({ at, kind, ...data }); say(kind, JSON.stringify(data).slice(0, 200)); return at; };
    const readyCount = () => capture.heard.filter((m) => m.ready).length;
    const started = (ch) => capture.rt.find((c) => c.ch === ch && c.dir === 'in' && c.event.type === 'session.started');
    const offerErrors = () => capture.frames.filter((f) => f.verb === 'offer-error');
    const lastActivity = (ch) => capture.rt.filter((c) => c.ch === ch && c.dir === 'in' && ['session.output_transcript.delta', 'session.delegation.created', 'session.input_transcript.delta'].includes(c.event.type)).at(-1)?.at ?? 0;
    const pressed = async () => (await state()).pressed === 'true';
    const waitPressed = async (want, ms) => { const dl = Date.now() + ms; while (Date.now() < dl) { await drain(); if ((await pressed()) === want) return true; await sleep(150); } return false; };

    async function load(url) {
      await call('Page.navigate', { url });
      await until("document.querySelector('#puppet')?.getAttribute('aria-disabled')==='false'", 240000, 'page ready');
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
        await evaluate("__say('wake')");
        mark('said', { label, key: 'wake', attempt });
        const dl = Date.now() + 6000;
        while (wakes() === before && Date.now() < dl) { await drain(); await sleep(150); }
        if (wakes() === before) { mark('no-wake', { label, attempt }); await sleep(2000); continue; }
        const opened = await waitPressed(true, 60000);
        await drain();
        if (offerErrors().length > errors) throw new Error(`offer refused: ${offerErrors().at(-1).text}`);
        if (opened && started(ch)) { mark('opened', { label, ch }); return; }
        throw new Error(`the wake fired but ${label} did not start`);
      }
      throw new Error(`wake phrase did not fire for ${label}`);
    }
    async function tap(label, ch, open) {
      await evaluate("document.querySelector('#puppet').click()");
      if (!await waitPressed(open, 60000)) throw new Error(`tapping did not ${open ? 'open' : 'close'} ${label}`);
      await drain();
      if (open && !started(ch)) throw new Error(`${label} did not start`);
      mark(open ? 'opened' : 'closed', { label, ch });
      if (open) return;
      await until("document.querySelector('#puppet')?.getAttribute('aria-label') === 'start conversation'", 30000, 'tappable');
      await sleep(2000);
      await drain();
      await record();
    }
    async function settle(ch, since, quietMs, maxMs) {
      const dl = Date.now() + maxMs;
      while (Date.now() < dl) {
        await drain();
        if (!(await pressed())) throw new Error(`session ${ch} ended itself`);
        const spoke = capture.rt.some((c) => c.ch === ch && c.dir === 'in' && c.at >= since && c.event.type === 'session.output_transcript.delta');
        if (spoke && now() - lastActivity(ch) > quietMs) return;
        await sleep(200);
      }
    }
    async function quietRelay() {
      const since = now();
      await until('Date.now() - (globalThis.__relayAt ?? 0) > 5000', 600000, 'quiet relay');
      mark('relay-quiet', { waited: now() - since });
    }
    async function ask(ch, key, maxMs) {
      const since = mark(`${key}-start`, { ch });
      await evaluate(`__say(${JSON.stringify(key)})`);
      await settle(ch, since, 3000, maxMs);
      return since;
    }
    async function askToSleep(ch) {
      mark('sleep-start', { ch });
      await evaluate("__say('sleep')");
      if (!await waitPressed(false, 60000)) throw new Error(`session ${ch} did not sign off when asked to sleep`);
      await until("document.querySelector('#puppet')?.getAttribute('aria-label') === 'start conversation'", 30000, 'wakeable');
      await sleep(2000);
      await drain();
      await record();
    }
    await load(pageUrl);
    await scenario({ capture, now, say, sleep, mark, evaluate, drain, settle, quietRelay, wake, tap, ask, askToSleep });
  }

  try {
    server = page ? null : await serveDevelopment({ config, port: 0 });
    pageUrl = page ?? server.url;
    chrome = await launchChromium({ args: livePageArgs({ width: 1280, height: 800 }) });
    await Promise.race([chrome.exited.then((error) => { throw error; }), run()]);
  } finally {
    await drain().catch(() => {});
    await record();
    await chrome?.close();
    await server?.close();
  }
  return { commit, page: pageUrl, sourceHashes, capture };
}
