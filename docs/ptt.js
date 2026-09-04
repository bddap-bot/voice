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

export function createPtt({ mediaDevices, capture, upload, onState = () => {} }) {
  let held = false;
  let stream = null;
  let cap = null;
  let chunks = [];

  const stopMic = () => {
    if (stream) for (const t of stream.getTracks()) t.stop();
    if (cap) cap.close();
    stream = null;
    cap = null;
  };

  return {
    get held() { return held; },
    async hold() {
      if (held) return;
      held = true;
      onState('opening mic');
      let s;
      try { s = await mediaDevices.getUserMedia({ audio: true }); }
      catch (e) { held = false; onState(`mic refused: ${e && e.message ? e.message : e}`); return; }
      if (!held) { for (const t of s.getTracks()) t.stop(); return; }
      stream = s;
      cap = capture(s);
      cap.onChunk((c) => { if (held) chunks.push(c); });
      onState('listening');
    },
    async release() {
      if (!held) return;
      held = false;
      const rate = cap ? cap.rate : 0;
      stopMic();
      const samples = concat(chunks);
      chunks = [];
      if (!samples.length) { onState('nothing captured'); return; }
      onState('sending');
      try {
        await upload(encodeWav(samples, rate));
        onState('sent');
      } catch (e) {
        onState(`send failed: ${e && e.message ? e.message : e}`);
      }
    },
  };
}

export function describeTrack(track) {
  return track ? `${track.kind || 'track'}:${track.readyState || '?'} enabled=${track.enabled !== false} muted=${track.muted === true}` : 'none';
}

export function createLivePtt({ mediaDevices, sender, silence, send, onState = () => {}, makeMeter = () => null, quietMs = 1500, setTimer = setTimeout, clearTimer = clearTimeout }) {
  let held = false;
  let track = null;
  let quiet = null;
  let meter = null;
  return {
    get held() { return held; },
    async hold() {
      if (held) return;
      held = true;
      clearTimer(quiet); quiet = null;
      onState('opening mic');
      let s;
      try { s = await mediaDevices.getUserMedia({ audio: true }); }
      catch (e) { held = false; onState(`mic refused: ${e && e.message ? e.message : e}`); return; }
      const t = s.getAudioTracks()[0];
      if (!t) { held = false; for (const candidate of s.getTracks()) candidate.stop(); onState('mic granted: no audio track'); return; }
      if (!held) { t.stop(); return; }
      track = t;
      onState(`mic granted: ${describeTrack(t)}`);
      try { meter = makeMeter(s); }
      catch (e) { onState(`input meter failed: ${e && e.message ? e.message : e}`); }
      try { await sender.replaceTrack(t); }
      catch (e) {
        if (meter) meter.close();
        meter = null;
        track.stop();
        track = null;
        held = false;
        onState(`sender hold failed: ${e && e.message ? e.message : e}`);
        return;
      }
      onState(`sender hold: ${describeTrack(sender.track || t)}`);
      await send('hold');
      onState('talking');
    },
    async release() {
      if (!held) return;
      held = false;
      if (!track) return;
      let level = null;
      try { level = meter ? meter.read() : null; }
      catch (e) { onState(`input meter failed: ${e && e.message ? e.message : e}`); }
      if (level !== null) onState(`input peak ${level.toFixed(4)}`);
      if (meter) meter.close();
      meter = null;
      track.stop();
      track = null;
      try {
        await sender.replaceTrack(silence);
        onState(`sender release: ${describeTrack(sender.track || silence)}`);
      } catch (e) { onState(`sender release failed: ${e && e.message ? e.message : e}`); }
      await send('release');
      quiet = setTimer(() => {
        quiet = null;
        if (!held) sender.replaceTrack(null).then(() => onState('sender quiet: none')).catch((e) => onState(`sender quiet failed: ${e && e.message ? e.message : e}`));
      }, quietMs);
      onState('listening');
    },
  };
}

export function createNudge({ afterMs = 8000, onNudge, onClear = () => {}, setTimer = setTimeout, clearTimer = clearTimeout }) {
  let timer = null;
  let held = false;
  return {
    connected() { if (held || timer !== null) return; timer = setTimer(() => { timer = null; if (!held) onNudge(); }, afterMs); },
    hold() { held = true; if (timer !== null) { clearTimer(timer); timer = null; } onClear(); },
    stop() { held = false; if (timer !== null) { clearTimer(timer); timer = null; } onClear(); },
  };
}
