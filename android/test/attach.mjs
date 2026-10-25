import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import path from 'node:path';
import config from '../../docs/config.js';

const adb = `${process.env.ANDROID_HOME}/platform-tools/adb`;
const serial = process.env.ANDROID_SERIAL || 'emulator-5646';
const apk = process.env.VOICE_ANDROID_APK || 'android/build/live-voice-test.apk';
const out = process.env.VOICE_ANDROID_EVIDENCE_DIR || 'android/build/attach';
const image = `live-voice-attach-${Date.now()}.png`;
const run = (...args) => execFileSync(adb, ['-s', serial, ...args], { encoding: 'utf8', timeout: 60000, maxBuffer: 64 << 20 });
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
await mkdir(out, { recursive: true });
const screenshot = name => writeFile(path.join(out, `${name}.png`), execFileSync(adb, ['-s', serial, 'exec-out', 'screencap', '-p'], { timeout: 60000, maxBuffer: 64 << 20 }));
const focus = () => run('shell', 'dumpsys', 'window').match(/mCurrentFocus=Window\{\S+ \S+ ([^}]+)\}/)?.[1] ?? '';
let socket;
async function connect() {
  const pid = run('shell', 'pidof', 'voice.live').trim();
  run('forward', 'tcp:15648', `localabstract:webview_devtools_remote_${pid}`);
  let page;
  for (let attempt = 0; attempt < 30 && !page; attempt++) {
    page = (await (await fetch('http://127.0.0.1:15648/json')).json()).find(page => page.url.startsWith(`${config.origin}/`));
    if (!page) await pause(1000);
  }
  assert.ok(page, 'Published page is loaded in the installed app');
  socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  const pending = new Map();
  let id = 0;
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    const reply = pending.get(message.id);
    if (!reply) return;
    pending.delete(message.id);
    if (message.error) reply.reject(new Error(message.error.message));
    else reply.resolve(message.result);
  };
  socket.onclose = () => { for (const reply of pending.values()) reply.reject(new Error('DevTools connection closed')); };
  return async expression => {
    const result = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`Evaluation timed out: ${expression.slice(0, 80)}`)), 30000);
      pending.set(++id, { resolve: value => { clearTimeout(timer); resolve(value); }, reject: error => { clearTimeout(timer); reject(error); } });
      socket.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, returnByValue: true, awaitPromise: true } }));
    });
    assert.ok(!result.exceptionDetails, 'JavaScript evaluation succeeds');
    return result.result.value;
  };
}
function nodes() {
  try {
    run('shell', 'uiautomator', 'dump', '/data/local/tmp/attach.xml');
  } catch {
    return [];
  }
  return [...run('shell', 'cat', '/data/local/tmp/attach.xml').matchAll(/<node\b[^>]*>/g)].map(match => match[0]);
}
function tap(rect) {
  run('shell', 'input', 'tap', String(Math.round((rect[0] + rect[2]) / 2)), String(Math.round((rect[1] + rect[3]) / 2)));
}
async function find(match, what, attempts = 20) {
  for (let attempt = 0; attempt < attempts; attempt++) {
    const rect = nodes().find(match)?.match(/bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"/)?.slice(1).map(Number);
    if (rect) return rect;
    await pause(500);
  }
  await screenshot(`missing-${what}`);
  assert.fail(`${what} is on screen`);
}
async function seen(match, attempts) {
  for (let attempt = 0; attempt < attempts; attempt++) {
    if (nodes().some(match)) return true;
    await pause(500);
  }
  return false;
}
const tapNode = async (match, what) => tap(await find(match, what));
async function chooseImageButton(evaluate) {
  const rect = await evaluate(`(async () => {
    if (!document.getElementById('controls').classList.contains('open')) document.getElementById('card-toggle').click();
    const input = document.getElementById('share-file');
    if (!input.dataset.taps) input.addEventListener('click', () => { input.dataset.taps = Number(input.dataset.taps) + 1; });
    input.dataset.taps ||= 0;
    const label = document.querySelector('label[for=share-file]');
    label.scrollIntoView({ block: 'center' });
    await new Promise(resolve => setTimeout(resolve, 300));
    const r = label.getBoundingClientRect();
    return [r.left, r.top, r.right, r.bottom].map(v => v * devicePixelRatio);
  })()`);
  const web = await find(node => node.includes('class="android.webkit.WebView"'), 'WebView');
  const taps = Number(await evaluate("document.getElementById('share-file').dataset.taps"));
  tap(rect.map((v, i) => v + web[i % 2]));
  await pause(300);
  assert.equal(Number(await evaluate("document.getElementById('share-file').dataset.taps")), taps + 1, 'The tap activates the file input');
}
async function pickerOpens(evaluate, label) {
  await chooseImageButton(evaluate);
  let shown = '';
  for (let attempt = 0; attempt < 20 && !shown; attempt++) {
    await pause(500);
    const now = focus();
    if (now && !now.startsWith('voice.live/')) shown = now;
  }
  await screenshot(label);
  assert.ok(shown, `Tapping Choose image opens a system picker (focus stayed on ${focus()})`);
  return shown;
}
try {
  run('install', '-r', '-g', apk);
  execFileSync('magick', ['-size', '64x48', 'xc:#3a7', '-define', 'png:exclude-chunks=date,time', path.join(out, image)]);
  run('push', path.join(out, image), `/sdcard/Download/${image}`);
  run('shell', 'am', 'broadcast', '-a', 'android.intent.action.MEDIA_SCANNER_SCAN_FILE', '-d', `file:///sdcard/Download/${image}`);
  run('logcat', '-c');
  run('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP');
  run('shell', 'wm', 'dismiss-keyguard');
  run('shell', 'am', 'force-stop', 'voice.live');
  run('shell', 'am', 'start', '-W', '-n', 'voice.live/.MainActivity');
  const evaluate = await connect();
  await pause(2000);

  const chooser = await pickerOpens(evaluate, 'chooser');
  await find(node => /text="Camera"/.test(node), 'Camera target');
  run('shell', 'input', 'keyevent', 'KEYCODE_BACK');
  for (let attempt = 0; attempt < 20 && !focus().startsWith('voice.live/'); attempt++) await pause(500);
  assert.equal(await evaluate("document.getElementById('share-image').textContent"), 'No image selected', 'Cancelling selects nothing');

  await pickerOpens(evaluate, 'chooser-again');
  await tapNode(node => /text="(Media picker|Photos|Files|Gallery)"/.test(node), 'gallery target');
  await tapNode(node => node.includes('id/icon_thumbnail') || node.includes(`text="${image}"`), 'pushed image');
  for (let attempt = 0; attempt < 20 && !focus().startsWith('voice.live/'); attempt++) await pause(500);
  const read = "(async () => { const file = document.getElementById('share-file').files[0]; return file && { name: file.name, type: file.type, bytes: [...new Uint8Array(await file.arrayBuffer())] }; })()";
  let chosen;
  for (let attempt = 0; attempt < 20 && !chosen; attempt++) {
    chosen = await evaluate(read);
    if (!chosen) await pause(500);
  }
  await screenshot('chosen');
  assert.ok(chosen, 'The page receives the chosen image');
  assert.equal(chosen.type, 'image/png');
  assert.deepEqual(Buffer.from(chosen.bytes), await readFile(path.join(out, image)), 'The page reads the gallery image byte for byte');

  await evaluate("document.getElementById('share-remove').click()");
  await pickerOpens(evaluate, 'chooser-camera');
  await tapNode(node => /text="Camera"/.test(node), 'Camera target');
  const confirm = node => /content-desc="Done"/.test(node);
  for (let attempt = 0; attempt < 5 && !await seen(confirm, attempt ? 6 : 0); attempt++) await tapNode(node => /content-desc="Shutter"/.test(node), 'camera shutter');
  await tapNode(confirm, 'camera confirm');
  for (let attempt = 0; attempt < 20 && !focus().startsWith('voice.live/'); attempt++) await pause(500);
  let photo;
  for (let attempt = 0; attempt < 20 && !photo; attempt++) {
    photo = await evaluate("(() => { const file = document.getElementById('share-file').files[0]; return file && { name: file.name, type: file.type, size: file.size }; })()");
    if (!photo) await pause(500);
  }
  await screenshot('photo');
  assert.ok(photo?.size > 0, 'The page receives the camera photo');
  assert.equal(photo.type, 'image/jpeg');
  console.log(`PASS: Choose image opened ${chooser} with Camera; cancel returned nothing; the gallery delivered ${chosen.name} (${chosen.bytes.length} bytes, identical); the camera delivered ${photo.name} (${photo.size} bytes)`);
} finally {
  socket?.close();
  run('shell', 'rm', '-f', `/sdcard/Download/${image}`);
  run('shell', 'am', 'broadcast', '-a', 'android.intent.action.MEDIA_SCANNER_SCAN_FILE', '-d', `file:///sdcard/Download/${image}`);
  await writeFile(path.join(out, 'logcat.txt'), run('logcat', '-d'));
}
