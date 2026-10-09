import { readFile } from 'node:fs/promises';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(fileURLToPath(new URL('../docs/', import.meta.url)));
const types = { '.html': 'text/html', '.js': 'text/javascript', '.json': 'application/json', '.wasm': 'application/wasm', '.css': 'text/css', '.svg': 'image/svg+xml' };

export function developmentConfig(token = process.env.VOICE_DEV_TOKEN?.trim()) {
  if (!token) throw new Error('set VOICE_DEV_TOKEN to the development server\'s `voice-web token`');
  const { endpoint_id, relay_url, secret } = JSON.parse(Buffer.from(token, 'base64url'));
  if (!endpoint_id || !relay_url || !secret) throw new Error('development backend is not ready');
  return { token, storageKey: `voice.dev.${endpoint_id}`, serviceWorker: false, transferTimeout: 120000 };
}

export async function serveDevelopment({ config, port = 5173 } = {}) {
  config ??= developmentConfig();
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
      const body = await readFile(file);
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
