import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { RATE } from '../docs/wake.js';
import { assessDelegatedReplies, delegatedFixture } from '../test/delegated-reply-measurements.js';
import { rigSpeech, runRig } from './live-rig.mjs';

const { positionals: [outDir, speechFile], values: { page } } = parseArgs({ allowPositionals: true, options: { page: { type: 'string' } } });
if (!outDir) throw new Error('usage: node scripts/delegated-reply-smoke.mjs OUTPUT [SPEECH_JSON] [--page URL]');
const SPOKEN = {
  request: 'Ask the hub for the current test beacon status.',
  utterance: "While the hub checks that, let me tell you about the garden. The tomatoes came in early this year, the basil bolted in the heat, and the squash vines have climbed over the whole north fence and into the neighbor's yard.",
};
const speech = await rigSpeech(outDir, speechFile, SPOKEN);
const utteranceMs = Buffer.from(speech.utterance.audio, 'base64').length / 4 / RATE * 1000;
const INJECT_AT = 0.4;
const replies = ['amber', 'violet', 'silver'].map((color) => 'The test beacon is ' + color + '.');

const { capture } = await runRig({ outDir, speech, page, extra: { utterance: SPOKEN.utterance, replies } }, async ({ sleep, mark, evaluate, settle, quietRelay, tap, ask }) => {
  for (const [ch, reply] of replies.entries()) {
    await quietRelay();
    await tap(`session-${ch}`, ch, true);
    await evaluate('globalThis.__lastHubRequest = null');
    await ask(ch, 'request', 15000);
    if (!await evaluate('Boolean(globalThis.__lastHubRequest)')) throw new Error(`session ${ch}: the request was not delegated`);
    mark('utterance-start', { ch });
    await evaluate(`setTimeout(() => __deliverHubReply(${JSON.stringify(reply)}, 'reply-${ch}'), ${Math.round(utteranceMs * INJECT_AT)}); __say('utterance')`);
    const ended = mark('utterance-end', { ch });
    await sleep(12000);
    await settle(ch, ended, 3000, 30000);
    mark('reply-end', { ch });
    await tap(`session-${ch}`, ch, false);
  }
});
const fixture = delegatedFixture({ utterance: SPOKEN.utterance, replies, capture });
await writeFile(path.join(outDir, 'fixture.json'), JSON.stringify(fixture, null, 2));
const rows = assessDelegatedReplies(fixture);
await writeFile(path.join(outDir, 'verdict.json'), JSON.stringify(rows, null, 2));
console.log(`PASS: ${rows.length} sessions heard the whole utterance and spoke the reply injected mid-utterance only at the model's next turn`);
