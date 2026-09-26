const UTTERANCE_GAP_MS = 2000;
const hex = (bytes) => Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) => byte.toString(16).padStart(2, '0')).join('');
const nanos = (ms) => (BigInt(Math.round(ms * 1000)) * 1000n).toString();

function newTurn(start) {
  const turn = { traceId: hex(16) };
  turn.root = { turn, name: 'turn', spanId: hex(8), start, attributes: {} };
  return turn;
}

function openSpan(parent, name, start, attributes = {}) {
  return { turn: parent.turn, name, parentSpanId: parent.spanId, spanId: hex(8), start, attributes };
}

function closeSpan(span, end, failure = null) {
  return {
    traceId: span.turn.traceId,
    spanId: span.spanId,
    ...(span.parentSpanId ? { parentSpanId: span.parentSpanId } : {}),
    name: span.name,
    startTimeUnixNano: nanos(span.start),
    endTimeUnixNano: nanos(Math.max(span.start, end)),
    attributes: Object.entries(span.attributes).map(([key, value]) => ({ key, value: { stringValue: String(value).slice(0, 256) } })),
    ...(failure ? { status: { code: 2, message: failure } } : {}),
  };
}

export class StageTrace {
  constructor(send, now = () => Date.now()) {
    this.send = send;
    this.now = now;
    this.listening = null;
    this.delegations = new Map();
    this.awaitingSpeech = [];
  }
  inputTranscript() {
    const at = this.now();
    if (!this.listening || this.listening.answered || at - this.listening.last > UTTERANCE_GAP_MS) {
      for (const waiting of this.awaitingSpeech.splice(0)) this.send(closeSpan(waiting.turn.root, waiting.at));
      this.listening = { turn: newTurn(at), first: at, answered: false };
    }
    this.listening.last = at;
  }
  delegationCreated(id) {
    const at = this.now();
    const listening = this.listening;
    this.listening = null;
    const turn = listening?.turn ?? newTurn(at);
    if (listening) {
      this.send(closeSpan(openSpan(turn.root, 'hear', listening.first), listening.last));
      this.send(closeSpan(openSpan(turn.root, 'decide', listening.last), at));
    }
    const span = openSpan(turn.root, 'delegate', at);
    this.delegations.set(id, span);
    return `00-${turn.traceId}-${span.spanId}-01`;
  }
  hubReplied(id, failure = null) {
    const span = this.delegations.get(id);
    if (!span) return;
    this.delegations.delete(id);
    const at = this.now();
    this.send(closeSpan(span, at, failure));
    if (failure) this.send(closeSpan(span.turn.root, at));
    else this.awaitingSpeech.push({ turn: span.turn, at });
  }
  outputTranscript(delay = 0) {
    const waiting = this.awaitingSpeech.shift();
    if (!waiting) {
      if (this.listening) this.listening.answered = true;
      return;
    }
    const heard = this.now() + (Number.isFinite(delay) ? delay : 0);
    this.send(closeSpan(openSpan(waiting.turn.root, 'await-speech', waiting.at), heard));
    this.send(closeSpan(waiting.turn.root, heard));
  }
  sessionEnded() {
    const at = this.now();
    for (const span of this.delegations.values()) {
      this.send(closeSpan(span, at, 'cancelled'));
      this.send(closeSpan(span.turn.root, at));
    }
    for (const waiting of this.awaitingSpeech) this.send(closeSpan(waiting.turn.root, waiting.at));
    this.listening = null;
    this.delegations.clear();
    this.awaitingSpeech = [];
  }
}
