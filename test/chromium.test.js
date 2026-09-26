import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';
import { launchChromium } from '../scripts/chromium.mjs';

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
