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
    this.tools = new Set();
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
    this.delegations.set(id, { turn, round: null, ready: null });
  }
  responseEvent(envelope) {
    const delegation = this.delegations.get(envelope?.delegation_id);
    const event = envelope?.event;
    if (!delegation || !event) return;
    const at = this.now();
    if (event.type === 'response.created') {
      if (delegation.ready !== null) this.send(closeSpan(openSpan(delegation.turn.root, 'resume', delegation.ready), at));
      delegation.ready = null;
      delegation.round = { span: openSpan(delegation.turn.root, 'respond', at), output: [], thinking: null };
      return;
    }
    const round = delegation.round;
    if (!round) return;
    if (event.type === 'response.output_item.added' && event.item?.type === 'reasoning') round.thinking = openSpan(round.span, 'think', at);
    else if (event.type === 'response.output_item.done' && event.item) {
      if (event.item.type === 'reasoning' && round.thinking) {
        this.send(closeSpan(round.thinking, at));
        round.thinking = null;
      }
      round.output.push(event.item.type === 'function_call' ? event.item.name : event.item.type);
    } else if (event.type === 'response.completed') this.#closeRound(delegation, at);
  }
  async tool(delegationId, item, run) {
    const delegation = this.delegations.get(delegationId);
    if (!delegation) return run(null);
    const entry = { delegation, span: openSpan(delegation.turn.root, 'tool', this.now(), { 'tool.name': item.name, 'tool.arguments': item.arguments ?? '' }) };
    this.tools.add(entry);
    try {
      const result = await run(`00-${delegation.turn.traceId}-${entry.span.spanId}-01`);
      this.#finishTool(entry, result?.ok === false ? 'failed' : null);
      return result;
    } catch (error) {
      this.#finishTool(entry, 'failed');
      throw error;
    }
  }
  delegationSettled(id) {
    const delegation = this.delegations.get(id);
    if (!delegation) return;
    this.delegations.delete(id);
    this.awaitingSpeech.push({ turn: delegation.turn, at: this.now() });
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
    for (const entry of [...this.tools]) this.#finishTool(entry, 'cancelled');
    for (const delegation of this.delegations.values()) {
      if (delegation.round) this.#closeRound(delegation, at);
      this.awaitingSpeech.push({ turn: delegation.turn, at });
    }
    for (const waiting of this.awaitingSpeech) this.send(closeSpan(waiting.turn.root, waiting.at));
    this.listening = null;
    this.delegations.clear();
    this.awaitingSpeech = [];
  }
  #closeRound(delegation, at) {
    const round = delegation.round;
    if (round.thinking) this.send(closeSpan(round.thinking, at));
    round.span.attributes.output = round.output.join(',');
    this.send(closeSpan(round.span, at));
    delegation.round = null;
    delegation.ready = at;
  }
  #finishTool(entry, failure) {
    if (!this.tools.delete(entry)) return;
    const at = this.now();
    this.send(closeSpan(entry.span, at, failure));
    entry.delegation.ready = at;
  }
}
