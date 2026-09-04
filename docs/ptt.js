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

export function createLivePtt({ mediaDevices, sender, silence, send, onState = () => {}, quietMs = 1500, setTimer = setTimeout, clearTimer = clearTimeout }) {
  let held = false;
  let track = null;
  let quiet = null;
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
      if (!held) { t.stop(); return; }
      track = t;
      await sender.replaceTrack(t);
      await send('hold');
      onState('live — talking');
    },
    async release() {
      if (!held) return;
      held = false;
      if (!track) return;
      track.stop();
      track = null;
      await sender.replaceTrack(silence);
      await send('release');
      quiet = setTimer(() => { quiet = null; if (!held) sender.replaceTrack(null); }, quietMs);
      onState('live — listening');
    },
  };
}
