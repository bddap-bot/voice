import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { SIGN_OFF } from '../docs/identity.js';
import { includesPhrase } from '../docs/live.js';
import { spokenBetween } from '../test/wake-reply-measurements.js';
import { rigSpeech, runRig } from './live-rig.mjs';

const { positionals: [outDir, sessionsFile, speechFile], values: { page } } = parseArgs({ allowPositionals: true, options: { page: { type: 'string' } } });
if (!outDir || !sessionsFile) throw new Error('usage: node scripts/dismissal-smoke.mjs OUTPUT SESSIONS_JSON [SPEECH_JSON] [--page URL]');
const sessions = JSON.parse(await readFile(sessionsFile, 'utf8'));
const spoken = Object.fromEntries(sessions.flatMap(({ name, turns, dismissal }) => [...turns.map(({ say }, index) => [`${name}-${index}`, say]), [`${name}-dismissal`, dismissal.say]]));
const speech = await rigSpeech(outDir, speechFile, spoken);

const rows = [];
await runRig({ outDir, speech, page, extra: { sessions: sessions.map(({ name }) => name) } }, async ({ capture, now, say, sleep, mark, evaluate, drain, settle, quietRelay, tap }) => {
  const open = async () => (await evaluate("document.querySelector('#puppet')?.getAttribute('aria-pressed')")) === 'true';
  for (const [ch, { name, standing, turns, dismissal }] of sessions.entries()) {
    await quietRelay();
    await tap(name, ch, true);
    const start = now();
    let failure = null;
    for (const [index, { reply, at }] of turns.entries()) {
      while (now() - start < at) await sleep(200);
      await evaluate('globalThis.__lastHubRequest = null');
      await evaluate(`__say(${JSON.stringify(`${name}-${index}`)})`);
      const asked = now();
      while (!await evaluate('Boolean(globalThis.__lastHubRequest)') && now() - asked < 20000) await sleep(200);
      await evaluate(`__deliverHubReply(${JSON.stringify(reply)}, ${JSON.stringify(`${name}-${index}`)}, ${JSON.stringify(standing)})`);
      try { await settle(ch, now(), 3000, 40000); }
      catch (error) { failure = `ended before the dismissal, after turn ${index}: ${error.message}`; break; }
    }
    let spokenAfter = '';
    let delegated = false;
    if (!failure) {
      while (now() - start < dismissal.at) await sleep(200);
      await evaluate('globalThis.__lastHubRequest = null');
      const since = mark('dismissal', { name });
      await evaluate(`__say(${JSON.stringify(`${name}-dismissal`)})`);
      const deadline = Date.now() + 30000;
      while (await open() && Date.now() < deadline) {
        if (!delegated && await evaluate('Boolean(globalThis.__lastHubRequest)')) {
          delegated = true;
          mark('dismissal-delegated', { name });
          if (dismissal.reply) await evaluate(`__deliverHubReply(${JSON.stringify(dismissal.reply)}, ${JSON.stringify(`${name}-dismissal`)}, ${JSON.stringify(standing)})`);
        }
        await drain(); await sleep(250);
      }
      await drain();
      spokenAfter = spokenBetween(capture, ch, since, now());
      if (await open()) {
        failure = 'the dismissal left the session open';
        await tap(name, ch, false);
      }
      else if (!includesPhrase(spokenAfter, SIGN_OFF)) failure = 'the session closed without the sign-off';
      else if (!includesPhrase(spokenAfter.split(/\s+/).slice(-SIGN_OFF.split(' ').length - 1).join(' '), SIGN_OFF)) failure = 'spoke after the sign-off';
      else await sleep(3000);
    }
    const row = { name, minutes: Math.round((now() - start) / 600) / 100, turns: turns.length, delegated, spokenAfter, failure };
    say('ROW', JSON.stringify(row));
    rows.push(row);
  }
});
const failures = rows.filter((row) => row.failure).map((row) => `${row.name}: ${row.failure}; said ${JSON.stringify(row.spokenAfter)}`);
await writeFile(path.join(outDir, 'verdict.json'), JSON.stringify({ rows, failures }, null, 2));
if (failures.length) { console.log(failures.join('\n')); process.exit(1); }
console.log(`PASS: all ${rows.length} recorded dismissals ended their session through the sign-off`);
