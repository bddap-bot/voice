import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import test from 'node:test';
import { launchChromium, renderDom } from '../scripts/chromium.mjs';

test('the puppet stage preserves hips world XZ throughout an idle cycle', { timeout: 30000 }, async () => {
  const bundle = await readFile(new URL('../docs/puppet.js', import.meta.url));
  const server = createServer((request, response) => {
    if (request.url === '/puppet.js') return response.writeHead(200, { 'content-type': 'text/javascript' }).end(bundle);
    response.writeHead(200, { 'content-type': 'text/html' }).end(`<canvas style="width:320px;height:240px"></canvas><script type="module">
      import { PuppetRuntime } from '/puppet.js';
      const runtime = new PuppetRuntime(document.querySelector('canvas'));
      runtime.pause();
      const hips = runtime.gazeTarget.clone();
      hips.position.set(0.125, 1, -0.25);
      runtime.idleRoot.add(hips);
      const anchor = hips.getWorldPosition(hips.position.clone());
      let drift = 0;
      for (let frame = 0; frame < 360; frame++) {
        runtime.updateBasePose(1 / 60);
        const point = hips.getWorldPosition(hips.position.clone());
        drift = Math.max(drift, Math.hypot(point.x - anchor.x, point.z - anchor.z));
      }
      runtime.dispose();
      document.body.dataset.drift = drift;
    </script>`);
  });
  let chrome;
  try {
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    chrome = await launchChromium({ args: ['--headless=new', '--no-sandbox', '--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader'] });
    const html = await renderDom(chrome, `http://127.0.0.1:${server.address().port}/`, { budget: 2000 });
    const drift = html.match(/data-drift="([^"]+)"/);
    assert.ok(drift, html);
    assert.ok(Number(drift[1]) < 1e-7, `hips world XZ drift: ${drift[1]}`);
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    await chrome?.close();
  }
});
