import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { LEAD_MS, STALL_MS, assessDelegatedReplies } from './delegated-reply-measurements.js';
import { applied } from './live-capture.js';

const recorded = () => JSON.parse(fs.readFileSync(process.env.VOICE_DELEGATED_REPLY_CAPTURE ?? new URL('./fixtures/delegated-reply-capture.json', import.meta.url), 'utf8'));
const mark = (data, kind, session) => data.capture.marks.find((item) => item.kind === kind && item.ch === session);
const events = (data, session, type) => data.capture.rt.filter((row) => row.ch === session && row.dir === 'in' && row.at >= mark(data, 'utterance-start', session).at && row.at <= mark(data, 'reply-end', session).at && row.event.type === type);
const userEnd = (data, session) => Math.max(...events(data, session, 'input_transcript.added').map((row) => row.event.start_ms));
const sent = (data, session) => data.capture.rt.find((row) => row.ch === session && row.dir === 'out' && row.event.type === 'delegation.context.append');
const ack = (data, session) => data.capture.rt.find((row) => row.ch === session && row.dir === 'in' && row.event.type === 'delegation.context.appended');

test('a reply injected mid-utterance is spoken at the next turn and the utterance is heard whole', () => {
  assert.equal(assessDelegatedReplies(recorded()).length, 3);
});

test('a follow-up append to one delegation pairs with the acknowledgement in send order', () => {
  const { capture } = recorded();
  const first = capture.rt.find((row) => row.ch === 0 && row.dir === 'out' && row.event.type === 'delegation.context.append');
  const firstAck = capture.rt.find((row) => row.ch === 0 && row.dir === 'in' && row.event.type === 'delegation.context.appended');
  const second = structuredClone({ ...first, at: firstAck.at + 1 });
  const secondAck = structuredClone({ ...firstAck, at: firstAck.at + 2 });
  capture.rt.push(second, secondAck);
  assert.equal(applied(capture, first), firstAck);
  assert.equal(applied(capture, second), secondAck);
});

for (const [label, mutate, error] of [
  ['reply spoken over the utterance', (data) => { const reply = events(data, 0, 'output_transcript.added').find((row) => /amber/i.test(row.event.item.text)); reply.event.start_ms = userEnd(data, 0) - 2000; }, /session 0: spoke the reply over the utterance/],
  ['truncated utterance', (data) => { const heard = events(data, 1, 'input_transcript.added'); data.capture.rt = data.capture.rt.filter((row) => !heard.slice(-4).includes(row)); }, /session 1: the utterance transcript is not whole/],
  ['reply applied after the utterance', (data) => { ack(data, 2).event.start_ms = userEnd(data, 2) - LEAD_MS + 200; }, /session 2: the reply reached the model too late/],
  ['reply never acknowledged', (data) => { data.capture.rt = data.capture.rt.filter((row) => row !== ack(data, 1)); }, /session 1: the reply was not applied to the session timeline/],
  ['reply addressed to another delegation', (data) => { sent(data, 0).event.delegation_item_id = 'item_other'; }, /session 0: the reply did not reach Live during the utterance/],
  ['reply with other text', (data) => { sent(data, 2).event.content[0].text = data.replies[0]; }, /session 2: the reply did not reach Live during the utterance/],
  ['reply never spoken', (data) => { for (const row of events(data, 1, 'output_transcript.added')) row.event.item.text = row.event.item.text.replace(/violet/gi, ''); }, /session 1: never spoke the reply/],
  ['microphone stall', (data) => { for (const row of events(data, 2, 'input_transcript.added').slice(5)) row.event.start_ms += STALL_MS; }, /session 2: the microphone stalled mid-utterance/],
]) {
  test(`${label} fails the delegated-reply check`, () => {
    const data = recorded();
    mutate(data);
    assert.throws(() => assessDelegatedReplies(data), error);
  });
}
