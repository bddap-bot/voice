import { spawn, execFileSync } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';

const adb = `${process.env.ANDROID_HOME}/platform-tools/adb`;
const serial = process.env.ANDROID_SERIAL || 'emulator-5646';
const run = (...args) => execFileSync(adb, ['-s', serial, ...args], { encoding: 'utf8', timeout: 60000 });
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
run('install', '-r', 'android/build/live-voice.apk');
run('install', '-r', 'android/build/probe.apk');
run('shell', 'pm', 'grant', 'voice.live', 'android.permission.RECORD_AUDIO');
run('shell', 'pm', 'grant', 'voice.live', 'android.permission.POST_NOTIFICATIONS');
const instrument = spawn(adb, ['-s', serial, 'shell', 'am', 'instrument', '-w', 'voice.live.probe/voice.live.Probe'], { stdio: 'ignore' });
let socket;
try {
  let pages;
  for (let attempt = 0; attempt < 90; attempt++) {
    const pid = run('shell', 'sh', '-c', 'pidof voice.live || true').trim();
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
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
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
  await evaluate(`(async () => {
    const { LivePlayback } = await import('./live-playback.js');
    const mic = await navigator.mediaDevices.getUserMedia({ audio: true });
    const a = new RTCPeerConnection(), b = new RTCPeerConnection();
    const speaker = new Audio();
    speaker.autoplay = true;
    document.body.append(speaker);
    const playback = new LivePlayback(error => { window.probeError = String(error); }, speaker);
    const attached = new Promise(resolve => b.ontrack = async event => {
      speaker.srcObject = await playback.attach(event.streams[0]);
      await speaker.play();
      resolve();
    });
    for (const track of mic.getTracks()) a.addTrack(track, mic);
    const gather = pc => pc.iceGatheringState === 'complete' ? Promise.resolve() : new Promise(resolve => pc.addEventListener('icegatheringstatechange', () => { if (pc.iceGatheringState === 'complete') resolve(); }));
    await a.setLocalDescription(await a.createOffer());
    await gather(a);
    await b.setRemoteDescription(a.localDescription);
    await b.setLocalDescription(await b.createAnswer());
    await gather(b);
    await a.setRemoteDescription(b.localDescription);
    await attached;
    window.probe = { mic, a, b, playback, speaker };
    return true;
  })()`);
  const sample = async label => {
    const data = await evaluate(`(async () => {
      const { mic, a, b, playback, speaker } = probe;
      const outbound = [...(await a.getStats()).values()].find(s => s.type === 'outbound-rtp' && s.kind === 'audio');
      const source = [...(await a.getStats()).values()].find(s => s.type === 'media-source' && s.kind === 'audio');
      const inbound = [...(await b.getStats()).values()].find(s => s.type === 'inbound-rtp' && s.kind === 'audio');
      return { visibility: document.visibilityState, mic: mic.getAudioTracks()[0].readyState, muted: mic.getAudioTracks()[0].muted,
        peer: a.connectionState, context: playback.context.state, contextTime: playback.context.currentTime,
        outputTime: speaker.currentTime, paused: speaker.paused, sent: outbound?.packetsSent || 0,
        received: inbound?.packetsReceived || 0, samples: source?.totalSamplesDuration || 0, error: window.probeError || null };
    })()`);
    console.log(label, JSON.stringify(data));
    return { label, ...data };
  };
  await pause(5000);
  const rows = [await sample('foreground')];
  run('shell', 'input', 'keyevent', 'KEYCODE_HOME');
  socket.close();
  await pause(30000);
  await connect();
  rows.push(await sample('background-30s'));
  run('shell', 'input', 'keyevent', 'KEYCODE_SLEEP');
  await pause(1000);
  const power = run('shell', 'dumpsys', 'power');
  assert.match(power, /mWakefulness=Asleep/, 'Android confirms the screen is asleep');
  socket.close();
  await pause(30000);
  await connect();
  rows.push(await sample('screen-off-30s'));
  for (let index = 1; index < rows.length; index++) {
    const row = rows[index], previous = rows[index - 1];
    assert.equal(row.visibility, 'hidden');
    assert.equal(row.mic, 'live');
    assert.equal(row.muted, false);
    assert.equal(row.peer, 'connected');
    assert.equal(row.context, 'running');
    assert.equal(row.paused, false);
    assert.equal(row.error, null);
    assert.ok(row.sent > previous.sent + 100, 'microphone RTP keeps advancing');
    assert.ok(row.received > previous.received + 100, 'reply RTP keeps advancing');
    assert.ok(row.samples > previous.samples + 10, 'captured sample duration keeps advancing');
    assert.ok(row.contextTime > previous.contextTime + 10, 'playback processing keeps advancing');
    assert.ok(row.outputTime > previous.outputTime + 10, 'speaker playback keeps advancing');
  }
  await writeFile(process.env.VOICE_ANDROID_EVIDENCE || 'android/build/background-evidence.json', JSON.stringify({ scope: 'Android emulator microphone/WebRTC loopback through deployed LivePlayback; no model or physical acoustic verification', screenAsleep: /mWakefulness=Asleep/.test(power), rows }, null, 2));
  run('shell', 'input', 'keyevent', 'KEYCODE_WAKEUP');
  run('shell', 'wm', 'dismiss-keyguard');
  run('shell', 'am', 'start', '-n', 'voice.live/.MainActivity');
  await pause(1000);
  run('shell', 'uiautomator', 'dump', '/data/local/tmp/voice-ui.xml');
  const ui = run('shell', 'cat', '/data/local/tmp/voice-ui.xml');
  const button = ui.match(/<node[^>]*text="Stop and close"[^>]*bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"/);
  assert.ok(button, 'Stop and close control is reachable');
  run('shell', 'input', 'tap', String((+button[1] + +button[3]) / 2), String((+button[2] + +button[4]) / 2));
  await pause(1000);
  const services = run('shell', 'dumpsys', 'activity', 'services', 'voice.live');
  assert.ok(!services.includes('isForeground=true'), 'Stop releases foreground service');
  console.log('PASS: microphone, RTP and playback advance while backgrounded and screen off; Stop releases service');
} finally {
  socket?.close();
  run('shell', 'am', 'force-stop', 'voice.live');
  run('forward', '--remove', 'tcp:15646');
  instrument.kill();
}
