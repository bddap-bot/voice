import assert from 'node:assert/strict';
import test from 'node:test';
import { gzipSync } from 'node:zlib';
import { PuppetChannel } from '../docs/puppet-client.js';

globalThis.location = new URL('https://example.test/voice/');
const encoder = new TextEncoder();
const puppetFrame = (verb, body = '') => encoder.encode(`${verb}\n${body}`);

function cacheStorage() {
  const entries = new Map();
  return {
    entries,
    async open() {
      return {
        async match(request) { return entries.get(request.url)?.clone(); },
        async put(request, response) { entries.set(request.url, response.clone()); },
      };
    },
    async delete() { entries.clear(); return true; },
  };
}

function binaryFrame(id, bytes) {
  const prefix = new TextEncoder().encode(`puppet-chunk\n${id}\n`);
  const frame = new Uint8Array(prefix.length + bytes.length);
  frame.set(prefix);
  frame.set(bytes, prefix.length);
  return frame;
}

function clipBinaryFrame(id, bytes) {
  const prefix = new TextEncoder().encode(`clip-chunk\n${id}\n`);
  const frame = new Uint8Array(prefix.length + bytes.length);
  frame.set(prefix);
  frame.set(bytes, prefix.length);
  return frame;
}

async function deliverClip(channel, entry, original) {
  const compressed = gzipSync(original);
  await channel.receive(puppetFrame('clip-start', JSON.stringify({ id: entry.name, size: compressed.length, originalSize: original.length, contentHash: entry.contentHash, encoding: 'gzip' })));
  await channel.receive(clipBinaryFrame(entry.name, compressed));
  await channel.receive(puppetFrame('clip-end', entry.name));
}

async function deliverPuppet(channel, id, original, contentHash = '') {
  const compressed = gzipSync(original);
  await channel.receive(puppetFrame('puppet-start', JSON.stringify({ id, size: compressed.length, originalSize: original.length, contentHash, encoding: 'gzip' })));
  await channel.receive(binaryFrame(id, compressed));
  await channel.receive(puppetFrame('puppet-end', id));
}

test('private puppet bytes are checked, cached, and selected separately', async () => {
  const sent = [];
  const cache = cacheStorage();
  const channel = new PuppetChannel(async (value) => sent.push(value), cache);
  const catalogPromise = channel.catalog();
  assert.deepEqual(sent, ['puppets']);
  await channel.receive(puppetFrame('puppets', JSON.stringify({ active: '42', avatars: [{ id: '42' }] })));
  assert.deepEqual(await catalogPromise, { active: '42', avatars: [{ id: '42' }] });
  const bytesPromise = channel.bytes('42', 'hash-42');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent.at(-1), 'puppet\n{"id":"42","encodings":["gzip"]}');
  const compressed = gzipSync(Uint8Array.from([0, 128, 255, 1, 2, 3]));
  await channel.receive(puppetFrame('puppet-start', JSON.stringify({ id: '42', size: compressed.length, originalSize: 6, contentHash: 'hash-42', encoding: 'gzip' })));
  await channel.receive(binaryFrame('42', compressed));
  await channel.receive(puppetFrame('puppet-end', '42'));
  assert.deepEqual(new Uint8Array(await bytesPromise), Uint8Array.from([0, 128, 255, 1, 2, 3]));
  await new Promise((resolve) => setImmediate(resolve));
  const before = sent.length;
  assert.deepEqual(new Uint8Array(await channel.bytes('42', 'hash-42')), Uint8Array.from([0, 128, 255, 1, 2, 3]));
  assert.equal(sent.length, before);
});

test('private puppet caches are isolated by authenticated endpoint', async () => {
  const cache = cacheStorage();
  let scope = 'first';
  const channel = new PuppetChannel(async () => {}, cache, () => scope);
  const first = channel.bytes('42');
  await new Promise((resolve) => setImmediate(resolve));
  await deliverPuppet(channel, '42', Uint8Array.of(1));
  assert.deepEqual(new Uint8Array(await first), Uint8Array.of(1));
  await new Promise((resolve) => setImmediate(resolve));
  scope = 'second';
  const second = channel.bytes('42');
  await new Promise((resolve) => setImmediate(resolve));
  await deliverPuppet(channel, '42', Uint8Array.of(2));
  assert.deepEqual(new Uint8Array(await second), Uint8Array.of(2));
  assert.equal(cache.entries.size, 2);
});

test('clips reuse the puppet cache by content hash and changed content refetches', async () => {
  const sent = [];
  const cache = cacheStorage();
  const channel = new PuppetChannel(async (value) => sent.push(value), cache, () => 'scope');
  const firstEntry = { name: 'idle.fbx', format: 'fbx', contentHash: 'idle-a' };
  const first = channel.clipBytes(firstEntry);
  await new Promise((resolve) => setImmediate(resolve));
  await deliverClip(channel, firstEntry, Uint8Array.of(1, 2));
  assert.deepEqual(new Uint8Array(await first), Uint8Array.of(1, 2));
  await new Promise((resolve) => setImmediate(resolve));
  const before = sent.length;
  assert.deepEqual(new Uint8Array(await channel.clipBytes(firstEntry)), Uint8Array.of(1, 2));
  assert.equal(sent.length, before);
  const changedEntry = { ...firstEntry, contentHash: 'idle-b' };
  const changed = channel.clipBytes(changedEntry);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent.at(-1), 'clip\n{"id":"idle.fbx","encodings":["gzip"]}');
  await deliverClip(channel, changedEntry, Uint8Array.of(3));
  assert.deepEqual(new Uint8Array(await changed), Uint8Array.of(3));
  assert.equal(cache.entries.size, 2);
});

test('an incomplete transfer is rejected and never cached', async () => {
  const cache = cacheStorage();
  const channel = new PuppetChannel(async () => {}, cache);
  const result = channel.bytes('7');
  await new Promise((resolve) => setImmediate(resolve));
  const partial = gzipSync(Uint8Array.from([1, 2, 3]));
  await channel.receive(puppetFrame('puppet-start', JSON.stringify({ id: '7', size: partial.length + 1, originalSize: 3, contentHash: '', encoding: 'gzip' })));
  await channel.receive(binaryFrame('7', partial));
  await channel.receive(puppetFrame('puppet-end', '7'));
  await assert.rejects(result, /incomplete puppet transfer/);
  assert.equal(cache.entries.size, 0);
});

test('a stalled mobile transfer reports a timeout and releases the request', async () => {
  const channel = new PuppetChannel(async () => {}, cacheStorage(), () => '', 10);
  await assert.rejects(channel.bytes('7'), /puppet transfer timed out/);
  await assert.rejects(channel.bytes('7'), /puppet transfer timed out/);
});

test('a puppet request queued during preload runs before the next preload item', async () => {
  const sent = [];
  const channel = new PuppetChannel(async (value) => sent.push(value), cacheStorage());
  const preload = channel.preload([
    { id: '1', contentHash: 'hash-1' },
    { id: '2', contentHash: 'hash-2' },
    { id: '4', contentHash: 'hash-4' },
  ], '1');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent.at(-1), 'puppet\n{"id":"2","encodings":["gzip"]}');
  const selected = channel.bytes('3', 'hash-3');
  await deliverPuppet(channel, '2', Uint8Array.of(2), 'hash-2');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent.at(-1), 'puppet\n{"id":"3","encodings":["gzip"]}');
  await deliverPuppet(channel, '3', Uint8Array.of(3), 'hash-3');
  assert.deepEqual(new Uint8Array(await selected), Uint8Array.of(3));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent.at(-1), 'puppet\n{"id":"4","encodings":["gzip"]}');
  await deliverPuppet(channel, '4', Uint8Array.of(4), 'hash-4');
  await preload;
});

const tick = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test('a slow transfer that keeps arriving outlives the stall timeout', async () => {
  const cache = cacheStorage();
  const channel = new PuppetChannel(async () => {}, cache, () => '', 20);
  const original = Uint8Array.from({ length: 4096 }, (_, i) => i % 251);
  const compressed = gzipSync(original);
  const result = channel.bytes('7');
  await tick(0);
  await channel.receive(puppetFrame('puppet-start', JSON.stringify({ id: '7', size: compressed.length, originalSize: original.length, contentHash: '', encoding: 'gzip' })));
  const pieces = 5;
  const step = Math.ceil(compressed.length / pieces);
  for (let at = 0; at < compressed.length; at += step) {
    await tick(12);
    await channel.receive(binaryFrame('7', compressed.subarray(at, at + step)));
  }
  await channel.receive(puppetFrame('puppet-end', '7'));
  assert.deepEqual(new Uint8Array(await result), original);
  assert.equal(cache.entries.size, 1);
});

test('frames of a transfer the client gave up on are ignored and the next request proceeds', async () => {
  const sent = [];
  const cache = cacheStorage();
  const channel = new PuppetChannel(async (value) => sent.push(value), cache, () => '', 10);
  await assert.rejects(channel.bytes('7'), /puppet transfer timed out/);
  const late = gzipSync(Uint8Array.of(9));
  assert.equal(await channel.receive(puppetFrame('puppet-start', JSON.stringify({ id: '7', size: late.length, originalSize: 1, contentHash: '', encoding: 'gzip' }))), true);
  assert.equal(await channel.receive(binaryFrame('7', late)), true);
  assert.equal(await channel.receive(puppetFrame('puppet-end', '7')), true);
  const next = channel.bytes('8');
  await tick(0);
  assert.equal(sent.at(-1), 'puppet\n{"id":"8","encodings":["gzip"]}');
  await channel.receive(puppetFrame('puppet-error', JSON.stringify({ id: '7', code: 'failed', message: 'stale' })));
  await deliverPuppet(channel, '8', Uint8Array.of(8));
  assert.deepEqual(new Uint8Array(await next), Uint8Array.of(8));
  assert.equal(cache.entries.size, 1);
});

test('an oversize transfer fails only itself, not the connection', async () => {
  const channel = new PuppetChannel(async () => {}, cacheStorage());
  const result = channel.bytes('7');
  await tick(0);
  await channel.receive(puppetFrame('puppet-start', JSON.stringify({ id: '7', size: 1, originalSize: 1, contentHash: '', encoding: 'gzip' })));
  assert.equal(await channel.receive(binaryFrame('7', Uint8Array.of(1, 2))), true);
  await assert.rejects(result, /puppet exceeds advertised size/);
  assert.equal(await channel.receive(binaryFrame('7', Uint8Array.of(3))), true);
});

test('selection acknowledges the requested puppet and rejects overlap', async () => {
  const sent = [];
  const channel = new PuppetChannel(async (value) => sent.push(value), cacheStorage());
  const first = channel.select('1');
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(sent, ['puppet-select\n{"id":"1"}']);
  await assert.rejects(channel.select('2'), /another puppet selection is pending/);
  await channel.receive(puppetFrame('puppet-selected', '{"id":"1"}'));
  assert.equal(await first, '1');
});

test('failed sends release each request slot', async () => {
  const channel = new PuppetChannel(async () => { throw new Error('offline'); }, cacheStorage());
  await assert.rejects(channel.catalog(), /offline/);
  await assert.rejects(channel.catalog(), /offline/);
  await assert.rejects(channel.bytes('8'), /offline/);
  await assert.rejects(channel.bytes('8'), /offline/);
  await assert.rejects(channel.select('8'), /offline/);
  await assert.rejects(channel.select('8'), /offline/);
});

test('a cache quota failure still returns the verified download', async () => {
  const cache = { open: async () => ({ match: async () => null, put: async () => { throw new Error('quota'); } }) };
  const sent = [];
  const channel = new PuppetChannel(async (value) => sent.push(value), cache);
  const result = channel.bytes('8');
  await new Promise((resolve) => setImmediate(resolve));
  await deliverPuppet(channel, '8', Uint8Array.of(9));
  assert.deepEqual(new Uint8Array(await result), Uint8Array.of(9));
});

test('connection replacement cancels a download before its delayed cache lookup can send', async () => {
  let release;
  const delayed = new Promise((resolve) => { release = resolve; });
  const sent = [];
  const cache = { open: async () => ({ match: async () => { await delayed; return null; } }) };
  const channel = new PuppetChannel(async (value) => sent.push(value), cache);
  const result = channel.bytes('42');
  await new Promise((resolve) => setImmediate(resolve));
  channel.fail(new Error('connection replaced'));
  release();
  await assert.rejects(result, /connection replaced/);
  assert.deepEqual(sent, []);
});

test('connection replacement cancels a transfer waiting in the queue', async () => {
  const sent = [];
  const channel = new PuppetChannel(async (value) => sent.push(value), cacheStorage());
  const first = channel.bytes('1');
  const second = channel.bytes('2');
  await new Promise((resolve) => setImmediate(resolve));
  channel.fail(new Error('connection replaced'));
  await assert.rejects(first, /connection replaced/);
  await assert.rejects(second, /connection replaced/);
  assert.deepEqual(sent, ['puppet\n{"id":"1","encodings":["gzip"]}']);
});
