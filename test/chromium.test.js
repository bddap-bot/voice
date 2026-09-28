import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, readdir, readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';
import { launchChromium, renderDom } from '../scripts/chromium.mjs';

const execute = promisify(execFile);

async function running(pid) {
  const { stdout } = await execute('ps', ['-eo', 'pid=,stat=']);
  return stdout.trim().split('\n').some(line => {
    const [id, state] = line.trim().split(/\s+/);
    return Number(id) === pid && !state.startsWith('Z');
  });
}

test('Chromium cleanup waits for an orphaned profile writer after the browser closes', { timeout: 15000 }, async () => {
  const writer = `
    const fs = require('node:fs');
    const profile = process.argv[1];
    fs.writeFileSync(profile + '/child', String(process.pid));
    const timer = setInterval(() => fs.writeFileSync(profile + '/writing', 'active'), 1);
    process.send('ready');
    setTimeout(() => { clearInterval(timer); process.exit(); }, 10000);
  `;
  const parent = `
    const { spawn } = require('node:child_process');
    const profile = process.argv[1].split('=')[1];
    const child = spawn(process.execPath, ['-e', ${JSON.stringify(writer)}, profile], { stdio: ['ignore', 'ignore', 'ignore', 'ipc'] });
    child.once('message', () => process.exit());
  `;
  const browser = await launchChromium({ executable: process.execPath, args: ['-e', parent, '--'] });
  let child;
  try {
    await browser.exited;
    child = Number(await readFile(join(browser.scratch, 'child'), 'utf8'));
    assert.ok(await running(child), 'the child outlives the browser and its stdio');
    await browser.close();
    assert.equal(await running(child), false, 'the child has exited before cleanup returns');
    await assert.rejects(access(browser.scratch), { code: 'ENOENT' });
    await browser.close();
  } finally {
    if (child && await running(child)) process.kill(child, 'SIGKILL');
    await browser.close();
  }
});

test('Chromium cleanup removes the profile when spawning fails', async () => {
  const browser = await launchChromium({ executable: join(process.cwd(), 'missing-chromium') });
  assert.equal((await browser.exited).code, 'ENOENT');
  await browser.close();
  await assert.rejects(access(browser.scratch), { code: 'ENOENT' });
});

async function renderTicking(tick) {
  let chrome, server;
  const processes = [];
  const record = async () => { processes.push(...(await chrome.devtools.call('SystemInfo.getProcessInfo')).processInfo); };
  try {
    server = createServer(async (request, response) => {
      if (request.url !== '/tick') return response.end(`<!doctype html><script>addEventListener('load', () => setInterval(async () => { await fetch('/tick'); document.body.dataset.ticks = (Number(document.body.dataset.ticks) || 0) + 1; }, 1000));</script>`);
      await record().catch(() => {});
      tick(response, processes);
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    chrome = await launchChromium({ args: ['--headless=new', '--no-sandbox'] });
    await record();
    const started = Date.now();
    const stdout = await renderDom(chrome, `http://127.0.0.1:${server.address().port}/`, { budget: 8000, stepLimit: 4000 });
    assert.ok((await readdir(chrome.scratch)).some(name => /^\.?(org\.chromium\.Chromium|com\.google\.Chrome)\./.test(name)), 'Chromium keeps its temporary files in its scratch directory');
    return { stdout, elapsed: Date.now() - started };
  } catch (error) {
    return { error };
  } finally {
    server?.closeAllConnections();
    server?.close();
    if (chrome) {
      await chrome.close();
      await assert.rejects(chrome.devtools.call('Browser.getVersion'), /Chromium closed DevTools/);
      for (const { id } of processes) assert.equal(await running(id), false, `Chromium process ${id} outlives cleanup`);
      await assert.rejects(access(chrome.scratch), { code: 'ENOENT' });
    }
  }
}

test('a slow page that keeps making progress is not cut off', async () => {
  const { stdout, error, elapsed } = await renderTicking(response => setTimeout(() => response.end(), 1000));
  assert.ifError(error);
  assert.ok(elapsed > 4000, `${elapsed} ms`);
  assert.match(stdout, /data-ticks="[78]"/);
});

test('a page stalled on a fetch that never completes fails within the step limit', async () => {
  const { error } = await renderTicking(() => {});
  assert.match(error?.message, /did not advance virtual time past \d+ ms within 4 s/);
});

for (const victim of ['renderer', 'browser']) test(`a page run fails when its ${victim} process dies`, async () => {
  const { error } = await renderTicking((response, processes) => {
    for (const { type, id } of processes) if (type === victim) try { process.kill(id, 'SIGKILL'); } catch {}
    response.end();
  });
  assert.match(error?.message, victim === 'renderer' ? /renderer crashed/ : /browser process ended/);
});
