import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { launchChromium } from '../scripts/chromium.mjs';
import { createServer } from 'node:http';
import { join } from 'node:path';

test('remote WebRTC audio stays silent while held, plays after release, and closes its decoder', { timeout: 30000 }, async () => {
  const server = createServer(async (request, response) => {
    if (request.url === '/') return response.end('<!doctype html><title>Playback</title>');
    try { response.setHeader('content-type', 'text/javascript'); response.end(await readFile(new URL('../docs' + request.url, import.meta.url))); }
    catch { response.writeHead(404).end(); }
  });
  let chrome, socket;
  try {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    chrome = await launchChromium({ prefix: '.playback-', args: ['--headless=new', '--no-sandbox', '--autoplay-policy=no-user-gesture-required', '--remote-debugging-port=0', `http://127.0.0.1:${server.address().port}/`] });
    let port;
    for (let i = 0; i < 100; i++) { try { port = Number((await readFile(join(chrome.scratch, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); break; } catch { await new Promise(r => setTimeout(r, 50)); } }
    assert.ok(port, 'DevTools must start');
    let page;
    for (let i = 0; i < 100; i++) {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      page = targets.find(t => t.type === 'page' && t.title === 'Playback');
      if (page) break;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    assert.ok(page, 'Playback page must load');
    socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
    const result = new Promise(resolve => { socket.onmessage = ({ data }) => { const message = JSON.parse(data); if (message.id === 1) resolve(message); }; });
    socket.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { awaitPromise: true, returnByValue: true, expression: `
      (async () => {
        const { LivePlayback } = await import('/live-playback.js');
        const context = new AudioContext();
        const oscillator = context.createOscillator();
        const destination = context.createMediaStreamDestination();
        oscillator.connect(destination); oscillator.start(); await context.resume();
        const sender = new RTCPeerConnection(), receiver = new RTCPeerConnection();
        sender.onicecandidate = ({candidate}) => { if (candidate) receiver.addIceCandidate(candidate); };
        receiver.onicecandidate = ({candidate}) => { if (candidate) sender.addIceCandidate(candidate); };
        const incoming = new Promise(resolve => receiver.ontrack = e => resolve(e.streams[0]));
        sender.addTrack(destination.stream.getAudioTracks()[0], destination.stream);
        await sender.setLocalDescription(await sender.createOffer());
        await receiver.setRemoteDescription(sender.localDescription);
        await receiver.setLocalDescription(await receiver.createAnswer());
        await sender.setRemoteDescription(receiver.localDescription);
        let failure;
        const playback = new LivePlayback(() => {}, e => failure = e.message);
        playback.hold('gesture');
        const output = await playback.attach(await incoming);
        const analyser = context.createAnalyser();
        context.createMediaStreamSource(output).connect(analyser);
        const peak = async () => {
          let max = 0;
          for (let i = 0; i < 20; i++) {
            await new Promise(resolve => setTimeout(resolve, 50));
            const samples = new Float32Array(analyser.fftSize);
            analyser.getFloatTimeDomainData(samples);
            max = Math.max(max, ...samples.map(Math.abs));
          }
          return max;
        };
        const held = await peak();
        oscillator.stop();
        await new Promise(resolve => setTimeout(resolve, 300));
        playback.release('gesture');
        const released = await peak();
        const sink = playback.sink;
        await playback.close();
        const closed = sink?.paused && sink.srcObject === null;
        sender.close(); receiver.close(); await context.close();
        return { held, released, closed, muted: sink?.muted, failure };
      })()` } }));
    let timer;
    const evaluated = await Promise.race([result, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('WebRTC decoding timed out')), 15000); })]).finally(() => clearTimeout(timer));
    assert.ok(!evaluated.result.exceptionDetails, JSON.stringify(evaluated.result.exceptionDetails));
    const values = evaluated.result.result.value;
    assert.equal(values.failure, undefined);
    assert.equal(values.held, 0);
    assert.ok(values.released > 0.1, JSON.stringify(values));
    assert.equal(values.closed, true);
    assert.equal(values.muted, true);
  } finally {
    socket?.close();
    await chrome?.close();
    await new Promise(resolve => server.close(resolve));
  }
});
