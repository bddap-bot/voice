export function formatElapsed(seconds) {
  const whole = Math.max(0, Math.floor(seconds));
  return `${String(Math.floor(whole / 60)).padStart(2, '0')}:${String(whole % 60).padStart(2, '0')}`;
}

export class SessionClock {
  constructor({ capSeconds, now = () => Date.now(), every = setInterval, cancel = clearInterval, onTick, onCap }) {
    this.capSeconds = capSeconds;
    this.now = now;
    this.every = every;
    this.cancel = cancel;
    this.onTick = onTick;
    this.onCap = onCap;
    this.started = null;
    this.timer = null;
  }
  start() {
    this.stop();
    this.started = this.now();
    this.onTick(0);
    this.timer = this.every(() => this.tick(), 250);
  }
  tick() {
    if (this.started === null) return;
    const elapsed = (this.now() - this.started) / 1000;
    this.onTick(elapsed);
    if (elapsed >= this.capSeconds) {
      this.stop();
      this.onCap();
    }
  }
  stop() {
    if (this.timer !== null) this.cancel(this.timer);
    this.timer = null;
    this.started = null;
  }
}

export class ConversationTrace {
  constructor(onChange = () => {}) {
    this.onChange = onChange;
    this.entries = [];
    this.pendingTurns = [];
    this.pendingEntries = new Set();
    this.heardAt = 0;
    this.nextSpeechSource = 'model alone';
    this.forceNewSpeech = false;
  }
  heard(delta, now = Date.now()) {
    if (!this.pendingTurns.length) this.heardAt = now;
    let entry = this.entries.at(-1);
    if (entry?.kind !== 'heard') {
      entry = { kind: 'heard', text: '' };
      this.entries.push(entry);
      this.pendingEntries.add(entry);
      this.pendingTurns.push('');
    }
    entry.text += delta;
    this.pendingTurns[this.pendingTurns.length - 1] += delta;
    this.onChange(this.entries);
  }
  delegated(id, now = Date.now()) {
    const sent = this.pendingTurns.map((text) => text.trim()).filter(Boolean).join('\n');
    const context = this.entries.filter((item) => item.kind !== 'delegation' && !this.pendingEntries.has(item)).slice(-20).map((item) => ({ speaker: item.kind === 'heard' ? 'owner' : 'live', text: item.text }));
    const entry = { kind: 'delegation', id, sent, context, reply: '', timing: null, duration_ms: this.heardAt ? now - this.heardAt : 0 };
    this.entries.push(entry);
    this.pendingTurns = [];
    this.pendingEntries.clear();
    this.heardAt = 0;
    this.onChange(this.entries);
    return entry;
  }
  hub(id, reply, timing) {
    const entry = this.entries.find((item) => item.kind === 'delegation' && item.id === id);
    if (!entry) return false;
    entry.reply = reply;
    entry.timing = timing;
    this.nextSpeechSource = 'model via hub';
    this.forceNewSpeech = true;
    this.onChange(this.entries);
    return true;
  }
  failed(id, message) {
    const entry = this.entries.find((item) => item.kind === 'delegation' && item.id === id);
    if (!entry) return false;
    entry.reply = message;
    entry.failed = true;
    this.onChange(this.entries);
    return true;
  }
  cancel() {
    for (const entry of this.entries) {
      if (entry.kind === 'delegation' && !entry.reply) {
        entry.reply = 'cancelled';
        entry.failed = true;
      }
    }
    this.onChange(this.entries);
  }
  spoke(delta) {
    let entry = this.entries.at(-1);
    if (entry?.kind !== 'spoken' || this.forceNewSpeech) {
      entry = { kind: 'spoken', source: this.nextSpeechSource, text: '' };
      this.entries.push(entry);
      this.nextSpeechSource = 'model alone';
      this.forceNewSpeech = false;
    }
    entry.text += delta;
    this.onChange(this.entries);
    return entry;
  }
  completeSpeech() {
    const entry = this.entries.at(-1);
    if (entry?.kind !== 'spoken' || entry.source !== 'model alone') return;
    this.pendingTurns = [];
    this.pendingEntries.clear();
    this.heardAt = 0;
  }
}
