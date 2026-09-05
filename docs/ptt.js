export function encodeWav(samples, inRate, outRate = 16000) {
  const ratio = inRate / outRate;
  const n = ratio === 1 ? samples.length : Math.floor(samples.length / ratio);
  const pcm = new Int16Array(n);
  for (let i = 0; i < n; i++) {
    const pos = i * ratio;
    const lo = Math.floor(pos);
    const hi = Math.min(lo + 1, samples.length - 1);
    const s = samples[lo] + (samples[hi] - samples[lo]) * (pos - lo);
    pcm[i] = Math.max(-1, Math.min(1, s)) * 0x7fff;
  }
  const bytes = new Uint8Array(44 + pcm.length * 2);
  const v = new DataView(bytes.buffer);
  const str = (o, s) => { for (let i = 0; i < s.length; i++) bytes[o + i] = s.charCodeAt(i); };
  str(0, 'RIFF'); v.setUint32(4, 36 + pcm.length * 2, true); str(8, 'WAVE');
  str(12, 'fmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, outRate, true); v.setUint32(28, outRate * 2, true);
  v.setUint16(32, 2, true); v.setUint16(34, 16, true);
  str(36, 'data'); v.setUint32(40, pcm.length * 2, true);
  bytes.set(new Uint8Array(pcm.buffer), 44);
  return bytes;
}

function concat(chunks) {
  const out = new Float32Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) { out.set(c, at); at += c.length; }
  return out;
}

const msg = (e) => (e && e.message ? e.message : String(e));

export function createPtt({ mediaDevices, sink, onState = () => {} }) {
  let held = false;
  let stream = null;
  let opened = null;
  let holding = null;
  const stopTracks = (s) => { for (const t of s.getTracks()) t.stop(); };
  const teardown = async () => {
    const s = stream, k = opened;
    stream = null; opened = null;
    stopTracks(s);
    let word;
    try { word = await k.close(); } catch (e) { word = `release failed: ${msg(e)}`; }
    if (word) onState(word);
  };
  return {
    get held() { return held; },
    async hold() {
      if (held) return;
      if (holding) await holding;
      if (held) return;
      held = true;
      holding = (async () => {
        onState('opening mic');
        let s;
        try { s = await mediaDevices.getUserMedia({ audio: true }); }
        catch (e) { held = false; onState(`mic refused: ${msg(e)}`); return; }
        if (!held) { stopTracks(s); return; }
        const k = sink();
        let word;
        try { word = await k.open(s); }
        catch (e) { held = false; stopTracks(s); onState(`hold failed: ${msg(e)}`); return; }
        stream = s; opened = k;
        if (word) onState(word);
        if (!held) await teardown();
      })();
      await holding;
      holding = null;
    },
    async release() {
      if (!held) return;
      held = false;
      if (holding) return holding;
      if (stream) await teardown();
    },
  };
}

export function wavSink({ capture, upload, onState = () => {} }) {
  let cap = null;
  let chunks = [];
  return {
    async open(stream) {
      cap = capture(stream);
      const mine = cap;
      cap.onChunk((c) => { if (cap === mine) chunks.push(c); });
      return 'listening';
    },
    async close() {
      const { rate } = cap;
      cap.close(); cap = null;
      const samples = concat(chunks);
      chunks = [];
      if (!samples.length) return 'nothing captured';
      onState('sending');
      try { await upload(encodeWav(samples, rate)); }
      catch (e) { return `send failed: ${msg(e)}`; }
    },
  };
}

export function describeTrack(track) {
  return track ? `${track.kind || 'track'}:${track.readyState || '?'} enabled=${track.enabled !== false} muted=${track.muted === true}` : 'none';
}

export function trackSink({ sender, silence, send, onState = () => {}, makeMeter = () => null }) {
  let meter = null;
  return {
    async open(stream) {
      const t = stream.getAudioTracks()[0];
      if (!t) throw new Error('no audio track');
      onState(`mic granted: ${describeTrack(t)}`);
      try { meter = makeMeter(stream); }
      catch (e) { onState(`input meter failed: ${msg(e)}`); }
      try {
        await sender.replaceTrack(t);
        onState(`sender hold: ${describeTrack(sender.track || t)}`);
        await send('hold');
      } catch (e) {
        if (meter) meter.close();
        meter = null;
        await sender.replaceTrack(silence).catch(() => {});
        throw e;
      }
      return 'talking';
    },
    async close() {
      if (meter) {
        try { onState(`input peak ${meter.read().toFixed(4)}`); }
        catch (e) { onState(`input meter failed: ${msg(e)}`); }
        meter.close(); meter = null;
      }
      try {
        await sender.replaceTrack(silence);
        onState(`sender release: ${describeTrack(sender.track || silence)}`);
      } catch (e) { onState(`sender release failed: ${msg(e)}`); }
      await send('release');
      return 'listening';
    },
  };
}

export function createNudge({ afterMs = 8000, onNudge, onClear = () => {}, setTimer = setTimeout, clearTimer = clearTimeout }) {
  let timer = null;
  let fired = false;
  let held = false;
  const clear = () => { if (timer !== null) clearTimer(timer); timer = null; };
  return {
    connected() { if (held || fired) return; fired = true; timer = setTimer(() => { timer = null; if (!held) onNudge(); }, afterMs); },
    hold() { held = true; clear(); onClear(); },
    stop() { clear(); onClear(); },
  };
}
