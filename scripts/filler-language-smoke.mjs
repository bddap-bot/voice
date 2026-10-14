import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { spokenBetween, spokenReply } from '../test/wake-reply-measurements.js';
import { rigSpeech, runRig } from './live-rig.mjs';

const { positionals: [outDir, speechFile], values: { page, sessions } } = parseArgs({ allowPositionals: true, options: { page: { type: 'string' }, sessions: { type: 'string', default: '4' } } });
if (!outDir || !(Number(sessions) >= 1)) throw new Error('usage: node scripts/filler-language-smoke.mjs OUTPUT [SPEECH_JSON] [--page URL] [--sessions N>=1]');
const FILLER = 'While a hub request is pending, fill the wait as a ditzy anime character speaking only Japanese. Speak everything else in English as usual.';
const SPOKEN = {
  first: 'Ask the hub for the current test beacon status.',
  second: 'Now ask the hub why the beacon keeps dropping its connection.',
  nudge: 'Are you still there? What is taking so long?',
};
const speech = await rigSpeech(outDir, speechFile, SPOKEN);
const runs = Array.from({ length: Number(sessions) }, (_, ch) => ({
  first: 'The test beacon is amber.',
  second: 'The beacon reaches the base station over Bluetooth, and it drops when the beacon sleeps. Keep it awake, keep the station close, and restart the pairing app.',
  ch,
  nudge: ch % 2 === 0,
}));
const japanese = (text) => (text.match(/[\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Han}]/gu) ?? []).length;
const latin = (text) => (text.match(/[A-Za-z]/g) ?? []).length;

const rows = [];
await runRig({ outDir, speech, page, extra: { filler: FILLER, runs } }, async ({ capture, now, say, sleep, mark, evaluate, drain, settle, quietRelay, tap, ask }) => {
  for (const { first, second, ch, nudge } of runs) {
    await quietRelay();
    await tap(`session-${ch}`, ch, true);
    await evaluate('globalThis.__lastHubRequest = null');
    await ask(ch, 'first', 15000);
    if (!await evaluate('Boolean(globalThis.__lastHubRequest)')) throw new Error(`session ${ch}: the first request was not delegated`);
    await evaluate(`__deliverHubReply(${JSON.stringify(first)}, 'first-${ch}', [${JSON.stringify(FILLER)}])`);
    let at = now();
    await sleep(8000);
    await settle(ch, at, 3000, 30000);
    await evaluate('globalThis.__lastHubRequest = null');
    await ask(ch, 'second', 15000);
    const request = await evaluate('JSON.stringify(globalThis.__lastHubRequest)');
    if (request === 'null') throw new Error(`session ${ch}: the second request was not delegated`);
    const waited = mark('wait-start', { ch, nudge });
    if (nudge) await evaluate("__say('nudge')");
    await sleep(nudge ? 4000 : 10000);
    if (nudge) await settle(ch, waited, 3000, 20000);
    await drain();
    const filler = spokenBetween(capture, ch, waited, now());
    const nudgeDelegated = await evaluate(`JSON.stringify(globalThis.__lastHubRequest) !== ${JSON.stringify(request)}`);
    await evaluate(`globalThis.__lastHubRequest = ${request}; __deliverHubReply(${JSON.stringify(second)}, 'second-${ch}')`);
    at = now();
    await sleep(12000);
    await settle(ch, at, 3000, 40000);
    const reply = spokenReply(capture, ch, `second-${ch}`, now());
    await tap(`session-${ch}`, ch, false);
    const row = { ch, nudge, nudgeDelegated, filler, reply, fillerJapanese: japanese(filler), replyJapanese: japanese(reply), replyLatin: latin(reply) };
    say('ROW', JSON.stringify(row));
    rows.push(row);
  }
});
const failures = rows.flatMap((row) => [
  ...(!row.nudge || row.fillerJapanese ? [] : [`session ${row.ch}: the wait filler had no Japanese: ${JSON.stringify(row.filler)}`]),
  ...(row.replyJapanese === 0 && row.replyLatin >= 40 ? [] : [`session ${row.ch}: the hub reply was not spoken in English: ${JSON.stringify(row.reply)}`]),
]);
await writeFile(path.join(outDir, 'verdict.json'), JSON.stringify({ rows, failures }, null, 2));
if (failures.length) { console.log(failures.join('\n')); process.exit(1); }
console.log(`PASS: ${rows.filter((row) => row.nudge).length} sessions filled a wait in Japanese when asked during it, and all ${rows.length} spoke the next hub reply in English`);
