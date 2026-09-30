export const AUDIO_DIAGNOSTICS_KEY = 'voice.audio-diagnostics.v1';

export class AudioDiagnostics {
  constructor(storage, now = Date.now) {
    this.storage = storage;
    this.now = now;
    try { this.rows = JSON.parse(storage.getItem(AUDIO_DIAGNOSTICS_KEY)) || []; }
    catch { this.rows = []; }
    if (!Array.isArray(this.rows)) this.rows = [];
  }
  record(kind, state = {}) {
    const row = { at: this.now(), kind };
    for (const key of ['state', 'muted', 'enabled', 'visibility', 'discarded', 'packetsSent', 'packetsReceived', 'bytesSent', 'bytesReceived', 'totalAudioEnergy', 'totalSamplesDuration', 'currentTime']) {
      if (['string', 'number', 'boolean'].includes(typeof state[key])) row[key] = state[key];
    }
    this.rows.push(row);
    this.rows = this.rows.slice(-300);
    try { this.storage.setItem(AUDIO_DIAGNOSTICS_KEY, JSON.stringify(this.rows)); } catch {}
  }
  track(track) {
    const snapshot = () => this.record('microphone', { state: track.readyState, muted: track.muted, enabled: track.enabled });
    for (const event of ['mute', 'unmute', 'ended']) track.addEventListener(event, snapshot);
    snapshot();
  }
  context(context, name) {
    const snapshot = () => this.record(name, { state: context.state, currentTime: context.currentTime });
    context.addEventListener('statechange', snapshot);
    snapshot();
  }
  async stats(pc) {
    try {
      for (const stat of (await pc.getStats()).values()) {
        if ((stat.kind === 'audio' || stat.mediaType === 'audio') && ['outbound-rtp', 'inbound-rtp', 'media-source'].includes(stat.type)) this.record(stat.type, stat);
      }
    } catch {}
  }
  peer(pc) {
    const snapshot = () => this.record('peer', { state: pc.connectionState });
    pc.addEventListener('connectionstatechange', snapshot);
    snapshot();
    const timer = setInterval(() => {
      if (pc.connectionState === 'closed') clearInterval(timer);
      else this.stats(pc);
    }, 5000);
  }
}

let diagnostics;
export function audioDiagnostics() {
  if (diagnostics || typeof document === 'undefined') return diagnostics;
  let storage;
  try { storage = localStorage; } catch { storage = { getItem() {}, setItem() {} }; }
  diagnostics = new AudioDiagnostics(storage);
  const snapshot = (event) => diagnostics.record(event.type, { visibility: document.visibilityState, discarded: Boolean(document.wasDiscarded) });
  for (const event of ['visibilitychange', 'freeze', 'resume']) document.addEventListener(event, snapshot);
  for (const event of ['pagehide', 'pageshow']) window.addEventListener(event, snapshot);
  snapshot({ type: 'loaded' });
  return diagnostics;
}
