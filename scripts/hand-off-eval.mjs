import { existsSync } from 'node:fs';
import { appendFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { scoreTrial, summarize } from '../test/hand-off-measurements.js';
import { rigSpeech, runRig } from './live-rig.mjs';

const { positionals: [outDir, speechFile], values: { cases: casesFile, trials, only, page } } = parseArgs({
  allowPositionals: true,
  options: { cases: { type: 'string', default: new URL('hand-off-cases.json', import.meta.url).pathname }, trials: { type: 'string', default: '3' }, only: { type: 'string' }, page: { type: 'string' } },
});
if (!outDir) throw new Error('usage: node scripts/hand-off-eval.mjs OUTPUT [SPEECH_JSON] [--cases FILE] [--trials N] [--only KIND,...] [--page URL]');
await mkdir(outDir, { recursive: true });
const { cases: allCases } = JSON.parse(await readFile(casesFile, 'utf8'));
const cases = only ? allCases.filter(({ kind }) => only.split(',').includes(kind)) : allCases;
const spoken = Object.fromEntries(cases.flatMap(({ name, turns }) => turns.map((text, index) => [`${name}-${index}`, text])));
const rowsFile = path.join(outDir, 'rows.jsonl');
const known = speechFile && existsSync(speechFile) ? JSON.parse(await readFile(speechFile, 'utf8')) : {};
const missing = Object.entries(spoken).filter(([key, text]) => known[key]?.text !== text);
if (missing.length) {
  const { heldOutSpeech } = await import('./wake.mjs');
  const audio = await heldOutSpeech(missing.map(([, text]) => text));
  missing.forEach(([key, text], index) => { known[key] = { text, audio: audio[index] }; });
}
await writeFile(path.join(outDir, 'speech.json'), JSON.stringify(known));
const speech = await rigSpeech(outDir, path.join(outDir, 'speech.json'), spoken);
const readRows = async () => (existsSync(rowsFile) ? (await readFile(rowsFile, 'utf8')).split('\n').filter(Boolean).map((line) => JSON.parse(line)) : []);
const done = new Set((await readRows()).map(({ name, trial }) => `${name}#${trial}`));
const runs = Array.from({ length: Number(trials) }, (_, trial) => cases.map((item) => ({ ...item, trial }))).flat().filter(({ name, trial }) => !done.has(`${name}#${trial}`));

// Each run keeps its own capture, so a resumed run never overwrites the raw events of the trials before it.
await runRig({ outDir, speech, page, captureFile: `capture-${Date.now()}.json`, extra: { cases: runs.map(({ name, trial }) => `${name}#${trial}`) } }, async ({ capture, now, say, sleep, mark, evaluate, drain, settle, quietRelay, tap, forget, pressed }) => {
  for (const [index, item] of runs.entries()) {
    // Each trial starts from a page with no memory, so one trial's words never steer the next.
    if (index) await forget();
    const ch = 0;
    await quietRelay();
    await tap(`${item.name}#${item.trial}`, ch, true);
    try { await settle(ch, now(), 2500, 20000); } catch (error) { say('greeting', error.message); }
    const start = mark('case', { name: item.name, trial: item.trial });
    const turnAt = [];
    for (const turn of item.turns.keys()) {
      if (turn) await sleep(item.gap_ms ?? 2500);
      turnAt.push(now());
      await evaluate(`__say(${JSON.stringify(`${item.name}-${turn}`)})`);
    }
    const speechEnd = now();
    let deadline = speechEnd + 20000;
    let first = null;
    while (now() < deadline) {
      await drain();
      const sent = capture.frames.find((frame) => frame.dir === 'out' && frame.verb === 'delegate' && frame.at >= start);
      if (sent && first === null) { first = sent.at; deadline = Math.max(now(), speechEnd) + 6000; }
      await sleep(250);
    }
    let replyAt = null;
    let end = now();
    if (first !== null) {
      replyAt = mark('reply', { name: item.name });
      await evaluate(`__deliverHubReply(${JSON.stringify(item.reply)}, ${JSON.stringify(`${item.name}-${item.trial}`)})`);
      for (end = now(); end < replyAt + 20000; end = now()) {
        await drain();
        if (scoreTrial({ capture, ch, item, start, turnAt, speechEnd, replyAt, end }).replySpoken) break;
        await sleep(250);
      }
    }
    const row = { ...scoreTrial({ capture, ch, item, start, turnAt, speechEnd, replyAt, end }), trial: item.trial };
    say('ROW', JSON.stringify(row));
    await appendFile(rowsFile, JSON.stringify(row) + '\n');
    // A closing ends the session itself.
    if (await pressed()) await tap(`${item.name}#${item.trial}`, ch, false);
  }
});
const summary = summarize(await readRows());
await writeFile(path.join(outDir, 'summary.json'), JSON.stringify(summary, null, 2));
console.log(JSON.stringify(summary, null, 2));
