import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import http from 'node:http';
import { homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const root = path.resolve(fileURLToPath(new URL('../docs/', import.meta.url)));
const types = { '.html': 'text/html', '.js': 'text/javascript', '.json': 'application/json', '.wasm': 'application/wasm', '.css': 'text/css', '.svg': 'image/svg+xml' };

export async function developmentConfig() {
  const state = path.join(homedir(), '.local/state/voice-web-dev');
  const { stdout } = await promisify(execFile)('voice-web', ['token', '--key-file', `${state}/secret.key`, '--secret-file', `${state}/auth.env`, '--relay-file', `${state}/relay-url`]);
  const token = stdout.trim();
  const { endpoint_id, relay_url, secret } = JSON.parse(Buffer.from(token, 'base64url'));
  if (!endpoint_id || !relay_url || !secret) throw new Error('development backend is not ready');
  return { token, storageKey: `voice.dev.${endpoint_id}`, serviceWorker: false };
}

export async function serveDevelopment({ config, port = 5173, wasmRoot = process.env.VOICE_WASM_DIR } = {}) {
  config ??= await developmentConfig();
  const server = http.createServer(async (request, response) => {
    const host = request.headers.host;
    if (host !== `127.0.0.1:${server.address().port}` && host !== `localhost:${server.address().port}`) return response.writeHead(403).end();
    response.setHeader('Cache-Control', 'no-store');
    const url = new URL(request.url, 'http://localhost');
    if (url.pathname === '/config.js') {
      const source = await readFile(path.join(root, 'config.js'), 'utf8');
      return response.writeHead(200, { 'Content-Type': 'text/javascript' }).end(source.replace(/^export default .*;/m, `export default ${JSON.stringify(config)};`));
    }
    if (url.pathname === '/sw.js') return response.writeHead(404).end();
    const relative = decodeURIComponent(url.pathname === '/' ? '/index.html' : url.pathname);
    const file = path.resolve(root, `.${relative}`);
    if (!file.startsWith(`${root}${path.sep}`)) return response.writeHead(403).end();
    try {
      const wasmFile = wasmRoot && ['/botq_dash_wasm.js', '/botq_dash_wasm_bg.wasm'].includes(url.pathname);
      let body = await readFile(wasmFile ? path.join(wasmRoot, path.basename(file)) : file);
      if (wasmRoot && relative === '/index.html') body = body.toString().replace('https://bddap-bot.github.io/botq/botq_dash_wasm.js', '/botq_dash_wasm.js');
      response.writeHead(200, { 'Content-Type': types[path.extname(file)] ?? 'application/octet-stream' }).end(body);
    } catch { response.writeHead(404).end(); }
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve); });
  return { url: `http://127.0.0.1:${server.address().port}/`, token: config.token, close: () => new Promise((resolve) => server.close(resolve)) };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const server = await serveDevelopment();
  console.log(`Development page: ${server.url}`);
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, async () => { await server.close(); });
}
