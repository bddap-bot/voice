import assert from 'node:assert/strict';
import http from 'node:http';
import { test } from 'node:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { homedir } from 'node:os';
import config, { configuredToken } from '../docs/config.js';
import { developmentConfig, serveDevelopment } from '../scripts/dev.mjs';

test('deployed config preserves saved and manually entered credentials', () => {
  assert.equal(configuredToken(config, 'deployed-token'), 'deployed-token');
  assert.equal(configuredToken(config, null), null);
  assert.equal(config.storageKey, 'voice.token');
  assert.equal(config.serviceWorker, true);
  assert.equal(config.transferTimeout, 30000);
});

test('development config selects its backend over stale or entered credentials', () => {
  const dev = { token: 'dev-token', storageKey: 'voice.dev.endpoint', serviceWorker: false, transferTimeout: 120000 };
  assert.equal(configuredToken(dev, 'deployed-token'), 'dev-token');
  assert.equal(configuredToken(dev, null), 'dev-token');
});

test('localhost serves the same bundle with private uncached config', async () => {
  const dev = { token: 'dev-token', storageKey: 'voice.dev.endpoint', serviceWorker: false, transferTimeout: 120000 };
  const server = await serveDevelopment({ config: dev, port: 0 });
  try {
    const page = await fetch(server.url);
    assert.equal(await page.text(), await readFile(new URL('../docs/index.html', import.meta.url), 'utf8'));
    const response = await fetch(`${server.url}config.js`);
    assert.equal(response.headers.get('cache-control'), 'no-store');
    const module = await import(`data:text/javascript,${encodeURIComponent(await response.text())}`);
    assert.deepEqual(module.default, dev);
    assert.equal(module.configuredToken(module.default, 'deployed-token'), 'dev-token');
    assert.equal((await fetch(`${server.url}sw.js`)).status, 404);
    const foreignHostStatus = await new Promise((resolve, reject) => { http.get(`${server.url}config.js`, { headers: { Host: 'example.org' } }, (response) => { response.resume(); resolve(response.statusCode); }).on('error', reject); });
    assert.equal(foreignHostStatus, 403);
    assert.equal((await fetch(`${server.url}%2e%2e%2fpackage.json`)).status, 403);
  } finally { await server.close(); }
});

test('page uses config for startup, manual connect, storage and service worker', async () => {
  const page = await readFile(new URL('../docs/index.html', import.meta.url), 'utf8');
  assert.match(page, /const KEY = config.storageKey/);
  assert.match(page, /new PuppetChannel\(sendFrame, caches, .*config.transferTimeout\)/);
  assert.match(page, /async function start\(raw\) \{\s+raw = configuredToken\(config, raw\)/);
  assert.match(page, /const saved = configuredToken\(config, localStorage.getItem\(KEY\)\)/);
  assert.match(page, /if \(config.serviceWorker && 'serviceWorker' in navigator\)/);
});

test('development smoke requires a started Live session and rejects deployed Live mode', async () => {
  const source = await readFile(new URL('../scripts/smoke.mjs', import.meta.url), 'utf8');
  assert.match(source, /const live = development/);
  assert.match(source, /if \(development && deployed\) throw/);
  assert.match(source, /getAttribute\('aria-pressed'\) === 'true'/);
  assert.match(source, /--use-fake-device-for-media-stream/);
  assert.match(source, /--use-fake-ui-for-media-stream/);
});

test('development credential command uses only the isolated instance files', async () => {
  const scratch = await mkdtemp(path.join(process.cwd(), '.config-test-'));
  const previousPath = process.env.PATH;
  const state = path.join(homedir(), '.local/state/voice-web-dev');
  const args = ['token', '--key-file', state + '/secret.key', '--secret-file', state + '/auth.env', '--relay-file', state + '/relay-url'];
  const token = Buffer.from(JSON.stringify({ endpoint_id: 'dev-endpoint', relay_url: 'https://relay.example', secret: 'dev-secret' })).toString('base64url');
  try {
    await writeFile(path.join(scratch, 'voice-web'), '#!' + process.execPath + '\n' + 'if (JSON.stringify(process.argv.slice(2)) !== ' + JSON.stringify(JSON.stringify(args)) + ') process.exit(2); console.log(' + JSON.stringify(token) + ');', { mode: 0o700 });
    process.env.PATH = scratch + path.delimiter + previousPath;
    assert.deepEqual(await developmentConfig(), { token, storageKey: 'voice.dev.dev-endpoint', serviceWorker: false, transferTimeout: 120000 });
    await writeFile(path.join(scratch, 'voice-web'), '#!' + process.execPath + '\nprocess.exit(1);');
    await assert.rejects(developmentConfig());
  } finally { process.env.PATH = previousPath; await rm(scratch, { recursive: true, force: true }); }
});
