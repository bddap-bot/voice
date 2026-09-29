import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { LEAD_MS, STALL_MS, assessDelegatedReplies } from './delegated-reply-measurements.js';

const load = (name) => JSON.parse(fs.readFileSync(new URL(`./fixtures/${name}`, import.meta.url), 'utf8'));
const recorded = () => process.env.VOICE_DELEGATED_REPLY_CAPTURE ? JSON.parse(fs.readFileSync(process.env.VOICE_DELEGATED_REPLY_CAPTURE, 'utf8')) : load('delegated-reply-capture.json');
const mark = (data, kind, session) => data.capture.marks.find((item) => item.kind === kind && item.ch === session);
const events = (data, session, type) => data.capture.rt.filter((row) => row.ch === session && row.dir === 'in' && row.at >= mark(data, 'utterance-start', session).at && row.at <= mark(data, 'reply-end', session).at && row.event.type === type);
const userEnd = (data, session) => Math.max(...events(data, session, 'session.input_transcript.delta').map((row) => row.event.start_ms));

test('a reply injected mid-utterance is spoken at the next turn and the utterance is heard whole', () => {
  assert.equal(assessDelegatedReplies(recorded()).length, 3);
});

test('a capture in which the model spoke the reply over the utterance fails', () => {
  assert.throws(() => assessDelegatedReplies(load('delegated-reply-barge-capture.json')), /spoke the reply over the utterance/);
});

for (const [label, mutate, error] of [
  ['reply spoken over the utterance', (data) => { const reply = events(data, 0, 'session.output_transcript.delta').find((row) => /beacon/i.test(row.event.delta)); reply.event.start_ms = userEnd(data, 0) - 2000; }, /session 0: spoke the reply over the utterance/],
  ['truncated utterance', (data) => { const heard = events(data, 1, 'session.input_transcript.delta'); data.capture.rt = data.capture.rt.filter((row) => !heard.slice(-4).includes(row)); }, /session 1: the utterance transcript is not whole/],
  ['reply applied after the utterance', (data) => { const applied = data.capture.rt.find((row) => row.ch === 2 && row.event.type === 'session.commentary.appended' && row.event.client_event_id === 'hub_reply-2'); applied.event.start_ms = userEnd(data, 2) - LEAD_MS + 200; }, /session 2: the reply reached the model too late/],
  ['reply never spoken', (data) => { for (const row of events(data, 1, 'session.output_transcript.delta')) row.event.delta = row.event.delta.replace(/violet/gi, ''); }, /session 1: never spoke the reply/],
  ['microphone stall', (data) => { for (const row of events(data, 2, 'session.input_transcript.delta').slice(5)) row.event.start_ms += STALL_MS; }, /session 2: the microphone stalled mid-utterance/],
]) {
  test(`${label} fails the delegated-reply check`, () => {
    const data = recorded();
    mutate(data);
    assert.throws(() => assessDelegatedReplies(data), error);
  });
}
