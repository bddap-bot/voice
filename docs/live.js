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
    this.hearing = '';
    this.heardAt = 0;
    this.speaking = null;
  }
  heard(delta, now = Date.now()) {
    if (!this.hearing) this.heardAt = now;
    this.speaking = null;
    this.hearing += delta;
    this.onChange(this.entries);
  }
  delegated(id, now = Date.now()) {
    const exact = this.hearing.trim();
    const entry = { id, heard: exact, decision: 'delegated', sent: exact, reply: '', timing: null, said: '' };
    entry.duration_ms = this.heardAt ? now - this.heardAt : 0;
    this.entries.push(entry);
    this.speaking = entry;
    this.hearing = '';
    this.heardAt = 0;
    this.onChange(this.entries);
    return entry;
  }
  hub(id, reply, timing) {
    const entry = this.entries.find((item) => item.id === id);
    if (!entry) return false;
    entry.reply = reply;
    entry.timing = timing;
    this.onChange(this.entries);
    return true;
  }
  spoke(delta) {
    let entry = this.speaking;
    if (!entry) entry = [...this.entries].reverse().find((item) => item.decision === 'delegated' && !item.said);
    if (!entry) {
      entry = this.entries.at(-1);
      if (!entry || entry.decision !== 'model alone' || entry.said) {
        entry = { heard: this.hearing.trim(), decision: 'model alone', sent: '', reply: '', timing: null, said: '' };
        this.entries.push(entry);
        this.hearing = '';
        this.heardAt = 0;
      }
    }
    this.speaking = entry;
    entry.said += delta;
    this.onChange(this.entries);
    return entry;
  }
}
