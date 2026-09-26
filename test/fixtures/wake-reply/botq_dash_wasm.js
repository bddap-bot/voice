const mod = await import(globalThis.__wakeReplyRelayModule);
export default mod.default;
export const init = mod.init;
export const connect = mod.connect;
const dec = new TextDecoder();
const enc = new TextEncoder();
const keep = (dir, bytes, verbs, limit, extra = {}) => {
  try {
    const text = dec.decode(bytes.subarray(0, limit));
    const verb = text.split('\n')[0];
    if (verbs.includes(verb)) (globalThis.__frames ??= []).push({ at: Date.now(), dir, verb, text, ...extra });
  } catch {}
};
const queue = [];
let waiting;
const deliver = (value) => { if (waiting) { const resolve = waiting; waiting = null; resolve(value); } else queue.push(value); };
let pumping = false;
export async function recv() {
  if (!pumping) {
    pumping = true;
    (async () => { for (;;) { const value = await mod.recv(); if (value instanceof Uint8Array) keep('in', value, ['answer', 'offer-error', 'hub', 'hub-error', 'error'], 16384); deliver(value); } })();
  }
  return queue.length ? queue.shift() : new Promise((resolve) => { waiting = resolve; });
}
globalThis.__deliverHubReply = (reply, stamp) => {
  const request = globalThis.__lastHubRequest;
  const value = enc.encode('hub\n' + JSON.stringify({ id: request?.id ?? stamp, heard: request?.text ?? '', reply, directives: [], timing_ms: 0, stamp }));
  keep('in', value, ['hub'], 16384);
  deliver(value);
};
export async function send_only(bytes) {
  const outgoing = dec.decode(bytes);
  if (outgoing.startsWith('delegate\n')) {
    keep('out', bytes, ['delegate'], 65536);
    const request = JSON.parse(outgoing.slice(outgoing.indexOf('\n') + 1));
    globalThis.__lastHubRequest = request;
    return;
  }
  if (outgoing.startsWith('hub-ack\n')) return;
  if (bytes instanceof Uint8Array) keep('out', bytes, ['offer', 'delegate', 'cancel'], 65536);
  return mod.send_only(bytes);
}
