import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { scoreTrial, summarize } from './hand-off-measurements.js';

const said = (at, text) => ({ at, ch: 0, dir: 'in', event: { type: 'output_transcript.added', item: { text } } });
const delegated = (at, text) => ({ at, dir: 'out', verb: 'delegate', text: `delegate\n${JSON.stringify({ id: `item_${at}`, text })}` });
const item = { name: 'whole', kind: 'whole', turns: ['Can you check on the backup job.', 'And tell me when it last finished.', 'Fly, Corvus.'] };
const quiet = (at, detail, id = `batch_${at}`) => ({ at, dir: 'out', verb: 'telemetry', batch: { id, events: [{ kind: 'session', name: 'hand-off-quiet', detail: JSON.stringify(detail), at }] } });
const trial = (rt, frames, replyAt = 9000, levels) => scoreTrial({ capture: { rt, frames, levels }, ch: 0, item, start: 0, turnAt: [100, 3000, 6000], speechEnd: 7000, replyAt, end: 12000 });

test('a trial is one whole request when one relay carries every turn, and a promise of silence counts from the phrase to the reply', () => {
  const row = trial([said(1000, 'Sure, checking that.'), said(6800, ' Staying quiet.'), said(9500, 'It starts in seventeen minutes.')], [delegated(7400, 'Can you check on the backup job\nAnd tell me when it last finished')]);
  assert.deepEqual(row, {
    name: 'whole', kind: 'whole', relayed: true, relays: 1, whole: true, clarifying: false, promisedSilence: true, playedSilence: true, audibleAfterSpeechMs: null, replySpoken: true, relayMs: 400,
    beforeRelay: 'Sure, checking that. Staying quiet.', afterPhrase: 'Staying quiet.', playedAfterPhrase: 'Staying quiet.', delegated: ['Can you check on the backup job\nAnd tell me when it last finished'], afterReply: 'It starts in seventeen minutes.', quietUnpaired: false, records: [],
  });
});

test('a promise before the phrase, a split request and an unspoken reply are each told apart', () => {
  const row = trial([said(1000, "I'll stay quiet. What job?"), said(9500, 'Okay.')], [delegated(2000, 'Can you check on the backup job'), delegated(7600, 'And tell me when it last finished')]);
  assert.equal(row.promisedSilence, false);
  assert.equal(row.clarifying, true);
  assert.equal(row.relays, 2);
  assert.equal(row.whole, false);
  assert.equal(row.replySpoken, false);
  assert.equal(row.relayMs, 600);
  const none = trial([said(6500, 'Standing by.')], [], null);
  assert.deepEqual([none.relayed, none.promisedSilence, none.replySpoken, none.relayMs], [false, true, null, null]);
});

test("what the model says while the page's records say it plays nothing counts as said but not played, and audible output is measured from the speaker", () => {
  const levels = [7000, 7050, 7100, 7150].map((at, index) => ({ at, rms: index < 2 ? 0.2 : 0.001 }));
  const row = trial([said(6800, 'Okay, staying quiet.'), said(9500, 'It starts in seventeen minutes.')], [delegated(7400, 'Can you check on the backup job\nAnd tell me when it last finished'), quiet(6700, { on: true }), quiet(9000, { on: false, reason: 'hub reply', unheard_chars: 20 })], 9000, levels);
  assert.deepEqual([row.promisedSilence, row.playedSilence, row.playedAfterPhrase, row.audibleAfterSpeechMs, row.replySpoken], [true, false, '', 100, true]);
  assert.deepEqual(row.records, [['hand-off-quiet', { on: true }, 6700], ['hand-off-quiet', { on: false, reason: 'hub reply', unheard_chars: 20 }, 9000]]);
  assert.equal(row.quietUnpaired, false);
  const resent = trial([], [quiet(6700, { on: true }), quiet(6700, { on: true }), quiet(6900, { on: true }, 'batch_lost_end')], 9000);
  assert.deepEqual([resent.records.length, resent.quietUnpaired], [2, true]);
  const cut = trial([], [quiet(6700, { on: true }), { at: 8000, dir: 'out', verb: 'telemetry', batch: { id: null, events: [], unread: true } }], 9000);
  assert.deepEqual([cut.records.length, cut.quietUnpaired], [1, true]);
  const unplayedReply = trial([said(9500, 'It starts in seventeen minutes.')], [delegated(7400, 'Can you check on the backup job\nAnd tell me when it last finished'), quiet(6700, { on: true }), quiet(30000, { on: false, reason: 'session ended', unheard_chars: 31 })], 9000);
  assert.deepEqual([unplayedReply.promisedSilence, unplayedReply.replySpoken], [false, false]);
});

test('the summary counts each rate per kind over its trials', () => {
  const rows = [
    { kind: 'whole', relayed: true, relays: 1, whole: true, clarifying: false, promisedSilence: false, playedSilence: false, audibleAfterSpeechMs: 0, replySpoken: true, relayMs: 400 },
    { kind: 'whole', relayed: true, relays: 2, whole: false, clarifying: false, promisedSilence: true, playedSilence: true, audibleAfterSpeechMs: 900, replySpoken: false, relayMs: 900 },
    { kind: 'whole', relayed: false, relays: 0, whole: false, clarifying: true, promisedSilence: false, playedSilence: false, audibleAfterSpeechMs: 200, replySpoken: null, relayMs: null },
    { kind: 'fragment', relayed: true, relays: 1, whole: true, clarifying: false, promisedSilence: false, playedSilence: false, audibleAfterSpeechMs: null, replySpoken: true, relayMs: 5000 },
  ];
  assert.deepEqual(summarize(rows), {
    whole: { trials: 3, relayed: '2/3', whole: '1/3', fragmented: '1/3', clarifying: '1/3', promisedSilence: '1/3', playedSilence: '1/3', medianAudibleAfterSpeechMs: 200, replySpoken: '1/2', medianRelayMs: 650 },
    fragment: { trials: 1, relayed: '1/1', whole: '1/1', fragmented: '0/1', clarifying: '0/1', promisedSilence: '0/1', playedSilence: '0/1', medianAudibleAfterSpeechMs: null, replySpoken: '1/1', medianRelayMs: 5000 },
  });
});

test('every hand-off case names a kind, gives each turn words and a reply the scorer recognises', async () => {
  const { cases } = JSON.parse(await readFile(new URL('../scripts/hand-off-cases.json', import.meta.url), 'utf8'));
  assert.equal(new Set(cases.map(({ name }) => name)).size, cases.length);
  for (const { name, kind, turns, reply } of cases) {
    assert.ok(['vague', 'misheard', 'whole', 'fragment'].includes(kind), name);
    assert.ok(turns.length && turns.every((turn) => /\w/.test(turn)), name);
    assert.match(reply, /\bseventeen\b/, name);
  }
});
