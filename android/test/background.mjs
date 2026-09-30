import { execFileSync } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';

const adb = `${process.env.ANDROID_HOME}/platform-tools/adb`;
const serial = process.env.ANDROID_SERIAL || 'emulator-5646';
const run = (...args) => execFileSync(adb, ['-s', serial, ...args], { encoding: 'utf8', timeout: 60000, maxBuffer: 64 * 1024 * 1024 });
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
run('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP');
run('shell', 'wm', 'dismiss-keyguard');
run('shell', 'settings', 'put', 'system', 'screen_off_timeout', '600000');
run('shell', 'settings', 'put', 'global', 'stay_on_while_plugged_in', '0');
run('shell', 'settings', 'put', 'global', 'always_finish_activities', '1');
run('shell', 'dumpsys', 'deviceidle', 'enable', 'deep');
run('shell', 'dumpsys', 'battery', 'unplug');
try { run('uninstall', 'voice.live'); } catch {}
run('install', 'android/build/live-voice-test.apk');
run('shell', 'pm', 'grant', 'voice.live', 'android.permission.RECORD_AUDIO');
run('shell', 'pm', 'grant', 'voice.live', 'android.permission.POST_NOTIFICATIONS');
const tap = async text => {
  for (let attempt = 0; attempt < 20; attempt++) {
    await pause(1000);
    try {
      run('shell', 'uiautomator', 'dump', '/data/local/tmp/voice-ui.xml');
      const button = run('shell', 'cat', '/data/local/tmp/voice-ui.xml').match(new RegExp(`<node[^>]*text="(?:${text}|${text.toUpperCase()})"[^>]*bounds="\\[(\\d+),(\\d+)\\]\\[(\\d+),(\\d+)\\]"`));
      if (button) return run('shell', 'input', 'tap', String((+button[1] + +button[3]) / 2), String((+button[2] + +button[4]) / 2));
    } catch {}
  }
  assert.fail(`${text} control is reachable`);
};
let socket;
try {
  run('shell', 'am', 'start', '-W', '-n', 'voice.live/.MainActivity');
  await tap('Open Live Voice');
  let pages;
  for (let attempt = 0; attempt < 90; attempt++) {
    let pid = '';
    try { pid = run('shell', 'pidof', 'voice.live').trim(); } catch {}
    if (pid) {
      run('forward', 'tcp:15646', `localabstract:webview_devtools_remote_${pid}`);
      try { pages = await (await fetch('http://127.0.0.1:15646/json')).json(); } catch {}
      if (pages?.some(page => page.url.startsWith('https://bddap-bot.github.io/voice/'))) break;
    }
    await pause(1000);
  }
  const page = pages?.find(page => page.url.startsWith('https://bddap-bot.github.io/voice/'));
  assert.ok(page, 'Live WebView was loaded');
  let next = 0;
  const pending = new Map();
  const connect = async () => {
    socket = new WebSocket(page.webSocketDebuggerUrl.replace(/localhost:\d+|127\.0\.0\.1:\d+/, '127.0.0.1:15646'));
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; setTimeout(() => reject(Error('DevTools connection timed out')), 30000); });
    socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (pending.has(message.id)) {
      const { resolve, reject, timer } = pending.get(message.id);
      pending.delete(message.id);
      clearTimeout(timer);
      if (message.error) reject(Error(JSON.stringify(message.error)));
      else resolve(message.result);
    }
  };
  };
  await connect();
  const evaluate = expression => new Promise((resolve, reject) => {
    const id = ++next;
    const timer = setTimeout(() => { pending.delete(id); reject(Error('WebView evaluation timed out')); }, 60000);
    pending.set(id, { resolve: result => result.exceptionDetails ? reject(Error(JSON.stringify(result.exceptionDetails))) : resolve(result.result.value), reject, timer });
    socket.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true, userGesture: true } }));
  });
  for (let attempt = 0; attempt < 60; attempt++) {
    let ready = false;
    try { ready = await evaluate("location.origin === 'https://bddap-bot.github.io' && document.readyState === 'complete'"); } catch {}
    if (ready) break;
    if (attempt === 59) throw Error('Live page did not finish loading');
    await pause(1000);
  }
  console.log('Loaded', await evaluate('location.href'));
  await evaluate(`(async () => {
    const { LivePlayback } = await import('https://bddap-bot.github.io/voice/live-playback.js');
    const mic = await navigator.mediaDevices.getUserMedia({ audio: true });
    const a = new RTCPeerConnection(), b = new RTCPeerConnection();
    const speaker = new Audio();
    speaker.autoplay = true;
    document.body.append(speaker);
    const playback = new LivePlayback(error => { window.probeError = String(error); }, speaker);
    const attached = new Promise(resolve => a.ontrack = async event => {
      speaker.srcObject = await playback.attach(event.streams[0]);
      await speaker.play();
      resolve();
    });
    const replyContext = new AudioContext();
    const tone = replyContext.createOscillator(), gain = replyContext.createGain(), reply = replyContext.createMediaStreamDestination();
    gain.gain.value = 0.05;
    tone.connect(gain).connect(reply);
    tone.start();
    await replyContext.resume();
    for (const track of reply.stream.getTracks()) b.addTrack(track, reply.stream);
    for (const track of mic.getTracks()) a.addTrack(track, mic);
    const gather = pc => pc.iceGatheringState === 'complete' ? Promise.resolve() : new Promise(resolve => pc.addEventListener('icegatheringstatechange', () => { if (pc.iceGatheringState === 'complete') resolve(); }));
    await a.setLocalDescription(await a.createOffer());
    await gather(a);
    await b.setRemoteDescription(a.localDescription);
    await b.setLocalDescription(await b.createAnswer());
    await gather(b);
    await a.setRemoteDescription(b.localDescription);
    await attached;
    const analyser = playback.context.createAnalyser();
    playback.context.createMediaStreamSource(playback.outputStream).connect(analyser);
    window.probe = { mic, a, b, playback, speaker, analyser, replyContext, tone };
    return true;
  })()`);
  const sample = async label => {
    const wall = Date.now() / 1000;
    const data = await evaluate(`(async () => {
      const { mic, a, b, playback, speaker, analyser } = probe;
      const outbound = [...(await a.getStats()).values()].find(s => s.type === 'outbound-rtp' && s.kind === 'audio');
      const source = [...(await a.getStats()).values()].find(s => s.type === 'media-source' && s.kind === 'audio');
      const inbound = [...(await b.getStats()).values()].find(s => s.type === 'inbound-rtp' && s.kind === 'audio');
      const reply = [...(await a.getStats()).values()].find(s => s.type === 'inbound-rtp' && s.kind === 'audio');
      const state = { replyReceived: reply?.packetsReceived || 0, visibility: document.visibilityState, mic: mic.getAudioTracks()[0].readyState, muted: mic.getAudioTracks()[0].muted,
        peer: a.connectionState, context: playback.context.state, contextTime: playback.context.currentTime,
        outputTime: speaker.currentTime, paused: speaker.paused, sent: outbound?.packetsSent || 0,
        received: inbound?.packetsReceived || 0, samples: source?.totalSamplesDuration || 0, error: window.probeError || null };
      const wave = new Float32Array(analyser.fftSize);
      let peak = 0;
      for (let read = 0; read < 20 && peak <= 0.01; read++) {
        analyser.getFloatTimeDomainData(wave);
        peak = wave.reduce((value, sample) => Math.max(value, Math.abs(sample)), peak);
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      return { ...state, peak };
    })()`);
    data.recording = /active\? true\n[^\n]*pack:voice\.live[^\n]*silenced:false/.test(run('shell', 'dumpsys', 'audio'));
    data.wall = wall;
    console.log(label, JSON.stringify(data));
    return { label, ...data };
  };
  await pause(5000);
  const rows = [await sample('start')];
  const interval = async label => {
    socket.close();
    await pause(30000);
    await connect();
    rows.push(await sample(label));
  };
  await interval('foreground-30s');
  run('shell', 'input', 'keyevent', 'KEYCODE_HOME');
  await interval('background-30s');
  run('shell', 'input', 'keyevent', 'KEYCODE_SLEEP');
  let power = '', display = '';
  for (let attempt = 0; attempt < 20; attempt++) {
    await pause(1000);
    power = run('shell', 'dumpsys', 'power');
    display = run('shell', 'dumpsys', 'display');
    if (/mWakefulness=(Asleep|Dozing)/.test(power) && /mScreenState=OFF|state OFF|state=OFF/.test(display)) break;
  }
  assert.match(power, /mWakefulness=(Asleep|Dozing)/, 'Android confirms the device is sleeping or dozing');
  assert.match(display, /mScreenState=OFF|state OFF|state=OFF/, 'Android confirms the display is off');
  run('shell', 'dumpsys', 'deviceidle', 'force-idle');
  assert.equal(run('shell', 'dumpsys', 'deviceidle', 'get', 'deep').trim(), 'IDLE', 'Android is in deep Doze');
  await interval('screen-off-30s');
  const rate = (row, key) => (row[key] - rows[rows.indexOf(row) - 1][key]) / (row.wall - rows[rows.indexOf(row) - 1].wall);
  const rates = rows.slice(1).map(row => ({ label: row.label, ...Object.fromEntries(['samples', 'contextTime', 'outputTime', 'sent', 'received', 'replyReceived'].map(key => [key, +rate(row, key).toFixed(3)])) }));
  console.log('rates per wall second', JSON.stringify(rates));
  const [control, ...hidden] = rows.slice(1);
  for (const row of rows.slice(1)) {
    assert.equal(row.visibility, row === control ? 'visible' : 'hidden');
    assert.equal(row.mic, 'live');
    assert.equal(row.muted, false);
    assert.equal(row.peer, 'connected');
    assert.equal(row.context, 'running');
    assert.equal(row.paused, false);
    assert.equal(row.error, null);
    assert.equal(row.recording, true, 'Android reports an active, unsilenced voice.live recording');
    assert.ok(row.peak > 0.01, 'reply processing produces nonzero audio');
  }
  for (const row of hidden) for (const key of ['samples', 'contextTime', 'outputTime', 'sent', 'received', 'replyReceived']) {
    assert.ok(rate(control, key) > 0, `${key} advances in the visible control interval`);
    assert.ok(rate(control, 'samples') > 0.25, 'visible capture runs at least a quarter of real time');
    assert.ok(rate(row, key) >= rate(control, key) / 4, `${row.label} ${key} keeps at least a quarter of the visible control rate`);
  }
  run('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP');
  run('shell', 'wm', 'dismiss-keyguard');
  run('shell', 'am', 'start', '-n', 'voice.live/.MainActivity');
  await tap('Stop and close');
  let services = '';
  for (let attempt = 0; attempt < 20; attempt++) {
    await pause(1000);
    services = run('shell', 'dumpsys', 'activity', 'services', 'voice.live');
    if (!services.includes('isForeground=true')) break;
  }
  assert.ok(!services.includes('isForeground=true'), 'Stop releases foreground service');
  await writeFile(process.env.VOICE_ANDROID_EVIDENCE || 'android/build/background-evidence.json', JSON.stringify({ scope: 'Android emulator duplex WebRTC: real microphone capture and synthetic reply tone through deployed LivePlayback; no model or physical acoustic verification', stopped: true, screenOff: /mScreenState=OFF|state OFF|state=OFF/.test(display), wakefulness: power.match(/mWakefulness=(\w+)/)?.[1], rows }, null, 2));
  console.log('PASS: microphone, RTP and playback advance while backgrounded and screen off; Stop releases service');
} finally {
  socket?.close();
  for (const command of [['dumpsys', 'deviceidle', 'unforce'], ['settings', 'put', 'global', 'always_finish_activities', '0'], ['am', 'force-stop', 'voice.live']]) {
    try { run('shell', ...command); } catch {}
  }
  try { run('forward', '--remove', 'tcp:15646'); } catch {}
}
