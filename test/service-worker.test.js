import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import test from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import { launchChromium } from '../scripts/chromium.mjs';

test('a reload after a deploy runs every shell module from the new deploy even while the old ones are still fresh in the HTTP cache', async () => {
  const worker = await readFile(new URL('../docs/sw.js', import.meta.url));
  let version = 'old';
  const files = () => ({
    '/index.html': `<!doctype html><script type="module">import { version } from './live.js'; document.title = version; navigator.serviceWorker.register('sw.js');</script>`,
    '/live.js': `export const version = '${version}';`,
    '/sw.js': worker,
  });
  const server = createServer((request, response) => {
    const path = request.url === '/' ? '/index.html' : new URL(request.url, 'http://localhost').pathname;
    const body = files()[path] ?? `/* ${path} */`;
    const etag = `"${createHash('sha1').update(body).digest('hex')}"`;
    response.setHeader('cache-control', 'max-age=600');
    response.setHeader('etag', etag);
    if (request.headers['if-none-match'] === etag) return response.writeHead(304).end();
    response.writeHead(200, { 'content-type': path.endsWith('.html') ? 'text/html' : 'text/javascript' }).end(body);
  });
  const chrome = await launchChromium({ args: ['--headless=new', '--no-sandbox', '--disable-gpu'] });
  try {
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    const { call } = chrome.devtools;
    const { targetId } = await call('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
    const page = (method, params) => call(method, params, sessionId);
    const evaluate = async (expression) => (await page('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })).result.value;
    const settle = async (check) => {
      for (let tries = 0; tries < 200; tries++) {
        if (await evaluate(check).catch(() => false)) return evaluate('document.title');
        await delay(50);
      }
      throw new Error(`page never satisfied ${check}`);
    };
    await page('Page.navigate', { url: `http://127.0.0.1:${server.address().port}/` });
    assert.equal(await settle("Boolean(navigator.serviceWorker?.controller) && document.title !== ''"), 'old');
    version = 'new';
    await evaluate('window.replaced = true');
    await page('Page.reload', {});
    assert.equal(await settle("!window.replaced && document.title !== ''"), 'new');
  } finally {
    await chrome.close();
    if (server.listening) await new Promise((resolve) => server.close(resolve));
  }
});
