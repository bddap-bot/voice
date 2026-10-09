import { spawn } from 'node:child_process';
import { access, mkdtemp, readdir, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { join } from 'node:path';

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
  const chrome = spawn(executable, [...args, `--user-data-dir=${scratch}`, '--remote-debugging-pipe'], { stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'], env: { ...process.env, HOME: scratch, XDG_CONFIG_HOME: scratch, XDG_CACHE_HOME: scratch, TMPDIR: scratch, VK_LOADER_DRIVERS_SELECT: '*swiftshader*' } });
  let stderr = '', failure;
  chrome.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-500); });
  chrome.on('error', error => { failure = error; });
  const exited = new Promise(resolve => chrome.once('close', (code, signal) => {
    resolve(failure ?? new Error(`Chromium exited with ${signal ?? `code ${code}`}: ${stderr}`));
  }));
  let closing;
  const close = () => closing ??= (async () => {
    if (chrome.pid) chrome.kill('SIGKILL');
    await exited;
    await rm(scratch, { recursive: true, force: true });
  })();
  return { scratch, exited, close, devtools: devtools(chrome.stdio[3], chrome.stdio[4], () => stderr), get stderr() { return stderr; } };
}

function devtools(input, output, stderr) {
  let id = 0, buffer = '', ended = false;
  const pending = new Map(), listeners = new Set();
  const closed = new Promise(resolve => output.on('close', resolve));
  const unanswered = method => new Error(`Chromium closed DevTools before answering ${method}: ${stderr()}`);
  closed.then(() => {
    ended = true;
    for (const { reject, method } of pending.values()) reject(unanswered(method));
    pending.clear();
  });
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
      if (ended) return reject(unanswered(method));
      pending.set(++id, { resolve, reject, method });
      input.write(`${JSON.stringify({ id, method, params, sessionId })}\0`);
    }),
    listen: listener => { listeners.add(listener); return () => listeners.delete(listener); },
  };
}

export const livePageArgs = ({ width, height }) => ['--headless=new', '--no-sandbox', '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--hide-scrollbars', '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--autoplay-policy=no-user-gesture-required', `--window-size=${width},${height}`];

export async function openPage(call, url = 'about:blank') {
  const { targetId } = await call('Target.createTarget', { url });
  const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
  const page = (method, params) => call(method, params, sessionId);
  const evaluate = async expression => {
    const { result, exceptionDetails } = await page('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
    return result.value;
  };
  return { targetId, sessionId, call: page, evaluate };
}

const DOCUMENT_HTML = "(document.doctype ? new XMLSerializer().serializeToString(document.doctype) + '\\n' : '') + document.documentElement.outerHTML";

export async function renderDom(chrome, url, { budget }) {
  const { call, listen, closed } = chrome.devtools;
  const loads = new Set();
  let expired = false, wake = () => {}, fail;
  const failure = new Promise((_, reject) => {
    fail = reject;
    closed.then(() => reject(new Error(`Chromium's browser process ended: ${chrome.stderr}`)));
  });
  const stop = listen(({ method, params }) => {
    if (method === 'Inspector.targetCrashed') fail(new Error(`Chromium renderer crashed: ${chrome.stderr}`));
    if (method === 'Page.lifecycleEvent' && params.name === 'load') loads.add(params.loaderId);
    if (method === 'Emulation.virtualTimeBudgetExpired') expired = true;
    wake();
  });
  const until = condition => new Promise(resolve => {
    wake = () => { if (condition()) resolve(); };
    wake();
  });
  const step = work => Promise.race([work, failure]);
  try {
    const { call: page, evaluate } = await openPage((...args) => step(call(...args)));
    await page('Inspector.enable');
    await page('Page.enable');
    await page('Page.setLifecycleEventsEnabled', { enabled: true });
    const { loaderId, errorText } = await page('Page.navigate', { url });
    if (errorText) throw new Error(`Chromium could not load ${url}: ${errorText}`);
    await step(until(() => loads.has(loaderId)));
    await page('Emulation.setVirtualTimePolicy', { policy: 'pauseIfNetworkFetchesPending', budget, maxVirtualTimeTaskStarvationCount: 9999 });
    await step(until(() => expired));
    return await evaluate(DOCUMENT_HTML);
  } finally {
    stop();
  }
}
