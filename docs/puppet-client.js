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
  constructor(send, cache = caches, cacheScope = () => '') {
    this.send = send;
    this.cacheStorage = cache;
    this.cacheScope = cacheScope;
    this.catalogWaiter = null;
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
  async bytes(id) {
    const epoch = this.epoch;
    const cache = await this.cacheStorage.open('voice-puppets-v1');
    const request = this.cacheRequest(id);
    const saved = await cache.match(request);
    if (epoch !== this.epoch) throw new Error('connection replaced');
    if (saved) return saved.arrayBuffer();
    if (this.transfer) throw new Error('another puppet is loading');
    const waiting = deferred();
    this.transfer = { id, size: null, total: 0, chunks: [], waiting, cache, request };
    try {
      await this.send(`puppet\n${JSON.stringify({ id })}`);
      return await waiting.promise;
    } finally {
      if (this.transfer?.waiting === waiting) this.transfer = null;
    }
  }
  async receive(raw) {
    const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
    const [verb, offset] = line(bytes);
    if (!verb.startsWith('puppet')) return false;
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
    if (verb === 'puppet-start') {
      const value = JSON.parse(decoder.decode(bytes.subarray(offset)));
      if (!this.transfer || value.id !== this.transfer.id || !Number.isSafeInteger(value.size) || value.size <= 0) throw new Error('invalid puppet transfer');
      this.transfer.size = value.size;
      return true;
    }
    if (verb === 'puppet-chunk') {
      const [id, body] = line(bytes, offset);
      if (!this.transfer || id !== this.transfer.id || this.transfer.size === null) throw new Error('unexpected puppet chunk');
      const chunk = bytes.slice(body);
      this.transfer.total += chunk.length;
      if (this.transfer.total > this.transfer.size) throw new Error('puppet exceeds advertised size');
      this.transfer.chunks.push(chunk);
      return true;
    }
    if (verb === 'puppet-end') {
      const id = decoder.decode(bytes.subarray(offset));
      const transfer = this.transfer;
      this.transfer = null;
      if (!transfer || id !== transfer.id || transfer.total !== transfer.size) {
        transfer?.waiting.reject(new Error('incomplete puppet transfer'));
        return true;
      }
      const complete = new Uint8Array(transfer.total);
      let at = 0;
      for (const chunk of transfer.chunks) {
        complete.set(chunk, at);
        at += chunk.length;
      }
      transfer.waiting.resolve(complete.buffer);
      transfer.cache.put(transfer.request, new Response(complete, { headers: { 'content-type': 'model/gltf-binary' } })).catch(() => {});
      return true;
    }
    if (verb === 'puppet-error') {
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
    this.selectionWaiter?.reject(error);
    this.transfer?.waiting.reject(error);
    this.catalogWaiter = null;
    this.selectionWaiter = null;
    this.transfer = null;
  }
  clearCache() {
    return this.cacheStorage.delete('voice-puppets-v1');
  }
}
