export function formatElapsed(seconds) {
  const whole = Math.max(0, Math.floor(seconds));
  return `${String(Math.floor(whole / 60)).padStart(2, '0')}:${String(whole % 60).padStart(2, '0')}`;
}

export function isOfferFor(waiter, payload) {
  return Boolean(waiter && (waiter.id ?? waiter.offerId) === (payload.offer_id ?? payload.id));
}

export function formatStartError(error) {
  const name = error?.name || 'Error';
  const message = error?.message || String(error);
  const summary = message.startsWith(`${name}:`) ? message : `${name}: ${message}`;
  const headers = new Set([name, message, `${name}: ${message}`]);
  const frame = String(error?.stack ?? '').split('\n').map((line) => line.trim()).find((line) => line && !headers.has(line));
  return frame ? `${summary} — ${frame}` : summary;
}

export function shareFrame({ id, text, mime = null, image = new Uint8Array() }) {
  const encoder = new TextEncoder();
  const cleanText = String(text ?? '');
  const bytes = image instanceof Uint8Array ? image : new Uint8Array(image);
  if (!cleanText.trim() && !bytes.length) throw new Error('add text or an image');
  if (encoder.encode(cleanText).length > 8192) throw new Error('text is too large');
  if (bytes.length > 8 * 1024 * 1024) throw new Error('image is too large');
  if (bytes.length && !['image/png', 'image/jpeg', 'image/gif', 'image/webp'].includes(mime)) throw new Error('choose a PNG, JPEG, GIF, or WebP image');
  const header = encoder.encode(`share\n${JSON.stringify({ id, text: cleanText, mime: bytes.length ? mime : null })}\n`);
  const frame = new Uint8Array(header.length + bytes.length);
  frame.set(header);
  frame.set(bytes, header.length);
  return frame;
}

function boundedText(parts, maximum) {
  const encoder = new TextEncoder();
  while (parts.length > 1 && encoder.encode(parts.join('\n')).length > maximum) parts.shift();
  const bytes = encoder.encode(parts.join('\n'));
  if (bytes.length <= maximum) return new TextDecoder().decode(bytes);
  let start = bytes.length - maximum;
  while ((bytes[start] & 0xc0) === 0x80) start++;
  return new TextDecoder().decode(bytes.subarray(start));
}

export class SessionClock {
  constructor({ capSeconds, now = () => Date.now(), every = (...args) => globalThis.setInterval(...args), cancel = (timer) => globalThis.clearInterval(timer), onTick, onCap }) {
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
    this.activeSpeechSource = null;
    this.lastOutputEnd = null;
    this.forceNewSpeech = false;
  }
  heard(delta, now = Date.now(), startMs = null) {
    if (this.activeSpeechSource === 'model after hub reply' && Number.isFinite(startMs) && Number.isFinite(this.lastOutputEnd) && startMs >= this.lastOutputEnd) this.activeSpeechSource = null;
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
    const pending = this.pendingTurns.map((text) => text.trim()).filter(Boolean);
    const sent = boundedText(pending, 8192);
    const context = this.entries.filter((item) => item.kind !== 'delegation' && !this.pendingEntries.has(item)).slice(-20).map((item) => ({ speaker: item.kind === 'heard' ? 'user' : 'live', text: item.text }));
    while (context.length && new TextEncoder().encode(JSON.stringify(context)).length > 8192) context.shift();
    const entry = { kind: 'delegation', id, sent, context, reply: '', timing: null, duration_ms: this.heardAt ? now - this.heardAt : 0 };
    this.entries.push(entry);
    this.pendingTurns = [];
    this.pendingEntries.clear();
    this.heardAt = 0;
    this.onChange(this.entries);
    return entry;
  }
  shared(id, sent) {
    const entry = { kind: 'delegation', id, sent, context: [], reply: '', timing: null, shared: true };
    this.entries.push(entry);
    this.onChange(this.entries);
    return entry;
  }
  hub(id, reply, timing) {
    const entry = this.entries.find((item) => item.kind === 'delegation' && item.id === id);
    if (!entry) return false;
    entry.reply = reply;
    entry.timing = timing;
    this.nextSpeechSource = 'model after hub reply';
    this.activeSpeechSource = null;
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
      if (entry.kind === 'delegation' && !entry.shared && !entry.reply) {
        entry.reply = 'cancelled';
        entry.failed = true;
      }
    }
    this.onChange(this.entries);
  }
  spoke(delta, startMs = null, endMs = null) {
    let entry = this.entries.at(-1);
    if (entry?.kind !== 'spoken' || this.forceNewSpeech) {
      this.activeSpeechSource ??= this.nextSpeechSource;
      entry = { kind: 'spoken', source: this.activeSpeechSource, text: '' };
      this.entries.push(entry);
      this.nextSpeechSource = 'model alone';
      this.forceNewSpeech = false;
    }
    entry.text += delta;
    if (Number.isFinite(endMs)) this.lastOutputEnd = Math.max(this.lastOutputEnd ?? endMs, endMs);
    this.onChange(this.entries);
    return entry;
  }
}
