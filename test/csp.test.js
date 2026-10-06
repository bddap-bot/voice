import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchChromium } from '../scripts/chromium.mjs';
import { CHUNK } from '../docs/wake.js';

const docs = fileURLToPath(new URL('../docs/', import.meta.url));
const index = await readFile(join(docs, 'index.html'), 'utf8');
const policies = [...index.matchAll(/<meta http-equiv="Content-Security-Policy" content="([^"]*)">/g)].map(([, content]) => content);
const policy = policies[0] ?? '';
const directives = new Map(policy.split(';').map((part) => part.trim().split(/\s+/)).filter(([name]) => name).map(([name, ...sources]) => [name, sources]));

const SPECIFIERS = [
  /<script\b[^>]*\bsrc\s*=\s*["']([^"']+)/g,
  /\bimport\s*\(\s*["'`]([^"'`]+)/g,
  /\b(?:from|import)\s*["']([^"']+)["']/g,
  /\bimportScripts\s*\(\s*["'`]([^"'`]+)/g,
  /\bnew\s+(?:Shared)?Worker\s*\(\s*["'`]([^"'`]+)/g,
];
const foreign = (specifier) => /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(specifier);
const CONNECT = ["'self'", 'blob:', 'https://huggingface.co', 'https://*.hf.co', 'https://*.iroh.link', 'wss://*.iroh.link', 'https://*.iroh.link.', 'wss://*.iroh.link.'];
const allowed = (url) => {
  const { protocol, hostname } = new URL(url);
  return CONNECT.some((source) => {
    const [scheme, host] = source.split('://');
    return host && `${scheme}:` === protocol && (host.startsWith('*.') ? hostname.endsWith(host.slice(1)) : hostname === host);
  });
};

test('every script the built page loads comes from its own origin', async () => {
  const files = (await readdir(docs, { recursive: true })).filter((file) => /\.(?:html|m?js)$/.test(file));
  assert.ok(files.includes(join('lib', 'ort.js')) && files.includes(join('lib', 'transformers.js')), 'build output is missing; run npm run build');
  const offenders = [];
  for (const file of files) {
    const source = await readFile(join(docs, file), 'utf8');
    for (const pattern of SPECIFIERS) for (const [, specifier] of source.matchAll(pattern)) if (foreign(specifier)) offenders.push(`${file}: ${specifier}`);
  }
  assert.deepEqual(offenders, []);
});

test('every absolute URL in the hand-written page is one its policy lets it reach', async () => {
  const files = (await readdir(docs)).filter((file) => /\.(?:html|js)$/.test(file) && file !== 'puppet.js');
  const unreachable = [];
  for (const file of files) {
    const source = (await readFile(join(docs, file), 'utf8')).replace(/<meta http-equiv="Content-Security-Policy"[^>]*>/g, '');
    for (const [url] of source.matchAll(/\b(?:https?|wss?):\/\/[^\s'"`<>)]+/g)) if (!allowed(url)) unreachable.push(`${file}: ${url}`);
  }
  assert.deepEqual(unreachable, []);
});

test('the page policy confines scripts to its origin and names every connection it makes', () => {
  assert.equal(policies.length, 1);
  assert.deepEqual(directives.get('script-src'), ["'self'", "'wasm-unsafe-eval'"]);
  assert.deepEqual(directives.get('object-src'), ["'none'"]);
  assert.deepEqual(directives.get('base-uri'), ["'none'"]);
  assert.deepEqual(directives.get('connect-src'), CONNECT);
  assert.doesNotMatch(index, /<script(?![^>]*\bsrc=)[^>]*>/, 'an inline script cannot run under this policy');
});

test('the vendored inference runtime loads and runs in a window under the page policy', async () => {
  const requests = [];
  const probe = `
const violations = [];
document.addEventListener('securitypolicyviolation', (event) => violations.push(event.violatedDirective + ' ' + event.blockedURI));
let result;
try {
  const ort = await import('./lib/ort.js');
  const mel = await ort.InferenceSession.create('./wake/melspectrogram.onnx');
  const { output } = await mel.run({ input: new ort.Tensor('float32', new Float32Array(${CHUNK + 480}), [1, ${CHUNK + 480}]) });
  const { env } = await import('./lib/transformers.js');
  result = { mel: output.data.length > 0, wasm: env.backends.onnx.wasm.wasmPaths.wasm, violations };
} catch (error) {
  result = { error: String(error?.stack ?? error), violations };
}
document.body.dataset.result = JSON.stringify(result);
`;
  const server = createServer(async (request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    requests.push(path);
    if (path === '/') return response.writeHead(200, { 'content-type': 'text/html' }).end(`<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${policy}"></head><body><script type="module" src="/probe.js"></script></body></html>`);
    if (path === '/probe.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(probe);
    const body = await readFile(join(docs, path)).catch(() => null);
    if (!body) return response.writeHead(404).end();
    response.writeHead(200, { 'content-type': path.endsWith('.wasm') ? 'application/wasm' : /\.m?js$/.test(path) ? 'text/javascript' : 'application/octet-stream' }).end(body);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const chrome = await launchChromium({ args: ['--headless=new', '--no-sandbox', '--disable-gpu'] });
  try {
    const { call } = chrome.devtools;
    const { targetId } = await call('Target.createTarget', { url: `${origin}/` });
    const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
    let result = null;
    for (const deadline = Date.now() + 60000; !result && Date.now() < deadline; await new Promise((resolve) => setTimeout(resolve, 200))) {
      const { result: { value } } = await call('Runtime.evaluate', { expression: 'document.body?.dataset.result ?? null', returnByValue: true }, sessionId);
      result = value && JSON.parse(value);
    }
    assert.match(result?.wasm ?? '', new RegExp(`^${origin}/lib/ort-wasm-simd-threaded\\.jsep-[A-Z0-9]{8}\\.wasm$`), JSON.stringify(result) + chrome.stderr);
    assert.deepEqual(result, { mel: true, wasm: result.wasm, violations: [] });
    assert.ok(requests.includes(new URL(result.wasm).pathname), requests.join(' '));
  } finally {
    await chrome.close();
    server.close();
  }
});
