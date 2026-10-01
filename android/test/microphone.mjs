import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, mkdir, writeFile } from 'node:fs/promises';

const adb = `${process.env.ANDROID_HOME}/platform-tools/adb`;
const serial = process.env.ANDROID_SERIAL || 'emulator-5646';
const out = process.env.VOICE_ANDROID_EVIDENCE_DIR || 'android/build/microphone';
const local = process.env.VOICE_ANDROID_PAGE_ROOT;
const run = (...args) => execFileSync(adb, ['-s', serial, ...args], { encoding: 'utf8', timeout: 60000 });
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
await mkdir(out, { recursive: true });
run('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP');
run('shell', 'wm', 'dismiss-keyguard');
run('shell', 'am', 'start', '-W', '-n', 'voice.live/.MainActivity');
if (process.env.VOICE_ANDROID_DEBUG_HELPER) execFileSync(process.env.VOICE_ANDROID_DEBUG_HELPER, { timeout: 30000 });
run('forward', 'tcp:15832', `localabstract:webview_devtools_remote_${run('shell', 'pidof', 'voice.live').trim()}`);
const c = await connect();
const active = text => /active\? true\n[^\n]*pack:voice\.live/.test(text);
const unsilenced = text => /active\? true\n[^\n]*pack:voice\.live[^\n]*silenced:false/.test(text);
let fixtureError;
const rows = [];
try {
  c.onEvent(message => {
    if (message.method !== 'Fetch.requestPaused') return;
    (async () => {
      const { requestId, request } = message.params;
      let body, type;
      if (request.url.endsWith('/microphone.js')) {
        body = await readFile(`${local}/microphone.js`, 'utf8');
        type = 'text/javascript';
      } else {
        const response = local ? null : await c.call('Fetch.getResponseBody', { requestId });
        body = local ? await readFile(`${local}/index.html`, 'utf8') : response.base64Encoded ? Buffer.from(response.body, 'base64').toString() : response.body;
        assert.ok(body.includes('function acquireMicrophone()'));
        body = body.replace('</script>', `
          globalThis.probeStart = async () => {
            savedState(true);
            globalThis.probeMic = await acquireMicrophone();
          };
        </script>`);
        type = 'text/html';
      }
      await c.call('Fetch.fulfillRequest', { requestId, responseCode: 200, responseHeaders: [{ name: 'Content-Type', value: type }], body: Buffer.from(body).toString('base64') });
    })().catch(error => { fixtureError = error; });
  });
  await c.call('Fetch.enable', { patterns: [
    { urlPattern: 'https://bddap-bot.github.io/voice/', resourceType: 'Document', requestStage: 'Response' },
    ...(local ? [{ urlPattern: '*/microphone.js', requestStage: 'Request' }] : []),
  ] });
  await c.call('Page.reload', { ignoreCache: true });
  for (let attempt = 0; attempt < 60; attempt++) {
    if (fixtureError) throw fixtureError;
    if (await c.evaluate("typeof probeStart === 'function'")) break;
    assert.ok(attempt < 59, 'capture fixture loads');
    await pause(500);
  }
  await c.evaluate('probeStart()');
  for (const phase of ['visible', 'background', 'screen-off']) {
    if (phase === 'background') run('shell', 'input', 'keyevent', 'KEYCODE_HOME');
    if (phase === 'screen-off') {
      run('shell', 'input', 'keyevent', 'KEYCODE_SLEEP');
      await pause(1500);
      const power = run('shell', 'dumpsys', 'power');
      await writeFile(`${out}/power.txt`, power);
      assert.match(power, /mWakefulness=(Asleep|Dozing)/);
    }
    for (const muted of [true, false, true, false]) {
      const start = performance.now();
      await c.evaluate("document.getElementById('mic-mute').click()");
      let audio;
      for (let attempt = 0; attempt < 40; attempt++) {
        audio = run('shell', 'dumpsys', 'audio');
        if (muted ? !active(audio) : unsilenced(audio)) break;
        await pause(100);
      }
      const ms = Math.round(performance.now() - start);
      const label = `${phase}-${rows.length}-${muted ? 'muted' : 'live'}`;
      await writeFile(`${out}/${label}-audio.txt`, audio);
      await writeFile(`${out}/${label}-policy.txt`, run('shell', 'dumpsys', 'media.audio_policy'));
      const services = run('shell', 'dumpsys', 'activity', 'services', 'voice.live');
      await writeFile(`${out}/${label}-services.txt`, services);
      assert.equal(active(audio), !muted, `${phase}: mute releases the OS recording client`);
      if (!muted) assert.ok(unsilenced(audio), `${phase}: reacquired capture is not silenced`);
      assert.equal(await c.evaluate("document.getElementById('mic-mute').getAttribute('aria-pressed')"), String(muted));
      const row = { phase, muted, ms, recording: active(audio) };
      rows.push(row);
      console.log(JSON.stringify(row));
    }
  }
  await writeFile(`${out}/result.json`, JSON.stringify({ scope: 'Real capture and page mute handler; fixture starts capture without a model session', rows }, null, 2));
  console.log('PASS: mute releases the OS recording client; unmute reacquires unsilenced capture while visible, backgrounded, and screen off');
} finally {
  c.close();
  run('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP');
  run('forward', '--remove', 'tcp:15832');
}

async function connect() {
  const pages = await (await fetch('http://127.0.0.1:15832/json')).json();
  const page = pages.find(page => page.url.startsWith('https://bddap-bot.github.io/voice/'));
  assert.ok(page, 'the installed app has loaded the published page');
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let id = 0, handler = () => {};
  const pending = new Map();
  socket.onclose = () => {
    for (const item of pending.values()) item.reject(new Error('WebView debugging connection closed'));
  };
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (!message.id) return handler(message);
    const item = pending.get(message.id);
    if (!item) return;
    pending.delete(message.id);
    if (message.error) item.reject(new Error(JSON.stringify(message.error)));
    else item.resolve(message.result);
  };
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${method} timed out`)), 30000);
    pending.set(++id, {
      resolve: value => { clearTimeout(timer); resolve(value); },
      reject: error => { clearTimeout(timer); reject(error); },
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const result = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  return { call, evaluate, onEvent: callback => { handler = callback; }, close: () => socket.close() };
}
