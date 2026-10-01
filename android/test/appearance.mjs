import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import path from 'node:path';

const adb = `${process.env.ANDROID_HOME}/platform-tools/adb`;
const serial = process.env.ANDROID_SERIAL || 'emulator-5646';
const out = process.env.VOICE_ANDROID_EVIDENCE_DIR || 'android/build/appearance';
const run = (...args) => execFileSync(adb, ['-s', serial, ...args], { encoding: 'utf8', timeout: 60000 });
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
await mkdir(out, { recursive: true });
const screenshot = name => writeFile(path.join(out, `${name}.png`), execFileSync(adb, ['-s', serial, 'exec-out', 'screencap', '-p'], { timeout: 60000 }));
let socket;
const events = [];
const fontScale = run('shell', 'settings', 'get', 'system', 'font_scale').trim();
async function connect() {
  if (process.env.VOICE_ANDROID_DEBUG_HELPER) execFileSync(process.env.VOICE_ANDROID_DEBUG_HELPER, { timeout: 30000 });
  const pid = run('shell', 'pidof', 'voice.live').trim();
  run('forward', 'tcp:15647', `localabstract:webview_devtools_remote_${pid}`);
  const pages = await (await fetch('http://127.0.0.1:15647/json')).json();
  const page = pages.find(page => page.url.startsWith('https://bddap-bot.github.io/voice/'));
  assert.ok(page, 'Published page is loaded in the installed app');
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  const pending = new Map();
  let id = 0;
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (!message.id) { if (['Runtime.consoleAPICalled', 'Runtime.exceptionThrown', 'Log.entryAdded'].includes(message.method)) events.push(message); return; }
    const reply = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) reply.reject(new Error(message.error.message));
    else reply.resolve(message.result);
  };
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    pending.set(++id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
  await call('Runtime.enable');
  await call('Log.enable');
  return async expression => {
    const result = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    assert.ok(!result.exceptionDetails, 'JavaScript evaluation succeeds');
    return result.result.value;
  };
}
function bounds(text) {
  run('shell', 'uiautomator', 'dump', '/data/local/tmp/appearance.xml');
  const xml = run('shell', 'cat', '/data/local/tmp/appearance.xml');
  const node = [...xml.matchAll(/<node\b[^>]*>/g)].map(match => match[0]).find(node => node.includes(`text="${text}"`));
  return node?.match(/bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"/)?.slice(1).map(Number);
}
function tap(rect) {
  assert.ok(rect, 'Native select dialog exposes its choice');
  run('shell', 'input', 'tap', String((rect[0] + rect[2]) / 2), String((rect[1] + rect[3]) / 2));
}
async function fixture(evaluate, reset) {
  await evaluate(`(() => {
    ${reset ? "localStorage.removeItem('voice.select-regression');" : ''}
    document.body.innerHTML = '<select aria-label="Appearance regression" style="position:fixed;top:100px;left:20px;width:280px;height:60px;font-size:24px"><option>Circle</option><option>Square</option></select>';
    const select = document.querySelector('select');
    select.value = localStorage.getItem('voice.select-regression') || 'Circle';
    select.addEventListener('change', () => localStorage.setItem('voice.select-regression', select.value));
  })()`);
}
async function choose(evaluate, from, to, label) {
  assert.equal(await evaluate("document.querySelector('select').value"), from);
  const rect = await evaluate("(() => { const r = document.querySelector('select').getBoundingClientRect(); return [r.left, r.top, r.right, r.bottom].map(v => v * devicePixelRatio); })()");
  run('shell', 'uiautomator', 'dump', '/data/local/tmp/appearance.xml');
  const web = run('shell', 'cat', '/data/local/tmp/appearance.xml').match(/class="android.webkit.WebView"[^>]*bounds="\[(\d+),(\d+)\]/);
  assert.ok(web, 'WebView window bounds are available');
  tap(rect.map((v, i) => v + Number(web[i % 2 + 1])));
  await screenshot(`${label}-dialog`);
  tap(bounds(to));
  await pause(300);
  assert.equal(await evaluate("document.querySelector('select').value"), to, 'Native selection dispatches the HTML change event');
  assert.equal(await evaluate("localStorage.getItem('voice.select-regression')"), to, 'Change handler persists the selection');
}
try {
  run('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP');
  run('shell', 'wm', 'dismiss-keyguard');
  run('shell', 'am', 'start', '-W', '-n', 'voice.live/.MainActivity');
  run('logcat', '-c');
  let evaluate = await connect();
  await fixture(evaluate, true);
  await choose(evaluate, 'Circle', 'Square', 'initial');
  run('logcat', '-c', '-b', 'events');
  run('shell', 'settings', 'put', 'system', 'font_scale', fontScale === '1.1' ? '1.0' : '1.1');
  let destroyed = false;
  for (let attempt = 0; attempt < 20; attempt++) {
    await pause(500);
    destroyed = /wm_on_destroy_called.*voice\.live\.MainActivity/.test(run('logcat', '-d', '-b', 'events'));
    if (destroyed) break;
  }
  assert.ok(destroyed, 'Activity was destroyed while the service retained the page');
  run('shell', 'am', 'start', '-W', '-n', 'voice.live/.MainActivity');
  assert.equal(await evaluate("document.querySelector('select').value"), 'Square', 'Service retains the page through activity destruction');
  await choose(evaluate, 'Square', 'Circle', 'reattached');
  await pause(6000);
  socket.close();
  run('shell', 'am', 'force-stop', 'voice.live');
  run('shell', 'am', 'start', '-W', '-n', 'voice.live/.MainActivity');
  await pause(3000);
  evaluate = await connect();
  assert.equal(await evaluate("localStorage.getItem('voice.select-regression')"), 'Circle', 'Stored selection survives a process restart');
  await fixture(evaluate, false);
  assert.equal(await evaluate("document.querySelector('select').value"), 'Circle', 'Selection survives a process restart');
  await choose(evaluate, 'Circle', 'Square', 'restarted');
  await screenshot('selected');
  console.log('PASS: native select change, persistence, activity recreation, and process restart');
} finally {
  socket?.close();
  if (fontScale === 'null') run('shell', 'settings', 'delete', 'system', 'font_scale');
  else run('shell', 'settings', 'put', 'system', 'font_scale', fontScale);
  await writeFile(path.join(out, 'console.json'), JSON.stringify(events, null, 2));
  await writeFile(path.join(out, 'logcat.txt'), run('logcat', '-d'));
}
