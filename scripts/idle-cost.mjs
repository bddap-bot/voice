import { readFile, readdir } from 'node:fs/promises';
import http from 'node:http';
import path from 'node:path';
import { SWIFTSHADER, launchChromium, livePageArgs, openPage } from './chromium.mjs';

const root = path.resolve(new URL('..', import.meta.url).pathname);
const flag = (name, fallback) => { const at = process.argv.indexOf(name); return at >= 0 ? process.argv[at + 1] : fallback; };
const seconds = Number(flag('--seconds', 20));
const settle = Number(flag('--settle', 6));
const variants = flag('--variants', 'both,puppet,spotter,neither').split(',');
const TICK = 100;
const BROWSER = [...livePageArgs({ width: 412, height: 915 }), ...SWIFTSHADER];

const harness = `<!doctype html><meta charset="utf-8"><style>body{margin:0;background:#111}canvas{width:100vw;height:100vh;display:block}</style><canvas id="puppet"></canvas>
<script>globalThis.__idle = { frames: 0, events: [], ready: false }; addEventListener('error', (event) => __idle.events.push({ error: String(event.message) })); addEventListener('unhandledrejection', (event) => __idle.events.push({ error: String(event.reason?.stack ?? event.reason) }));</script>
<script type="module">
import { PuppetRuntime } from './puppet.js';
import { startWakeSpotter } from './wake.js';
const options = new URLSearchParams(location.search);
if (options.has('puppet')) {
  const runtime = new PuppetRuntime(document.querySelector('#puppet'));
  const render = runtime.renderer.render.bind(runtime.renderer);
  runtime.renderer.render = (...args) => { __idle.frames++; return render(...args); };
  const [figure, motion] = await Promise.all([fetch('/golden/figure.vrm').then((r) => r.arrayBuffer()), fetch('/golden/idle.json').then((r) => r.json())]);
  if (!await runtime.load(figure)) throw new Error('figure did not load');
  runtime.asleep(true);
  runtime.pose('sit');
  await runtime.loadClips(['sit', 'sit-idle'].map((action) => ({ action, format: 'motion', data: { ...motion, name: action } })));
}
if (options.has('spotter')) {
  let stream;
  if (options.has('phrase')) {
    const { audio } = await fetch('/golden/wake.json').then((r) => r.json());
    const samples = Float32Array.from(new Int16Array(Uint8Array.from(atob(audio), (c) => c.charCodeAt(0)).buffer), (sample) => sample / 32767);
    const context = new AudioContext({ sampleRate: 16000 });
    const buffer = context.createBuffer(1, samples.length + 16000, 16000);
    buffer.copyToChannel(samples, 0);
    const source = context.createBufferSource();
    source.buffer = buffer;
    source.loop = true;
    const destination = context.createMediaStreamDestination();
    source.connect(destination);
    source.start();
    stream = destination.stream;
  } else stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const model = await fetch('/wake-demo.json').then((r) => r.json());
  await new Promise((resolve) => startWakeSpotter(stream, (event) => { __idle.events.push(event); if (event.ready) resolve(); }, model));
}
__idle.ready = true;
</script>`;

async function serve() {
  const server = http.createServer(async (request, response) => {
    const url = new URL(request.url, 'http://localhost');
    if (url.pathname === '/') return response.writeHead(200, { 'content-type': 'text/html' }).end(harness);
    const file = url.pathname.startsWith('/golden/') ? path.join(root, 'vr', url.pathname)
      : url.pathname === '/wake-demo.json' ? path.join(root, 'scripts/wake-demo.json')
      : path.join(root, 'docs', url.pathname);
    try {
      const body = await readFile(file);
      const type = file.endsWith('.js') || file.endsWith('.mjs') ? 'text/javascript' : file.endsWith('.wasm') ? 'application/wasm' : file.endsWith('.json') ? 'application/json' : 'application/octet-stream';
      response.writeHead(200, { 'content-type': type }).end(body);
    } catch { response.writeHead(404).end(); }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { url: `http://127.0.0.1:${server.address().port}/`, close: () => server.close() };
}

async function readOrNull(file) { try { return await readFile(file, 'utf8'); } catch { return null; } }

async function descendants(scratch) {
  const processes = [];
  for (const name of await readdir('/proc')) {
    if (!/^\d+$/.test(name)) continue;
    const [text, cmdline] = await Promise.all([readOrNull(`/proc/${name}/stat`), readOrNull(`/proc/${name}/cmdline`)]);
    if (!text || !cmdline) continue;
    processes.push({ pid: Number(name), ppid: Number(text.slice(text.lastIndexOf(')') + 2).split(' ')[1]), cmdline: cmdline.split(/[\0 ]/) });
  }
  const browser = processes.find((each) => each.cmdline.includes(`--user-data-dir=${scratch}`) && !each.cmdline.some((arg) => arg.startsWith('--type=')));
  const family = new Set([browser.pid]);
  for (let grew = true; grew;) {
    grew = false;
    for (const each of processes) if (!family.has(each.pid) && family.has(each.ppid)) { family.add(each.pid); grew = true; }
  }
  return processes.filter((each) => family.has(each.pid)).map((each) => {
    const type = each.cmdline.find((arg) => arg.startsWith('--type='))?.slice(7) ?? 'browser';
    const sub = each.cmdline.find((arg) => arg.startsWith('--utility-sub-type='))?.slice(19).split('.')[0];
    return { pid: each.pid, kind: sub ? `${type}:${sub}` : type };
  });
}

async function threadTicks(processes) {
  const ticks = new Map();
  for (const { pid, kind } of processes) {
    let tasks = [];
    try { tasks = await readdir(`/proc/${pid}/task`); } catch { continue; }
    for (const tid of tasks) {
      const [text, comm, status] = await Promise.all([readOrNull(`/proc/${pid}/task/${tid}/stat`), readOrNull(`/proc/${pid}/task/${tid}/comm`), readOrNull(`/proc/${pid}/task/${tid}/status`)]);
      if (!text || !comm || !status) continue;
      const fields = text.slice(text.lastIndexOf(')') + 2).split(' ');
      ticks.set(tid, { key: `${kind} ${Number(tid) === pid ? 'main' : comm.trim().replace(/\d+$/, '#')}`, cpu: Number(fields[11]) + Number(fields[12]), wakes: Number(/voluntary_ctxt_switches:\s*(\d+)/.exec(status)[1]) });
    }
  }
  return ticks;
}

async function until(evaluate, expression, failure) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const [value, error] = await evaluate(`[${expression}, globalThis.__idle?.events.find((event) => event.error)?.error]`).catch(() => []);
    if (error) throw new Error(error);
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(failure);
}

async function measure(server, variant) {
  const params = new URLSearchParams();
  if (variant === 'both' || variant === 'puppet') params.set('puppet', '');
  if (variant === 'both' || variant === 'spotter') params.set('spotter', '');
  const chrome = await launchChromium({ args: BROWSER });
  try {
    const { evaluate, call } = await openPage(chrome.devtools.call);
    await call('Page.navigate', { url: `${server.url}?${params}` });
    await until(evaluate, 'globalThis.__idle?.ready', `${variant}: the harness never became ready`);
    await new Promise((resolve) => setTimeout(resolve, settle * 1000));
    const processes = await descendants(chrome.scratch);
    const before = await threadTicks(processes);
    const framesBefore = await evaluate('__idle.frames');
    const started = performance.now();
    await new Promise((resolve) => setTimeout(resolve, seconds * 1000));
    const after = await threadTicks(processes);
    const elapsed = (performance.now() - started) / 1000;
    const frames = (await evaluate('__idle.frames')) - framesBefore;
    const errors = await evaluate('__idle.events.filter((event) => event.error).map((event) => event.error)');
    if (errors.length) throw new Error(`${variant}: ${errors.join('; ')}`);
    const threads = {};
    for (const [tid, { key, cpu, wakes }] of after) {
      const was = before.get(tid) ?? { cpu: 0, wakes: 0 };
      const total = threads[key] ?? { cpu: 0, wakes: 0 };
      threads[key] = { cpu: total.cpu + (cpu - was.cpu) * 1000 / TICK / elapsed, wakes: total.wakes + (wakes - was.wakes) / elapsed };
    }
    const groups = {
      main: (key) => key === 'renderer main',
      wakeWorker: (key) => key === 'renderer DedicatedWorker',
      audio: (key) => key.startsWith('utility:audio') || /^renderer (Realtime AudioW|AudioInputDevic|AudioOutputDevi)/.test(key),
      gpu: (key) => key.startsWith('gpu-process'),
      total: () => true,
    };
    const sum = (field) => Object.fromEntries(Object.entries(groups).map(([name, test]) => [name, Number(Object.entries(threads).filter(([key]) => test(key)).reduce((total, [, value]) => total + value[field], 0).toFixed(1))]));
    const cpuMsPerSecond = sum('cpu');
    return { variant, seconds: Number(elapsed.toFixed(1)), fps: Number((frames / elapsed).toFixed(1)), gpuMsPerFrame: frames ? Number((cpuMsPerSecond.gpu / (frames / elapsed)).toFixed(2)) : null, cpuMsPerSecond, wakesPerSecond: sum('wakes') };
  } finally { await chrome.close(); }
}

async function phrase(server) {
  const chrome = await launchChromium({ args: BROWSER });
  try {
    const { evaluate, call } = await openPage(chrome.devtools.call);
    await call('Page.navigate', { url: `${server.url}?puppet&spotter&phrase` });
    return await until(evaluate, 'globalThis.__idle?.events.find((event) => event.wake)', 'the wake phrase did not fire');
  } finally { await chrome.close(); }
}

const server = await serve();
try {
  if (process.argv.includes('--phrase')) console.log(JSON.stringify({ phrase: await phrase(server) }));
  else for (const variant of variants) console.log(JSON.stringify(await measure(server, variant)));
} finally { server.close(); }
