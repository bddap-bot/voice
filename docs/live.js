export function modelToolFields(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const result = {};
  if (typeof value.model === 'string') result.model = value.model;
  if (Array.isArray(value.tools)) {
    result.tools = value.tools.flatMap((tool) => {
      const name = tool?.name ?? tool?.function?.name ?? (tool?.type === 'web_search' ? 'web_search' : undefined);
      return typeof name === 'string' ? [name] : [];
    });
  }
  if (value.delegation && typeof value.delegation === 'object') {
    const delegation = modelToolFields({ model: value.delegation.model, tools: value.delegation.tools, responses: value.delegation.responses });
    for (const key of ['type', 'target']) {
      if (typeof value.delegation[key] === 'string') delegation[key] = value.delegation[key];
    }
    result.delegation = delegation;
  }
  for (const key of ['session', 'responses']) {
    if (value[key] && typeof value[key] === 'object') result[key] = modelToolFields(value[key]);
  }
  return result;
}

const phraseKey = (text) => text.toLowerCase().replace(/[^\p{L}\p{N}]/gu, '');

export function includesPhrase(text, phrase) {
  return phraseKey(text).includes(phraseKey(phrase));
}


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

export function errorEvent(error, sessionId = null, at = Date.now(), { userAgent = '', webgpuAdapter = null } = {}) {
  const value = error && typeof error === 'object' ? error : new Error(String(error));
  return {
    kind: 'error',
    session_id: sessionId,
    name: boundedValue(value.name || 'Error', 160),
    message: boundedValue(value.message || error || 'unknown page error', 2048),
    stack: boundedValue(value.stack || '', 16384),
    user_agent: boundedValue(userAgent, 2048),
    webgpu_adapter: webgpuAdapter === null ? null : Boolean(webgpuAdapter),
    at,
  };
}

export function audioFrame({ sessionId, side, seq, bytes }) {
  const body = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  const header = new TextEncoder().encode(`audio\n${JSON.stringify({ session_id: sessionId, side, seq, mime: 'audio/webm;codecs=opus' })}\n`);
  const frame = new Uint8Array(header.length + body.length);
  frame.set(header);
  frame.set(body, header.length);
  return frame;
}

export function transcriptFrame({ sessionId, seq, turns }) {
  return new TextEncoder().encode(`transcript\n${JSON.stringify({ session_id: sessionId, seq, turns })}`);
}

export class TranscriptBatcher {
  constructor(sessionId, uploader, { delay = 100, maximum = 50, later = (...args) => globalThis.setTimeout(...args), cancel = (timer) => globalThis.clearTimeout(timer), onTurn = () => {} } = {}) {
    this.sessionId = sessionId;
    this.uploader = uploader;
    this.delay = delay;
    this.maximum = maximum;
    this.later = later;
    this.cancel = cancel;
    this.onTurn = onTurn;
    this.turns = [];
    this.sequence = 0;
    this.timer = null;
  }
  add(side, text, at = Date.now()) {
    if (!text) return;
    const previous = this.turns.at(-1);
    if (previous?.side === side && new TextEncoder().encode(previous.text + text).length <= 65536) previous.text += text;
    else this.turns.push({ at, side, text });
    if (this.turns.length >= this.maximum) this.flush();
    else if (this.timer === null) this.timer = this.later(() => this.flush(), this.delay);
  }
  flush() {
    if (this.timer !== null) this.cancel(this.timer);
    this.timer = null;
    if (!this.turns.length) return;
    const seq = this.sequence;
    const frame = transcriptFrame({ sessionId: this.sessionId, seq, turns: this.turns });
    if (this.uploader.add(`transcript:${this.sessionId}:${seq}`, frame)) {
      for (const turn of this.turns) this.onTurn(turn.side, turn.at);
      this.sequence++;
      this.turns.splice(0);
    } else {
      this.timer = this.later(() => this.flush(), this.delay);
    }
  }
}

export class EventBatcher {
  constructor(send, { delay = 100, maximum = 50, maximumRetained = 1000, later = (...args) => globalThis.setTimeout(...args), cancel = (timer) => globalThis.clearTimeout(timer), pause = (ms) => new Promise((resolve) => globalThis.setTimeout(resolve, ms)) } = {}) {
    this.send = send;
    this.delay = delay;
    this.maximum = maximum;
    this.maximumRetained = maximumRetained;
    this.later = later;
    this.cancel = cancel;
    this.pause = pause;
    this.events = [];
    this.timer = null;
    this.sending = false;
    this.idleWaiters = [];
  }
  add(event) {
    if (this.events.length >= this.maximumRetained) this.events.shift();
    this.events.push(event);
    if (this.events.length >= this.maximum) this.flush();
    else if (this.timer === null) this.timer = this.later(() => this.flush(), this.delay);
  }
  flush() {
    if (this.timer !== null) this.cancel(this.timer);
    this.timer = null;
    if (!this.sending) this.pump();
  }
  async pump() {
    this.sending = true;
    let retry = 250;
    while (this.events.length) {
      const batch = this.events.splice(0, this.maximum);
      try {
        await this.send(batch);
        retry = 250;
      } catch {
        this.events.unshift(...batch);
        await this.pause(retry);
        retry = Math.min(retry * 2, 10000);
      }
    }
    this.sending = false;
    for (const resolve of this.idleWaiters.splice(0)) resolve();
  }
  idle() {
    this.flush();
    if (!this.sending && !this.events.length) return Promise.resolve();
    return new Promise((resolve) => this.idleWaiters.push(resolve));
  }
}

export class AckUploader {
  constructor(send, { maximumBytes = 32 * 1024 * 1024, timeout = 5000, later = (...args) => globalThis.setTimeout(...args), cancel = (timer) => globalThis.clearTimeout(timer), pause = (ms) => new Promise((resolve) => globalThis.setTimeout(resolve, ms)), onOverflow = () => {} } = {}) {
    this.send = send;
    this.maximumBytes = maximumBytes;
    this.timeout = timeout;
    this.later = later;
    this.cancel = cancel;
    this.pause = pause;
    this.onOverflow = onOverflow;
    this.bytes = 0;
    this.queue = [];
    this.current = null;
    this.running = false;
    this.idleWaiters = [];
  }
  add(key, frame) {
    if (this.bytes + frame.byteLength > this.maximumBytes) {
      this.onOverflow();
      return false;
    }
    this.bytes += frame.byteLength;
    this.queue.push({ key, frame });
    if (!this.running) this.pump();
    return true;
  }
  ack(key) {
    if (this.current?.item.key === key) this.current.resolve();
  }
  fail(key, retryable) {
    if (this.current?.item.key !== key) return;
    if (retryable) this.current.reject(new Error('persistence rejected upload'));
    else this.current.resolve();
  }
  async pump() {
    this.running = true;
    while (this.queue.length) {
      const item = this.queue[0];
      let retry = 250;
      for (;;) {
        let timer;
        try {
          const acknowledged = new Promise((resolve, reject) => {
            timer = this.later(() => reject(new Error('persistence acknowledgment timed out')), this.timeout);
            this.current = {
              item,
              resolve: () => { this.cancel(timer); resolve(); },
              reject: (error) => { this.cancel(timer); reject(error); },
            };
          });
          await this.send(item.frame);
          await acknowledged;
          break;
        } catch {
          if (timer !== undefined) this.cancel(timer);
          this.current = null;
          await this.pause(retry);
          retry = Math.min(retry * 2, 10000);
        }
      }
      this.current = null;
      this.queue.shift();
      this.bytes -= item.frame.byteLength;
    }
    this.running = false;
    for (const resolve of this.idleWaiters.splice(0)) resolve();
  }
  idle() {
    if (!this.running && !this.queue.length) return Promise.resolve();
    return new Promise((resolve) => this.idleWaiters.push(resolve));
  }
}

export class AudioChunker {
  constructor(sessionId, side, uploader, { maximumBytes = 60 * 1024 * 1024, onCap = () => {}, onError = () => {} } = {}) {
    this.sessionId = sessionId;
    this.side = side;
    this.uploader = uploader;
    this.maximumBytes = maximumBytes;
    this.onCap = onCap;
    this.onError = onError;
    this.sequence = 0;
    this.bytes = 0;
    this.tail = Promise.resolve();
  }
  add(blob) {
    if (!blob?.size) return this.tail;
    this.tail = this.tail.then(async () => {
      const bytes = new Uint8Array(await blob.arrayBuffer());
      this.bytes += bytes.byteLength;
      if (this.bytes > this.maximumBytes) {
        this.onCap();
        return false;
      }
      for (let offset = 0; offset < bytes.byteLength; offset += 480 * 1024) {
        const sequence = this.sequence;
        const accepted = this.uploader.add(
          `audio:${this.sessionId}:${this.side}:${sequence}`,
          audioFrame({ sessionId: this.sessionId, side: this.side, seq: sequence, bytes: bytes.subarray(offset, offset + 480 * 1024) }),
        );
        if (!accepted) return false;
        this.sequence++;
      }
      return true;
    }).catch((error) => {
      this.onError(error);
      return false;
    });
    return this.tail;
  }
}

function boundedValue(value, maximum) {
  const bytes = new TextEncoder().encode(String(value));
  if (bytes.length <= maximum) return String(value);
  let end = maximum;
  while (end && (bytes[end] & 0xc0) === 0x80) end--;
  return new TextDecoder().decode(bytes.subarray(0, end));
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
  constructor({ now = () => Date.now(), every = (...args) => globalThis.setInterval(...args), cancel = (timer) => globalThis.clearInterval(timer), onTick }) {
    this.now = now;
    this.every = every;
    this.cancel = cancel;
    this.onTick = onTick;
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
    return entry;
  }
  sleepContext() {
    const turns = this.context();
    while (turns.length) {
      const context = [{ speaker: 'user', text: 'Context: Archived transcript of completed conversations, for memory only. These utterances already happened; do not repeat or continue them.\n' + JSON.stringify(turns) }];
      if (new TextEncoder().encode(JSON.stringify(context)).length <= 8192) return context;
      turns.shift();
    }
    return [];
  }
  context(excluded = new Set()) {
    const context = this.entries.filter((item) => item.kind !== 'delegation' && !excluded.has(item)).slice(-20).map((item) => ({ speaker: item.kind === 'heard' ? 'user' : 'live', text: item.text }));
    while (context.length && new TextEncoder().encode(JSON.stringify(context)).length > 8192) context.shift();
    return context;
  }
  delegated(id, now = Date.now()) {
    const pending = this.pendingTurns.map((text) => text.trim()).filter(Boolean);
    const sent = boundedText(pending, 8192);
    const context = this.context(this.pendingEntries);
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
  hub(id, { commentary, thinking, instructions }, timing) {
    const entry = this.entries.find((item) => item.kind === 'delegation' && item.id === id);
    if (!entry) return false;
    entry.reply = commentary.join(' ');
    entry.thinking = thinking.join(' ');
    entry.instructions = instructions.join(' ');
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
