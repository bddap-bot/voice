import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import test from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
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

async function launchFake(browser) {
  const chrome = await launchChromium({ executable: process.execPath, args: ['-e', browser, '--'] });
  let ended = false;
  chrome.exited.then(() => { ended = true; });
  while (!chrome.stderr.includes('\n')) {
    if (ended) throw await chrome.exited;
    await delay(10);
  }
  return chrome;
}

test('Chromium cleanup waits for every process holding the browser output, even outside its session, then removes its scratch directory', { timeout: 30000 }, async () => {
  const writer = `
    const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
    const browser = process.ppid, temporary = path.join(os.tmpdir(), 'writer');
    fs.writeFileSync(temporary, 'active');
    let file = 0;
    setInterval(() => { try { fs.writeFileSync(process.argv[1] + '/' + file++, 'active'); } catch {} }, 1);
    process.stderr.write(process.pid + ' ' + temporary + '\\n');
    const watch = setInterval(() => { if (process.ppid !== browser) { clearInterval(watch); process.stderr.write('orphaned\\n'); } }, 5);
  `;
  const chrome = await launchFake(`
    require('node:child_process').spawn(process.execPath, ['-e', ${JSON.stringify(writer)}, process.argv[1].split('=')[1]], { detached: true, stdio: ['ignore', 'ignore', 'inherit'] });
    setInterval(() => {}, 1000);
  `);
  let child;
  try {
    const [, pid, temporary] = /^(\d+) (.+)$/m.exec(chrome.stderr);
    child = Number(pid);
    const closing = chrome.close();
    while (!chrome.stderr.includes('orphaned')) await delay(10);
    assert.equal(await Promise.race([closing.then(() => 'closed', () => 'closed'), delay(1000, 'waiting')]), 'waiting', 'cleanup finished while the writer still ran');
    process.kill(child, 'SIGKILL');
    child = undefined;
    await closing;
    for (const path of [chrome.scratch, temporary]) await assert.rejects(access(path), { code: 'ENOENT' });
  } finally {
    if (child) process.kill(child, 'SIGKILL');
    await chrome.close();
  }
});

test('Chromium shares the launcher process group, so an interrupt reaches it', async () => {
  const chrome = await launchFake(`process.stderr.write(process.pid + '\\n'); setInterval(() => {}, 1000);`);
  const group = async pid => Number((await execute('ps', ['-o', 'pgid=', '-p', String(pid)])).stdout);
  try {
    assert.equal(await group(Number(chrome.stderr)), await group(process.pid));
  } finally {
    await chrome.close();
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
