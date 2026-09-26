import { execFile, spawn } from 'node:child_process';
import { access, mkdtemp, readdir, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { setTimeout } from 'node:timers/promises';

const execute = promisify(execFile);

export async function chromiumExecutable() {
  if (process.env.CHROMIUM_BIN) return process.env.CHROMIUM_BIN;
  const candidates = ['chromium', 'chromium-browser', 'google-chrome'];
  try { candidates.push(...(await readdir('/nix/store')).filter(name => name.includes('-chromium-')).map(name => `/nix/store/${name}/bin/chromium`)); } catch {}
  for (const candidate of candidates) for (const location of candidate.includes('/') ? [candidate] : (process.env.PATH ?? '').split(':').map(directory => join(directory, candidate))) {
    try { await access(location, constants.X_OK); return location; } catch {}
  }
  throw new Error('headless Chromium is required; set CHROMIUM_BIN');
}

export async function launchChromium({ executable, args = [], prefix = '.chromium-' } = {}) {
  executable ??= await chromiumExecutable();
  const scratch = await mkdtemp(join(process.cwd(), prefix));
  const chrome = spawn(executable, [...args, `--user-data-dir=${scratch}`], { detached: true, stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '', failure;
  chrome.stderr.on('data', chunk => { stderr = (stderr + chunk).slice(-500); });
  chrome.on('error', error => { failure = error; });
  const exited = new Promise(resolve => chrome.once('close', (code, signal) => {
    resolve(failure ?? new Error(`Chromium exited with ${signal ?? `code ${code}`}: ${stderr}`));
  }));
  let closing;
  const close = () => closing ??= (async () => {
    if (chrome.pid) {
      try { process.kill(-chrome.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
    }
    await exited;
    if (chrome.pid) {
      const deadline = Date.now() + 10000;
      while (true) {
        const { stdout } = await execute('ps', ['-eo', 'pgid=,stat=']);
        const running = stdout.trim().split('\n').some(line => {
          const [group, state] = line.trim().split(/\s+/);
          return Number(group) === chrome.pid && !state.startsWith('Z');
        });
        if (!running) break;
        if (Date.now() >= deadline) throw new Error('Chromium process group did not exit; profile retained');
        await setTimeout(20);
      }
    }
    await rm(scratch, { recursive: true, force: true });
  })();
  return { scratch, exited, close, get stderr() { return stderr; } };
}
