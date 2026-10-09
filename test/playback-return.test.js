import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { launchChromium } from '../scripts/chromium.mjs';

// Exercise actual decoded audio. Inject the platform audio interruption and,
// in mobile app mode, overlay lifecycle events: desktop Chromium cannot open an
// Android embedded browser. Desktop coverage uses real tab visibility/focus.
for (const mobile of [false, true]) test(`audio returns after ${mobile ? 'standalone Android app cover' : 'another tab'}`, async (t) => {
  const server = createServer(async (request, response) => {
    if (request.url === '/') return response.end('<!doctype html><title>Return</title><audio id="speaker" autoplay></audio><a href="/covered">Panel link</a>');
    if (request.url === '/probe.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(`
      registerProcessor('peak-probe', class extends AudioWorkletProcessor {
        constructor() {
          super();
          this.port.onmessage = ({ data }) => { this.silentQuanta = data; this.quiet = 0; };
        }
        process(inputs) {
          if (this.silentQuanta === undefined) return true;
          const peak = (inputs[0]?.[0] ?? []).reduce((value, sample) => Math.max(value, Math.abs(sample)), 0);
          this.quiet = peak === 0 ? this.quiet + 1 : 0;
          if (this.silentQuanta ? this.quiet === this.silentQuanta : peak > 0.1) {
            this.silentQuanta = undefined;
            this.port.postMessage(null);
          }
          return true;
        }
      });
    `);
    if (request.url === '/covered') return response.end('<!doctype html><title>Covered</title>Linked page');
    try { response.setHeader('content-type', 'text/javascript'); response.end(await readFile(new URL('../docs' + request.url, import.meta.url))); }
    catch { response.writeHead(404).end(); }
  });
  let chrome;
  try {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    chrome = await launchChromium({ args: ['--headless=new', '--no-sandbox', '--autoplay-policy=no-user-gesture-required', mobile ? `--app=${origin}` : origin] });
    const pages = (await chrome.devtools.call('Target.getTargets')).targetInfos.filter(target => target.type === 'page');
    assert.equal(pages.length, 1, JSON.stringify(pages));
    const [{ targetId }] = pages;
    const { sessionId } = await chrome.devtools.call('Target.attachToTarget', { targetId, flatten: true });
    const call = (method, params) => chrome.devtools.call(method, params, sessionId);
    await call('Page.enable');
    await call('Page.setLifecycleEventsEnabled', { enabled: true });
    const loads = new Set();
    let loaded = () => {};
    const stop = chrome.devtools.listen(({ method, params }) => {
      if (method === 'Page.lifecycleEvent' && params.name === 'load') { loads.add(params.loaderId); loaded(); }
    });
    const { loaderId, errorText } = await call('Page.navigate', { url: `${origin}/` });
    assert.ok(!errorText, errorText);
    await Promise.race([
      new Promise(resolve => { loaded = () => loads.has(loaderId) && resolve(); loaded(); }),
      chrome.devtools.closed.then(() => { throw new Error('Chromium closed before the page loaded'); }),
    ]);
    stop();
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
      const recorded = new EventTarget();
      const record = name => { events.push(name); recorded.dispatchEvent(new Event('record')); };
      window.seen = (names, from) => new Promise(resolve => {
        const check = () => { if (names.every(name => events.includes(name, from))) { recorded.removeEventListener('record', check); resolve(); } };
        recorded.addEventListener('record', check);
        check();
      });
      document.addEventListener('visibilitychange', () => record(document.visibilityState));
      for (const name of ['blur', 'focus']) addEventListener(name, () => record(name));
      window.failures = [];
      window.playback = new LivePlayback(error => failures.push(error.message), document.querySelector('#speaker'));
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
      await context.audioWorklet.addModule('/probe.js');
      const probe = new AudioWorkletNode(context, 'peak-probe');
      context.createMediaStreamSource(speaker.captureStream()).connect(probe).connect(context.destination);
      const heard = silentQuanta => new Promise(resolve => {
        probe.port.onmessage = () => resolve();
        probe.port.postMessage(silentQuanta);
      });
      window.audible = () => heard(0);
      window.silent = () => heard(Math.ceil(0.6 * context.sampleRate / 128));
    })()`);
    await evaluate('audible()');
    const leaving = await evaluate('events.length');
    const cover = await call('Target.createTarget', { url: `${origin}/covered` });
    await call('Target.activateTarget', { targetId: cover.targetId });
    if (mobile) await evaluate(`dispatchEvent(new Event('blur')); Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' }); Object.defineProperty(document, 'hidden', { configurable: true, value: true }); document.dispatchEvent(new Event('visibilitychange'));`);
    await evaluate(`seen(['blur', 'hidden'], ${leaving})`);
    await evaluate(`(async () => {
      await playback.context.suspend(); playback.sink.pause(); document.querySelector('#speaker').pause();
    })()`);
    assert.equal(await evaluate('document.visibilityState'), 'hidden');
    await evaluate('silent()');
    const returning = await evaluate('events.length');
    await call('Target.activateTarget', { targetId });
    if (mobile) await evaluate(`delete document.visibilityState; delete document.hidden; document.dispatchEvent(new Event('visibilitychange')); dispatchEvent(new Event('focus'));`);
    await evaluate(`seen(['visible', 'focus'], ${returning})`);
    assert.deepEqual(await evaluate('[playback.speaker.paused, playback.sink.paused]'), [false, false], 'returning did not resume playback');
    await evaluate('audible()');
    const state = await evaluate(`({ events, failures, state: playback.context.state, muted: document.querySelector('#speaker').muted, volume: document.querySelector('#speaker').volume, paused: document.querySelector('#speaker').paused, decoderPaused: playback.sink.paused, target: document.querySelector('a').target })`);
    for (const event of ['blur', 'hidden', 'visible', 'focus']) assert.ok(state.events.includes(event), JSON.stringify(state.events));
    assert.deepEqual(state.failures, []);
    assert.equal(state.target, '');
    assert.equal(state.paused, false, JSON.stringify(state));
    assert.equal(state.decoderPaused, false);
    assert.equal(state.muted, false);
    assert.ok(state.volume > 0);
    t.diagnostic(JSON.stringify(state));
    for (const event of ['visibilitychange', 'focus', 'pageshow']) {
      await evaluate(`(async () => {
        await playback.context.suspend(); playback.sink.pause(); playback.speaker.pause();
        await silent();
        ${event === 'visibilitychange' ? 'document' : 'window'}.dispatchEvent(new Event('${event}'));
        if (playback.speaker.paused || playback.sink.paused) throw new Error('${event} did not resume playback');
        await audible();
      })()`);
      assert.equal(await evaluate('playback.speaker.paused'), false, event);
    }
    await evaluate(`(async () => {
      playback.speaker.pause();
      const play = playback.speaker.play.bind(playback.speaker);
      let refused = false;
      playback.speaker.play = () => { playback.speaker.play = play; refused = true; return Promise.reject(new DOMException('Gesture required', 'NotAllowedError')); };
      dispatchEvent(new Event('focus'));
      if (!refused) throw new Error('focus did not retry playback');
    })()`);
    assert.deepEqual(await evaluate('failures'), []);
    assert.equal(await evaluate('playback.speaker.paused'), true);
    await evaluate("dispatchEvent(new Event('pointerdown'))");
    assert.equal(await evaluate('playback.speaker.paused'), false);
    await evaluate('window.decoder = playback.sink; playback.close()');
    assert.deepEqual(await evaluate('({ paused: decoder.paused, source: decoder.srcObject, muted: decoder.muted })'), { paused: true, source: null, muted: true });
    await call('Target.activateTarget', { targetId: cover.targetId });
    await call('Target.activateTarget', { targetId });
    assert.equal(await evaluate('playback.context.state'), 'closed');
    assert.deepEqual(await evaluate('failures'), []);
  } finally {
    await chrome?.close();
    await new Promise(resolve => server.close(resolve));
  }
});
