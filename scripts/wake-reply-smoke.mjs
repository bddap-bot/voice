import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { NAME, WAKE_PHRASE } from '../docs/identity.js';
import { assessWakeReplies, recordedFixture, spokenBetween, spokenReply } from '../test/wake-reply-measurements.js';
import { rigSpeech, runRig } from './live-rig.mjs';

const { positionals: [outDir, speechFile], values: { page } } = parseArgs({ allowPositionals: true, options: { page: { type: 'string' } } });
if (!outDir) throw new Error('usage: node scripts/wake-reply-smoke.mjs OUTPUT [SPEECH_JSON] [--page URL]');
const SPOKEN = { wake: WAKE_PHRASE, recall: 'What color was the test beacon before you went to sleep?', request: 'Ask the hub for the current test beacon status.', sleep: `${NAME}, go to sleep.` };
const speech = await rigSpeech(outDir, speechFile, SPOKEN);
const replies = ['amber', 'violet', 'silver', 'crimson', 'golden', 'indigo', 'turquoise', 'copper', 'magenta', 'ivory', 'emerald'].map((color) => 'The test beacon is ' + color + '.');

const { commit, page: pageUrl, sourceHashes, capture } = await runRig({ outDir, speech, page, extra: { replies } }, async ({ capture, now, say, sleep, mark, evaluate, settle, quietRelay, wake, ask, askToSleep }) => {
  for (const [ch, reply] of replies.entries()) {
    await quietRelay();
    await wake(`session-${ch}`, ch);
    await settle(ch, 0, 3000, 30000);
    say('GREETING', ch, JSON.stringify(spokenBetween(capture, ch, 0, now())));
    if (ch) {
      const since = await ask(ch, 'recall', 30000);
      mark('recall-end', { ch });
      say('RECALL', ch, JSON.stringify({ expected: replies[ch - 1], spoken: spokenBetween(capture, ch, since, now()) }));
    }
    await evaluate('globalThis.__lastHubRequest = null');
    await ask(ch, 'request', 15000);
    await evaluate(`__deliverHubReply(${JSON.stringify(reply)}, 'reply-${ch}')`);
    const delivered = now();
    await sleep(12000);
    await settle(ch, delivered, 3000, 30000);
    const audio = await evaluate(`__peers[${ch}].getStats().then((stats) => [...stats.values()].filter((s) => s.type === 'inbound-rtp' && s.kind === 'audio').map(({ bytesReceived, totalSamplesReceived }) => ({ bytesReceived, totalSamplesReceived })))`);
    mark('reply-end', { ch, audio });
    say('REPLY', ch, JSON.stringify({ expected: reply, spoken: spokenReply(capture, ch, `reply-${ch}`, now()) }));
    await askToSleep(ch);
  }
});
const fixture = recordedFixture({ replies, capture });
await writeFile(path.join(outDir, 'fixture.json'), JSON.stringify(fixture, null, 2));
const rows = assessWakeReplies(fixture);
await writeFile(path.join(outDir, 'verdict.json'), JSON.stringify({ commit, page: pageUrl, sourceHashes, rows }, null, 2));
console.log(`PASS: ${rows.length - 1} woken sessions greeted without a farewell, recalled the answer from before their sleep, and spoke their exact fresh replies`);
