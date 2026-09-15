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
  async bytes(id, contentHash = '') {
    return this.transferBytes('puppet', id, contentHash, 'vrm');
  }
  async transferBytes(kind, id, contentHash, format) {
    const epoch = this.epoch;
    const cache = await this.cacheStorage.open('voice-puppets-v1');
    const request = kind === 'puppet' ? this.cacheRequest(contentHash || id) : new Request(new URL(`.private-motion/${encodeURIComponent(this.cacheScope())}/${encodeURIComponent(contentHash || id)}.${format}`, location.href));
    const saved = await cache.match(request);
    if (epoch !== this.epoch) throw new Error('connection replaced');
    if (saved) return saved.arrayBuffer();
    if (this.transfer) throw new Error('another puppet is loading');
    const waiting = deferred();
    this.transfer = { kind, id, contentHash, size: null, originalSize: null, encoding: null, total: 0, chunks: [], waiting, cache, request };
    const timer = setTimeout(() => waiting.reject(new Error('puppet transfer timed out')), this.transferTimeout);
    try {
      const encodings = ['br', 'gzip'].filter((encoding) => {
        try { new DecompressionStream(encoding); return true; } catch { return false; }
      });
      await this.send(`${kind}\n${JSON.stringify({ id, encodings })}`);
      return await waiting.promise;
    } finally {
      clearTimeout(timer);
      if (this.transfer?.waiting === waiting) this.transfer = null;
    }
  }
  async receive(raw) {
    const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
    const [verb, offset] = line(bytes);
    if (!verb.startsWith('puppet') && !verb.startsWith('clip')) return false;
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
    if (verb === 'puppet-start' || verb === 'clip-start') {
      const value = JSON.parse(decoder.decode(bytes.subarray(offset)));
      if (!this.transfer || verb !== `${this.transfer.kind}-start` || value.id !== this.transfer.id || !Number.isSafeInteger(value.size) || value.size <= 0 || !Number.isSafeInteger(value.originalSize) || value.originalSize <= 0 || !['br', 'gzip'].includes(value.encoding) || value.contentHash !== this.transfer.contentHash) throw new Error('invalid transfer');
      this.transfer.size = value.size;
      this.transfer.originalSize = value.originalSize;
      this.transfer.encoding = value.encoding;
      return true;
    }
    if (verb === 'puppet-chunk' || verb === 'clip-chunk') {
      const [id, body] = line(bytes, offset);
      if (!this.transfer || verb !== `${this.transfer.kind}-chunk` || id !== this.transfer.id || this.transfer.size === null) throw new Error('unexpected transfer chunk');
      const chunk = bytes.slice(body);
      this.transfer.total += chunk.length;
      if (this.transfer.total > this.transfer.size) throw new Error('puppet exceeds advertised size');
      this.transfer.chunks.push(chunk);
      return true;
    }
    if (verb === 'puppet-end' || verb === 'clip-end') {
      const id = decoder.decode(bytes.subarray(offset));
      const transfer = this.transfer;
      this.transfer = null;
      if (!transfer || verb !== `${transfer.kind}-end` || id !== transfer.id || transfer.total !== transfer.size) {
        transfer?.waiting.reject(new Error('incomplete puppet transfer'));
        return true;
      }
      const complete = new Uint8Array(transfer.total);
      let at = 0;
      for (const chunk of transfer.chunks) {
        complete.set(chunk, at);
        at += chunk.length;
      }
      const response = new Response(complete).body.pipeThrough(new DecompressionStream(transfer.encoding));
      const decoded = await new Response(response).arrayBuffer();
      if (decoded.byteLength !== transfer.originalSize) {
        transfer.waiting.reject(new Error('invalid decompressed puppet size'));
        return true;
      }
      transfer.waiting.resolve(decoded);
      transfer.cache.put(transfer.request, new Response(decoded, { headers: { 'content-type': 'model/gltf-binary' } })).catch(() => {});
      return true;
    }
    if (verb === 'puppet-error' || verb === 'clip-error') {
      const value = JSON.parse(decoder.decode(bytes.subarray(offset)));
      if (value.code !== 'busy' && this.transfer && (!value.id || value.id === this.transfer.id)) {
        const transfer = this.transfer;
        this.transfer = null;
        transfer.waiting.reject(new Error(value.message));
      }
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
    return this.cacheStorage.delete('voice-puppets-v1');
  }
  async preload(avatars, active) {
    for (const avatar of avatars) {
      if (avatar.id !== active) await this.bytes(avatar.id, avatar.contentHash);
    }
  }
}
