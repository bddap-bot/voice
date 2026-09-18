export class ResponsesTools {
  constructor(execute, send, valid = () => true, submitted = () => {}, settled = () => {}) {
    this.execute = execute;
    this.send = send;
    this.valid = valid;
    this.submitted = submitted;
    this.settled = settled;
    this.responses = new Map();
    this.delegations = new Map();
    this.calls = new Set();
    this.closed = false;
  }
  close() { this.closed = true; }
  active() { return !this.closed && this.valid(); }
  handle(envelope) {
    if (envelope?.type !== 'response.event' || !this.active()) return;
    const event = envelope.event;
    if (event?.type === 'response.created') {
      this.delegations.set(envelope.delegation_id, event.response.id);
      if (!this.responses.has(event.response.id)) this.responses.set(event.response.id, { calls: [], terminal: false, sent: 0, continued: false, delegation: envelope.delegation_id });
      return;
    }
    const id = event?.response_id ?? event?.response?.id ?? this.delegations.get(envelope.delegation_id);
    const response = this.responses.get(id);
    if (!response || response.delegation !== envelope.delegation_id) return;
    const item = event.type === 'response.output_item.done' ? event.item : null;
    if (item?.type === 'function_call' && item.call_id && item.name && !this.calls.has(item.call_id)) {
      this.calls.add(item.call_id);
      let execution;
      try { execution = this.execute(item, envelope.delegation_id); }
      catch { execution = { ok: false, error: 'tool execution failed' }; }
      const result = Promise.resolve(execution).catch(() => ({ ok: false, error: 'tool execution failed' }));
      response.calls.push({ item, result });
    }
    if (event.type === 'response.completed') {
      response.terminal = true;
      return this.complete(response);
    }
  }
  flush() { return Promise.all([...this.responses.values()].filter((response) => response.terminal).map((response) => this.complete(response))); }
  complete(response) {
    if (response.pending) return response.pending;
    response.pending = this.submit(response).finally(() => { response.pending = null; });
    return response.pending;
  }
  async submit(response) {
    const outputs = await Promise.all(response.calls.map(async ({ item, result }) => ({ call_id: item.call_id, output: JSON.stringify(await result) })));
    if (!this.active()) return;
    if (!outputs.length) { this.settled(response.delegation); return; }
    while (response.sent < outputs.length) {
      this.send({ type: 'response.item.create', item: { type: 'function_call_output', ...outputs[response.sent] } });
      response.sent++;
    }
    if (!response.continued) {
      this.send({ type: 'response.create' });
      response.continued = true;
    }
    for (const output of outputs) await this.submitted(output.call_id);
  }
}
