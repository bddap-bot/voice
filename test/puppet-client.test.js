import assert from 'node:assert/strict';
import test from 'node:test';
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

test('private puppet bytes are checked, cached, and selected separately', async () => {
  const sent = [];
  const cache = cacheStorage();
  const channel = new PuppetChannel(async (value) => sent.push(value), cache);
  const catalogPromise = channel.catalog();
  assert.deepEqual(sent, ['puppets']);
  await channel.receive(puppetFrame('puppets', JSON.stringify({ active: '42', avatars: [{ id: '42' }] })));
  assert.deepEqual(await catalogPromise, { active: '42', avatars: [{ id: '42' }] });
  const bytesPromise = channel.bytes('42');
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(sent.at(-1), 'puppet\n{"id":"42"}');
  await channel.receive(puppetFrame('puppet-start', '{"id":"42","size":6}'));
  await channel.receive(binaryFrame('42', Uint8Array.from([0, 128, 255])));
  await channel.receive(binaryFrame('42', Uint8Array.from([1, 2, 3])));
  await channel.receive(puppetFrame('puppet-end', '42'));
  assert.deepEqual(new Uint8Array(await bytesPromise), Uint8Array.from([0, 128, 255, 1, 2, 3]));
  await new Promise((resolve) => setImmediate(resolve));
  const before = sent.length;
  assert.deepEqual(new Uint8Array(await channel.bytes('42')), Uint8Array.from([0, 128, 255, 1, 2, 3]));
  assert.equal(sent.length, before);
});

test('private puppet caches are isolated by authenticated endpoint', async () => {
  const cache = cacheStorage();
  let scope = 'first';
  const channel = new PuppetChannel(async () => {}, cache, () => scope);
  const first = channel.bytes('42');
  await new Promise((resolve) => setImmediate(resolve));
  await channel.receive(puppetFrame('puppet-start', '{"id":"42","size":1}'));
  await channel.receive(binaryFrame('42', Uint8Array.of(1)));
  await channel.receive(puppetFrame('puppet-end', '42'));
  assert.deepEqual(new Uint8Array(await first), Uint8Array.of(1));
  await new Promise((resolve) => setImmediate(resolve));
  scope = 'second';
  const second = channel.bytes('42');
  await new Promise((resolve) => setImmediate(resolve));
  await channel.receive(puppetFrame('puppet-start', '{"id":"42","size":1}'));
  await channel.receive(binaryFrame('42', Uint8Array.of(2)));
  await channel.receive(puppetFrame('puppet-end', '42'));
  assert.deepEqual(new Uint8Array(await second), Uint8Array.of(2));
  assert.equal(cache.entries.size, 2);
});

test('an incomplete transfer is rejected and never cached', async () => {
  const cache = cacheStorage();
  const channel = new PuppetChannel(async () => {}, cache);
  const result = channel.bytes('7');
  await new Promise((resolve) => setImmediate(resolve));
  await channel.receive(puppetFrame('puppet-start', '{"id":"7","size":4}'));
  await channel.receive(binaryFrame('7', Uint8Array.from([1, 2, 3])));
  await channel.receive(puppetFrame('puppet-end', '7'));
  await assert.rejects(result, /incomplete puppet transfer/);
  assert.equal(cache.entries.size, 0);
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
  await channel.receive(puppetFrame('puppet-start', '{"id":"8","size":1}'));
  await channel.receive(binaryFrame('8', Uint8Array.of(9)));
  await channel.receive(puppetFrame('puppet-end', '8'));
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
