import { execFile, spawn } from 'node:child_process';
import { access, mkdtemp, readdir, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { setTimeout } from 'node:timers/promises';

const execute = promisify(execFile);

export async function chromiumExecutable() {
  if (process.env.CHROMIUM_BIN) return process.env.CHROMIUM_BIN;
  const candidates = ['chromium', 'chromium-browser', 'google-chrome', 'google-chrome-stable'];
  try { candidates.push(...(await readdir('/nix/store')).filter(name => name.includes('-chromium-')).map(name => `/nix/store/${name}/bin/chromium`)); } catch {}
  for (const candidate of candidates) for (const location of candidate.includes('/') ? [candidate] : (process.env.PATH ?? '').split(':').map(directory => join(directory, candidate))) {
    try { await access(location, constants.X_OK); return location; } catch {}
  }
  throw new Error('headless Chromium is required; set CHROMIUM_BIN');
}

export async function launchChromium({ executable, args = [], prefix = '.chromium-' } = {}) {
  executable ??= await chromiumExecutable();
  const scratch = await mkdtemp(join(process.cwd(), prefix));
  const chrome = spawn(executable, [...args, `--user-data-dir=${scratch}`, '--remote-debugging-pipe'], { detached: true, stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'], env: { ...process.env, TMPDIR: scratch } });
  let stderr = '', failure;
  chrome.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-500); });
  chrome.on('error', error => { failure = error; });
  const exited = new Promise(resolve => chrome.once('close', (code, signal) => {
    resolve(failure ?? new Error(`Chromium exited with ${signal ?? `code ${code}`}: ${stderr}`));
  }));
  let closing;
  const close = () => closing ??= (async () => {
    if (chrome.pid) {
      try { process.kill(-chrome.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
    }
    await exited;
    if (chrome.pid) {
      const deadline = Date.now() + 10000;
      while (true) {
        const { stdout } = await execute('ps', ['-eo', 'pgid=,stat=']);
        const running = stdout.trim().split('\n').some(line => {
          const [group, state] = line.trim().split(/\s+/);
          return Number(group) === chrome.pid && !state.startsWith('Z');
        });
        if (!running) break;
        if (Date.now() >= deadline) throw new Error('Chromium process group did not exit; profile retained');
        await setTimeout(20);
      }
    }
    await rm(scratch, { recursive: true, force: true });
  })();
  return { scratch, exited, close, devtools: devtools(chrome.stdio[3], chrome.stdio[4]), get stderr() { return stderr; } };
}

function devtools(input, output) {
  let id = 0, buffer = '';
  const pending = new Map(), listeners = new Set();
  const closed = new Promise(resolve => output.on('close', resolve));
  output.setEncoding('utf8').on('data', chunk => {
    const messages = (buffer + chunk).split('\0');
    buffer = messages.pop();
    for (const message of messages.map(text => JSON.parse(text))) {
      const reply = pending.get(message.id);
      pending.delete(message.id);
      if (!reply) for (const listener of listeners) listener(message);
      else if (message.error) reply.reject(new Error(`${message.error.message} (${message.error.code})`));
      else reply.resolve(message.result);
    }
  });
  input.on('error', () => {});
  return {
    closed,
    call: (method, params = {}, sessionId) => new Promise((resolve, reject) => {
      pending.set(++id, { resolve, reject });
      closed.then(() => reject(new Error(`Chromium closed DevTools before answering ${method}`)));
      input.write(`${JSON.stringify({ id, method, params, sessionId })}\0`);
    }),
    listen: listener => { listeners.add(listener); return () => listeners.delete(listener); },
  };
}

const DOCUMENT_HTML = "(document.doctype ? new XMLSerializer().serializeToString(document.doctype) + '\\n' : '') + document.documentElement.outerHTML";
const VIRTUAL_STEP_MS = 1000;

export async function renderDom(chrome, url, { budget, stepLimit = 60000 }) {
  const { call, listen, closed } = chrome.devtools;
  const loads = new Set();
  let expired = 0, wake = () => {}, crashed;
  const failure = new Promise((_, reject) => {
    crashed = reject;
    closed.then(() => reject(new Error(`Chromium's browser process ended: ${chrome.stderr}`)));
  });
  const stop = listen(({ method, params }) => {
    if (method === 'Inspector.targetCrashed') crashed(new Error(`Chromium renderer crashed: ${chrome.stderr}`));
    if (method === 'Page.lifecycleEvent' && params.name === 'load') loads.add(params.loaderId);
    if (method === 'Emulation.virtualTimeBudgetExpired') expired++;
    wake();
  });
  const until = condition => new Promise(resolve => {
    wake = () => { if (condition()) resolve(); };
    wake();
  });
  const step = (action, work) => {
    const settled = new AbortController();
    const limit = setTimeout(stepLimit, null, { signal: settled.signal }).then(() => { throw new Error(`Chromium did not ${action} within ${stepLimit / 1000} s: ${chrome.stderr}`); });
    return Promise.race([work, failure, limit]).finally(() => settled.abort());
  };
  try {
    const { targetId } = await step('start', call('Target.createTarget', { url: 'about:blank' }));
    const { sessionId } = await step('attach to its page', call('Target.attachToTarget', { targetId, flatten: true }));
    const page = (method, params) => step(`answer ${method}`, call(method, params, sessionId));
    await page('Inspector.enable');
    await page('Page.enable');
    await page('Page.setLifecycleEventsEnabled', { enabled: true });
    const { loaderId, errorText } = await page('Page.navigate', { url });
    if (errorText) throw new Error(`Chromium could not load ${url}: ${errorText}`);
    await step(`load ${url}`, until(() => loads.has(loaderId)));
    for (let elapsed = 0; elapsed < budget; elapsed += VIRTUAL_STEP_MS) {
      const before = expired;
      await page('Emulation.setVirtualTimePolicy', { policy: 'pauseIfNetworkFetchesPending', budget: Math.min(VIRTUAL_STEP_MS, budget - elapsed), maxVirtualTimeTaskStarvationCount: 9999 });
      await step(`advance virtual time past ${elapsed} ms`, until(() => expired > before));
    }
    return (await page('Runtime.evaluate', { expression: DOCUMENT_HTML, returnByValue: true })).result.value;
  } finally {
    stop();
  }
}
