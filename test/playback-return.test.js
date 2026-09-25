import assert from 'node:assert/strict';
import test from 'node:test';
import { spawn } from 'node:child_process';
import { access, mkdtemp, readdir, readFile, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { createServer } from 'node:http';
import { join } from 'node:path';

// Exercise actual decoded audio. Inject the platform audio interruption and,
// in mobile app mode, overlay lifecycle events: desktop Chromium cannot open an
// Android embedded browser. Desktop coverage uses real tab visibility/focus.
for (const mobile of [false, true]) test(`audio returns after ${mobile ? 'standalone Android app cover' : 'another tab'}`, { timeout: 30000 }, async (t) => {
  const candidates = process.env.CHROMIUM_BIN ? [process.env.CHROMIUM_BIN] : [];
  candidates.push(...(process.env.PATH ?? '').split(':').map(p => join(p, 'chromium')));
  try { candidates.push(...(await readdir('/nix/store')).filter(n => n.includes('-chromium-')).map(n => `/nix/store/${n}/bin/chromium`)); } catch {}
  let executable;
  for (const candidate of candidates) { try { await access(candidate, constants.X_OK); executable = candidate; break; } catch {} }
  assert.ok(executable, 'Chromium required');
  const scratch = await mkdtemp(join(process.cwd(), '.playback-return-'));
  const server = createServer(async (request, response) => {
    if (request.url === '/') return response.end('<!doctype html><title>Return</title><audio id="speaker" autoplay></audio><a href="/covered">Panel link</a>');
    if (request.url === '/covered') return response.end('<!doctype html><title>Covered</title>Linked page');
    try { response.setHeader('content-type', 'text/javascript'); response.end(await readFile(new URL('../docs' + request.url, import.meta.url))); }
    catch { response.writeHead(404).end(); }
  });
  let chrome, socket;
  try {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    chrome = spawn(executable, ['--headless=new', '--no-sandbox', '--autoplay-policy=no-user-gesture-required', `--user-data-dir=${scratch}`, '--remote-debugging-port=0', ...(mobile ? [`--app=${origin}`] : [origin])], { stdio: 'ignore' });
    let port;
    for (let i = 0; i < 100; i++) { try { port = Number((await readFile(join(scratch, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); break; } catch { await new Promise(r => setTimeout(r, 50)); } }
    assert.ok(port);
    let page;
    for (let i = 0; i < 100; i++) {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      page = targets.find(t => t.type === 'page' && t.title === 'Return');
      if (page) break;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    assert.ok(page, 'page loaded');
    socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
    let sequence = 0;
    const pending = new Map();
    socket.onmessage = ({ data }) => { const message = JSON.parse(data); const entry = pending.get(message.id); if (entry) { pending.delete(message.id); message.error ? entry.reject(new Error(JSON.stringify(message.error))) : entry.resolve(message.result); } };
    const call = (method, params = {}) => new Promise((resolve, reject) => { const id = ++sequence; pending.set(id, { resolve, reject }); socket.send(JSON.stringify({ id, method, params })); });
    const evaluate = async expression => {
      const result = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      assert.ok(!result.exceptionDetails, JSON.stringify(result.exceptionDetails));
      return result.result.value;
    };
    if (mobile) {
      await call('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 2, mobile: true });
      await call('Emulation.setUserAgentOverride', { userAgent: 'Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 Chrome/130.0.0.0 Mobile Safari/537.36' });
      await call('Emulation.setEmulatedMedia', { features: [{ name: 'display-mode', value: 'standalone' }] });
      assert.equal(await evaluate("matchMedia('(display-mode: standalone)').matches"), true);
    }
    await evaluate(`(async () => {
      const { LivePlayback } = await import('/live-playback.js');
      window.events = [];
      document.addEventListener('visibilitychange', () => events.push(document.visibilityState));
      for (const name of ['blur', 'focus']) addEventListener(name, () => events.push(name));
      window.failures = []; window.transcripts = [];
      window.playback = new LivePlayback(text => transcripts.push(text), error => failures.push(error.message), document.querySelector('#speaker'));
      window.context = new AudioContext();
      const oscillator = context.createOscillator(), destination = context.createMediaStreamDestination();
      oscillator.connect(destination); oscillator.start(); await context.resume();
      window.sender = new RTCPeerConnection(); window.receiver = new RTCPeerConnection();
      sender.onicecandidate = ({ candidate }) => { if (candidate) receiver.addIceCandidate(candidate); };
      receiver.onicecandidate = ({ candidate }) => { if (candidate) sender.addIceCandidate(candidate); };
      const incoming = new Promise(resolve => receiver.ontrack = event => resolve(event.streams[0]));
      sender.addTrack(destination.stream.getAudioTracks()[0], destination.stream);
      await sender.setLocalDescription(await sender.createOffer()); await receiver.setRemoteDescription(sender.localDescription);
      await receiver.setLocalDescription(await receiver.createAnswer()); await sender.setRemoteDescription(receiver.localDescription);
      const speaker = document.querySelector('#speaker');
      speaker.srcObject = await playback.attach(await incoming); await speaker.play();
      // Capture the final audible element, not merely the upstream worklet stream.
      const analyser = context.createAnalyser();
      context.createMediaStreamSource(speaker.captureStream()).connect(analyser);
      const silent = context.createGain(); silent.gain.value = 0; analyser.connect(silent).connect(context.destination);
      window.peak = async () => {
        let peak = 0;
        for (let i = 0; i < 20; i++) {
          await new Promise(resolve => setTimeout(resolve, 30));
          const samples = new Float32Array(analyser.fftSize); analyser.getFloatTimeDomainData(samples);
          peak = Math.max(peak, ...samples.map(Math.abs));
        }
        return peak;
      };
    })()`);
    assert.ok(await evaluate('peak()') > 0.1, 'audible before leaving');
    // Leave the live document intact, as an embedded browser overlay does.
    const cover = await call('Target.createTarget', { url: `${origin}/covered` });
    await call('Target.activateTarget', { targetId: cover.targetId });
    if (mobile) await evaluate(`dispatchEvent(new Event('blur')); Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' }); Object.defineProperty(document, 'hidden', { configurable: true, value: true }); document.dispatchEvent(new Event('visibilitychange'));`);
    await evaluate(`(async () => {
      await playback.context.suspend(); playback.sink.pause(); document.querySelector('#speaker').pause();
      playback.transcript('Reply while covered');
      await new Promise(resolve => setTimeout(resolve, 300));
    })()`);
    assert.equal(await evaluate('document.visibilityState'), 'hidden');
    assert.equal(await evaluate('peak()'), 0, 'interrupted pipeline is silent');
    await call('Target.activateTarget', { targetId: page.id });
    if (mobile) await evaluate(`delete document.visibilityState; delete document.hidden; document.dispatchEvent(new Event('visibilitychange')); dispatchEvent(new Event('focus'));`);
    const peak = await evaluate('peak()');
    const state = await evaluate(`({ events, transcripts, failures, state: playback.context.state, muted: document.querySelector('#speaker').muted, volume: document.querySelector('#speaker').volume, paused: document.querySelector('#speaker').paused, decoderPaused: playback.sink.paused, target: document.querySelector('a').target })`);
    for (const event of ['blur', 'hidden', 'visible', 'focus']) assert.ok(state.events.includes(event), JSON.stringify(state.events));
    assert.deepEqual(state.transcripts, ['Reply while covered']);
    assert.ok(peak > 0.1, `audio must recover without reload: ${JSON.stringify({ peak, ...state })}`);
    assert.deepEqual(state.failures, []);
    assert.equal(state.target, '');
    assert.equal(state.paused, false, JSON.stringify(state));
    assert.equal(state.decoderPaused, false);
    assert.equal(state.muted, false);
    assert.ok(state.volume > 0);
    t.diagnostic(JSON.stringify({ peak, ...state }));
    for (const event of ['visibilitychange', 'focus', 'pageshow']) {
      await evaluate(`(async () => {
        await playback.context.suspend(); playback.sink.pause(); playback.speaker.pause();
        ${event === 'visibilitychange' ? 'document' : 'window'}.dispatchEvent(new Event('${event}'));
      })()`);
      assert.ok(await evaluate('peak()') > 0.1, event);
      assert.equal(await evaluate('playback.speaker.paused'), false, event);
    }
    await evaluate(`(async () => {
      playback.speaker.pause();
      const play = playback.speaker.play.bind(playback.speaker);
      playback.speaker.play = () => { playback.speaker.play = play; return Promise.reject(new DOMException('Gesture required', 'NotAllowedError')); };
      dispatchEvent(new Event('focus'));
      await new Promise(resolve => setTimeout(resolve, 50));
    })()`);
    assert.deepEqual(await evaluate('failures'), []);
    assert.equal(await evaluate('playback.speaker.paused'), true);
    await evaluate("dispatchEvent(new Event('pointerdown'))");
    assert.ok(await evaluate('peak()') > 0.1);
    assert.equal(await evaluate('playback.speaker.paused'), false);
    await evaluate('playback.close()');
    await call('Target.activateTarget', { targetId: cover.targetId });
    await call('Target.activateTarget', { targetId: page.id });
    assert.equal(await evaluate('playback.context.state'), 'closed');
    assert.deepEqual(await evaluate('failures'), []);
  } finally {
    socket?.close();
    if (chrome && chrome.exitCode === null) { const exited = new Promise(resolve => chrome.once('exit', resolve)); chrome.kill('SIGKILL'); await exited; }
    await new Promise(resolve => server.close(resolve));
    await rm(scratch, { recursive: true, force: true });
  }
});
