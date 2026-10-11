import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { hubFrames } from './live-capture.js';
import { assessWakeReplies } from './wake-reply-measurements.js';

const recorded = () => JSON.parse(fs.readFileSync(process.env.VOICE_WAKE_REPLY_CAPTURE ?? new URL('./fixtures/wake-reply-capture.json', import.meta.url), 'utf8'));
const recordedSignOff = recorded().signOff;
const mark = (data, kind, session) => data.capture.marks.find((item) => item.kind === kind && item.ch === session);
const outputDeltas = (data, session, from, to) => data.capture.rt.filter((row) => row.ch === session && row.dir === 'in' && row.at >= from && row.at <= to && row.event.type === 'output_transcript.added');
const overwrite = (deltas, text) => { assert.ok(deltas.length); deltas.forEach((row, index) => { row.event.item.text = index === 0 ? text : ''; }); };
const replyAt = (data, session) => hubFrames(data.capture).find((hub) => hub.stamp === `reply-${session}`).at;
const prefixReply = (data, session, prefix) => { const { item } = outputDeltas(data, session, replyAt(data, session), mark(data, 'reply-end', session).at)[0].event; item.text = prefix + item.text; };
const offer = (data, session, edit) => {
  const frame = data.capture.frames.filter((row) => row.verb === 'offer')[session];
  const value = JSON.parse(frame.text.slice(6));
  edit(value);
  frame.text = 'offer\n' + JSON.stringify(value);
};
const memory = (value, edit) => {
  const [archived] = value.context;
  const split = archived.text.indexOf('\n') + 1;
  const conversations = JSON.parse(archived.text.slice(split));
  conversations.at(-1).turns = edit(conversations.at(-1).turns);
  archived.text = archived.text.slice(0, split) + JSON.stringify(conversations);
};

test('recorded woken sessions greet, carry the conversation from before their sleep, and speak each fresh hub reply exactly', () => {
  assert.equal(assessWakeReplies(recorded()).length, 11);
});

test('a carried sign-off split around a late input transcript still counts', () => {
  const data = recorded();
  offer(data, 2, (value) => memory(value, (turns) => turns.flatMap((turn) => turn.speaker === 'live' && turn.text.includes(recordedSignOff) ? [{ speaker: 'live', text: turn.text.replace(recordedSignOff, recordedSignOff.slice(0, recordedSignOff.indexOf(' '))) }, { speaker: 'user', text: ' sleep' }, { speaker: 'live', text: recordedSignOff.slice(recordedSignOff.indexOf(' ')) }] : [turn])));
  assert.equal(assessWakeReplies(data).length, 11);
});

test('a vocalization in the transcript around a fresh reply is not speech', () => {
  const data = recorded();
  prefixReply(data, 1, '[exhale]');
  assert.equal(assessWakeReplies(data).length, 11);
});

for (let session = 1; session <= 10; session++) {
  test(`a prior answer substituted in woken session ${session} fails the live oracle`, () => {
    const data = recorded();
    overwrite(outputDeltas(data, session, replyAt(data, session), mark(data, 'reply-end', session).at), data.replies[session - 1]);
    assert.throws(() => assessWakeReplies(data), (error) => error.message.includes(`session ${session}: spoke something other than the fresh hub reply`) && error.actual === data.replies[session - 1]);
  });
}

for (const [label, mutate, error] of [
  ['missing carried answer', (data) => offer(data, 1, (value) => { value.context = []; }), /previous answer was not carried/],
  ['wake marker without the gap', (data) => offer(data, 1, (value) => { value.wake = value.wake.replace(/about \d+ \w+ ago/, 'a while ago'); }), /session 1: missing wake marker/],
  ['conversation not ended by the sign-off', (data) => offer(data, 2, (value) => memory(value, (turns) => turns.map((turn) => turn.speaker === 'live' && (turn.text.includes(recordedSignOff) || recordedSignOff.includes(turn.text.trim())) ? { ...turn, text: ' Still listening.' } : turn))), /session 2: the carried conversation did not end on the sign-off/],
  ['page reload', (data) => data.capture.marks.push({ kind: 'page-ready' }), /sessions must share one page/],
  ['missing sleep', (data) => { data.capture.rt = data.capture.rt.filter((row) => row.ch !== 1 || row.dir !== 'state' || row.event.raw !== 'close'); }, /session never closed/],
  ['session ending itself', (data) => { data.capture.rt.find((row) => row.ch === 1 && row.dir === 'state' && row.event.raw === 'close').at = mark(data, 'request-start', 1).at; }, /session 1: ended before the sleep request/],
  ['no sign-off', (data) => overwrite(outputDeltas(data, 10, mark(data, 'sleep-start', 10).at, Infinity), 'Goodnight.'), /session 10: did not sign off when asked to sleep/],
  ['farewell greeting', (data) => { outputDeltas(data, 1, 0, mark(data, 'request-start', 1).at)[0].event.item.text = 'Goodbye. '; }, /session 1: opened with a farewell/],
  ['spoken delegation label', (data) => { prefixReply(data, 1, '[delegation_result]'); }, /session 1: spoke something other than the fresh hub reply/],
  ['extra speech', (data) => { data.capture.rt.find((row) => row.ch === 1 && row.dir === 'in' && row.at >= mark(data, 'request-start', 1).at && row.event.type === 'output_transcript.added' && row.event.item.text.includes('violet')).event.item.text += ' The hub will answer later.'; }, /spoke something other than the fresh hub reply/],
]) {
  test(`${label} cannot produce a passing wake measurement`, () => {
    const data = recorded();
    mutate(data);
    assert.throws(() => assessWakeReplies(data), error);
  });
}
