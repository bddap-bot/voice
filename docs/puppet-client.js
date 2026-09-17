const decoder = new TextDecoder();

function line(bytes, start = 0) {
  const end = bytes.indexOf(10, start);
  return end < 0 ? [decoder.decode(bytes.subarray(start)), bytes.length] : [decoder.decode(bytes.subarray(start, end)), end + 1];
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

export class PuppetChannel {
  constructor(send, cache = caches, cacheScope = () => '', transferTimeout = 30000) {
    this.send = send;
    this.cacheStorage = cache;
    this.cacheScope = cacheScope;
    this.transferTimeout = transferTimeout;
    this.catalogWaiter = null;
    this.clipCatalogWaiter = null;
    this.selectionWaiter = null;
    this.transfer = null;
    this.transferTail = Promise.resolve();
    this.epoch = 0;
  }
  cacheRequest(id) {
    return new Request(new URL(`.private-puppets/${encodeURIComponent(this.cacheScope())}/${encodeURIComponent(id)}.vrm`, location.href));
  }
  async catalog() {
    if (this.catalogWaiter) return this.catalogWaiter.promise;
    const waiter = deferred();
    this.catalogWaiter = waiter;
    try {
      await this.send('puppets');
      return await waiter.promise;
    } finally {
      if (this.catalogWaiter === waiter) this.catalogWaiter = null;
    }
  }
  async select(id) {
    const waiter = { ...deferred(), id };
    if (this.selectionWaiter) throw new Error('another puppet selection is pending');
    this.selectionWaiter = waiter;
    try {
      await this.send(`puppet-select\n${JSON.stringify({ id })}`);
      return await waiter.promise;
    }
    finally { if (this.selectionWaiter === waiter) this.selectionWaiter = null; }
  }
  async clips() {
    if (this.clipCatalogWaiter) return this.clipCatalogWaiter.promise;
    const waiter = deferred();
    this.clipCatalogWaiter = waiter;
    try {
      await this.send('clips');
      return await waiter.promise;
    } finally {
      if (this.clipCatalogWaiter === waiter) this.clipCatalogWaiter = null;
    }
  }
  clipBytes(entry) {
    return this.transferBytes('clip', entry.name, entry.contentHash, entry.format);
  }
  trackBytes(modelHash, entry) {
    return this.transferBytes('track', entry.name, `${modelHash}-${entry.contentHash}`, 'json', { modelHash, clipHash: entry.contentHash });
  }
  async bytes(id, contentHash = '') {
    return this.transferBytes('puppet', id, contentHash, 'vrm');
  }
  transferBytes(kind, id, contentHash, format, fields = {}) {
    const epoch = this.epoch;
    const operation = this.transferTail.then(() => this.runTransfer(kind, id, contentHash, format, fields, epoch));
    this.transferTail = operation.catch(() => {});
    return operation;
  }
  async runTransfer(kind, id, contentHash, format, fields, epoch) {
    const cache = await this.cacheStorage.open('voice-puppets-v2');
    const request = kind === 'puppet' ? this.cacheRequest(contentHash || id) : new Request(new URL(`.private-motion/${encodeURIComponent(this.cacheScope())}/${encodeURIComponent(contentHash || id)}.${format}`, location.href));
    const saved = await cache.match(request);
    if (epoch !== this.epoch) throw new Error('connection replaced');
    if (saved) return saved.arrayBuffer();
    const waiting = deferred();
    const transfer = { kind, id, contentHash, size: null, originalSize: null, encoding: null, total: 0, chunks: [], waiting, cache, request, timer: null };
    transfer.progress = () => {
      clearTimeout(transfer.timer);
      transfer.timer = setTimeout(() => waiting.reject(new Error('puppet transfer timed out')), this.transferTimeout);
    };
    this.transfer = transfer;
    transfer.progress();
    try {
      const encodings = ['br', 'gzip'].filter((encoding) => {
        try { new DecompressionStream(encoding); return true; } catch { return false; }
      });
      await this.send(`${kind}\n${JSON.stringify({ id, encodings, ...fields })}`);
      return await waiting.promise;
    } finally {
      clearTimeout(transfer.timer);
      if (this.transfer === transfer) this.transfer = null;
    }
  }
  abandon(transfer, message) {
    if (this.transfer === transfer) this.transfer = null;
    clearTimeout(transfer.timer);
    transfer.waiting.reject(new Error(message));
    return true;
  }
  current(verb, step, id) {
    const transfer = this.transfer;
    return transfer && verb === `${transfer.kind}-${step}` && id === transfer.id ? transfer : null;
  }
  async receive(raw) {
    const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
    const [verb, offset] = line(bytes);
    if (!verb.startsWith('puppet') && !verb.startsWith('clip') && !verb.startsWith('track')) return false;
    if (verb === 'clips') {
      const waiter = this.clipCatalogWaiter;
      this.clipCatalogWaiter = null;
      waiter?.resolve(JSON.parse(decoder.decode(bytes.subarray(offset))));
      return true;
    }
    if (verb === 'clips-error') {
      const waiter = this.clipCatalogWaiter;
      this.clipCatalogWaiter = null;
      waiter?.reject(new Error(JSON.parse(decoder.decode(bytes.subarray(offset))).message));
      return true;
    }
    if (verb === 'puppets') {
      const waiter = this.catalogWaiter;
      this.catalogWaiter = null;
      waiter?.resolve(JSON.parse(decoder.decode(bytes.subarray(offset))));
      return true;
    }
    if (verb === 'puppets-error') {
      const waiter = this.catalogWaiter;
      this.catalogWaiter = null;
      waiter?.reject(new Error(JSON.parse(decoder.decode(bytes.subarray(offset))).message));
      return true;
    }
    if (verb === 'puppet-selected' || verb === 'puppet-select-error') {
      const value = JSON.parse(decoder.decode(bytes.subarray(offset)));
      if (verb === 'puppet-selected' && value.id === this.selectionWaiter?.id) this.selectionWaiter.resolve(value.id);
      else if (verb === 'puppet-selected') this.selectionWaiter?.reject(new Error('puppet selection did not match'));
      else this.selectionWaiter?.reject(new Error(value.message));
      return true;
    }
    if (verb === 'puppet-start' || verb === 'clip-start' || verb === 'track-start') {
      const value = JSON.parse(decoder.decode(bytes.subarray(offset)));
      const transfer = this.current(verb, 'start', value.id);
      if (!transfer) return true;
      if (transfer.size !== null || !Number.isSafeInteger(value.size) || value.size <= 0 || !Number.isSafeInteger(value.originalSize) || value.originalSize <= 0 || !['br', 'gzip'].includes(value.encoding) || value.contentHash !== transfer.contentHash) return this.abandon(transfer, 'invalid transfer');
      transfer.size = value.size;
      transfer.originalSize = value.originalSize;
      transfer.encoding = value.encoding;
      transfer.progress();
      return true;
    }
    if (verb === 'puppet-chunk' || verb === 'clip-chunk' || verb === 'track-chunk') {
      const [id, body] = line(bytes, offset);
      const transfer = this.current(verb, 'chunk', id);
      if (!transfer) return true;
      if (transfer.size === null) return this.abandon(transfer, 'transfer chunk before start');
      const chunk = bytes.slice(body);
      transfer.total += chunk.length;
      if (transfer.total > transfer.size) return this.abandon(transfer, 'puppet exceeds advertised size');
      transfer.chunks.push(chunk);
      transfer.progress();
      return true;
    }
    if (verb === 'puppet-end' || verb === 'clip-end' || verb === 'track-end') {
      const transfer = this.current(verb, 'end', decoder.decode(bytes.subarray(offset)));
      if (!transfer) return true;
      if (transfer.total !== transfer.size) return this.abandon(transfer, 'incomplete puppet transfer');
      this.transfer = null;
      clearTimeout(transfer.timer);
      const complete = new Uint8Array(transfer.total);
      let at = 0;
      for (const chunk of transfer.chunks) {
        complete.set(chunk, at);
        at += chunk.length;
      }
      const response = new Response(complete).body.pipeThrough(new DecompressionStream(transfer.encoding));
      const decoded = await new Response(response).arrayBuffer();
      if (decoded.byteLength !== transfer.originalSize) return this.abandon(transfer, 'invalid decompressed puppet size');
      transfer.waiting.resolve(decoded);
      transfer.cache.put(transfer.request, new Response(decoded, { headers: { 'content-type': 'model/gltf-binary' } })).catch(() => {});
      return true;
    }
    if (verb === 'puppet-error' || verb === 'clip-error' || verb === 'track-error') {
      const value = JSON.parse(decoder.decode(bytes.subarray(offset)));
      const transfer = this.transfer;
      if (transfer && value.code !== 'busy' && verb === `${transfer.kind}-error` && (!value.id || value.id === transfer.id)) this.abandon(transfer, value.message);
      return true;
    }
    return true;
  }
  fail(error) {
    this.epoch++;
    this.catalogWaiter?.reject(error);
    this.clipCatalogWaiter?.reject(error);
    this.selectionWaiter?.reject(error);
    this.transfer?.waiting.reject(error);
    this.catalogWaiter = null;
    this.clipCatalogWaiter = null;
    this.selectionWaiter = null;
    this.transfer = null;
  }
  clearCache() {
    return this.cacheStorage.delete('voice-puppets-v2');
  }
  async preload(avatars, active) {
    for (const avatar of avatars) {
      if (avatar.id !== active) await this.bytes(avatar.id, avatar.contentHash);
    }
  }
}
