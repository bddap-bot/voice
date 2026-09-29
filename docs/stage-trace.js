const UTTERANCE_GAP_MS = 2000;
const hex = (bytes) => Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) => byte.toString(16).padStart(2, '0')).join('');
const nanos = (ms) => (BigInt(Math.round(ms * 1000)) * 1000n).toString();

function newTrace(start, name = 'turn', attributes = {}) {
  const trace = { traceId: hex(16) };
  trace.root = { trace, name, spanId: hex(8), start, attributes };
  return trace;
}

function openSpan(parent, name, start, attributes = {}) {
  return { trace: parent.trace, name, parentSpanId: parent.spanId, spanId: hex(8), start, attributes };
}

function closeSpan(span, end, failure = null) {
  return {
    traceId: span.trace.traceId,
    spanId: span.spanId,
    ...(span.parentSpanId ? { parentSpanId: span.parentSpanId } : {}),
    name: span.name,
    startTimeUnixNano: nanos(span.start),
    endTimeUnixNano: nanos(Math.max(span.start, end)),
    attributes: Object.entries(span.attributes).map(([key, value]) => ({ key, value: { stringValue: String(value).slice(0, 256) } })),
    ...(failure ? { status: { code: 2, message: String(failure).slice(0, 256) } } : {}),
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
    if (!this.listening || this.listening.repliedAt !== undefined || at - this.listening.last > UTTERANCE_GAP_MS) {
      this.closeReply();
      for (const waiting of this.awaitingSpeech.splice(0)) this.send(closeSpan(waiting.trace.root, waiting.at));
      this.listening = { trace: newTrace(at), first: at };
    }
    this.listening.last = at;
  }
  delegationCreated(id) {
    const at = this.now();
    const listening = this.listening;
    this.listening = null;
    const trace = listening?.trace ?? newTrace(at);
    if (listening) {
      this.heard(listening);
      this.send(closeSpan(openSpan(trace.root, 'decide', listening.last), at));
    }
    const span = openSpan(trace.root, 'delegate', at);
    this.delegations.set(id, span);
    return `00-${trace.traceId}-${span.spanId}-01`;
  }
  hubReplied(id, failure = null) {
    const span = this.delegations.get(id);
    if (!span) return;
    this.delegations.delete(id);
    const at = this.now();
    this.send(closeSpan(span, at, failure));
    if (failure) this.send(closeSpan(span.trace.root, at));
    else this.awaitingSpeech.push({ trace: span.trace, at });
  }
  outputTranscript(delay = 0) {
    const heard = this.now() + (Number.isFinite(delay) ? delay : 0);
    const waiting = this.awaitingSpeech.shift();
    if (!waiting) {
      if (this.listening) this.listening.repliedAt ??= heard;
      return;
    }
    this.send(closeSpan(openSpan(waiting.trace.root, 'await-speech', waiting.at), heard));
    this.send(closeSpan(waiting.trace.root, heard));
  }
  heard(listening) {
    this.send(closeSpan(openSpan(listening.trace.root, 'hear', listening.first), listening.last));
  }
  closeReply() {
    const listening = this.listening;
    if (listening?.repliedAt === undefined) return;
    this.heard(listening);
    this.send(closeSpan(openSpan(listening.trace.root, 'reply', listening.last), listening.repliedAt));
    this.send(closeSpan(listening.trace.root, listening.repliedAt));
  }
  sessionEnded() {
    this.closeReply();
    const at = this.now();
    for (const span of this.delegations.values()) {
      this.send(closeSpan(span, at, 'cancelled'));
      this.send(closeSpan(span.trace.root, at));
    }
    for (const waiting of this.awaitingSpeech) this.send(closeSpan(waiting.trace.root, waiting.at));
    this.listening = null;
    this.delegations.clear();
    this.awaitingSpeech = [];
  }
}

export async function traceStages(send, name, attributes, work, now = () => Date.now()) {
  const root = newTrace(now(), name, attributes).root;
  const stage = async (stageName, run, describe = () => ({})) => {
    const span = openSpan(root, stageName, now());
    try {
      const result = await run();
      span.attributes = describe(result);
      send(closeSpan(span, now()));
      return result;
    } catch (error) {
      send(closeSpan(span, now(), error?.message || 'failed'));
      throw error;
    }
  };
  try {
    const result = await work(stage);
    send(closeSpan(root, now()));
    return result;
  } catch (error) {
    send(closeSpan(root, now(), error?.message || 'failed'));
    throw error;
  }
}
