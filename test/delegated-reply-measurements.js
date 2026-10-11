import assert from 'node:assert/strict';
import { applied, eventText, hubFrames, recordedRows, replySent } from './live-capture.js';

export const OVERLAP_MS = 400;
export const LEAD_MS = 3000;
export const STALL_MS = 1000;
const words = (text) => text.toLowerCase().replace(/[’']/g, '').match(/[a-z]+/g) ?? [];
const subject = (reply) => words(reply).at(-1);

export function delegatedFixture({ utterance, replies, capture }) {
  return {
    utterance,
    replies,
    capture: {
      marks: capture.marks.filter(({ kind }) => ['utterance-start', 'utterance-end', 'reply-end'].includes(kind)),
      frames: capture.frames.filter(({ verb }) => verb === 'hub'),
      rt: recordedRows(capture.rt),
    },
  };
}

export function assessDelegatedReplies({ utterance, replies, capture }) {
  const expected = words(utterance);
  const rows = [];
  for (const [ch, reply] of replies.entries()) {
    const [start, end, done] = ['utterance-start', 'utterance-end', 'reply-end'].map((kind) => capture.marks.find((mark) => mark.kind === kind && mark.ch === ch));
    assert.ok(start && end && done && start.at < end.at && end.at < done.at, `session ${ch}: missing utterance window`);
    const incoming = capture.rt.filter((row) => row.ch === ch && row.dir === 'in');
    const hub = hubFrames(capture).find(({ at, commentary }) => at >= start.at && at <= end.at && commentary[0] === reply);
    const sent = hub && replySent(capture, ch, hub);
    assert.ok(sent && sent.at <= end.at, `session ${ch}: the reply did not reach Live during the utterance`);
    assert.ok(incoming.some(({ at, event }) => at < sent.at && event.type === 'delegation.created' && event.item?.id === sent.event.delegation_item_id), `session ${ch}: the reply was not addressed to the session's delegation`);
    const ack = applied(capture, sent);
    assert.ok(Number.isFinite(ack?.event.start_ms), `session ${ch}: the reply was not applied to the session timeline`);
    const heard = incoming.filter(({ at, event }) => at >= start.at && at <= done.at && event.type === 'input_transcript.added');
    const transcript = words(heard.map(({ event }) => eventText(event)).join(' '));
    const covered = expected.filter((word) => transcript.includes(word)).length / expected.length;
    assert.ok(covered >= 0.8 && transcript.slice(-3).includes(expected.at(-1)), `session ${ch}: the utterance transcript is not whole: ${transcript.join(' ')}`);
    const starts = heard.map(({ event }) => event.start_ms);
    assert.ok(starts.every((at, index) => !index || at - starts[index - 1] <= STALL_MS), `session ${ch}: the microphone stalled mid-utterance`);
    const userEnd = Math.max(...starts);
    assert.ok(ack.event.start_ms <= userEnd - LEAD_MS, `session ${ch}: the reply reached the model too late in the utterance to test the turn`);
    const spoken = incoming.filter(({ at, event }) => at >= sent.at && at <= done.at && event.type === 'output_transcript.added');
    const replyWords = new Set(words(reply).filter((word) => word.length > 3));
    const early = words(spoken.filter(({ event }) => event.start_ms < userEnd - OVERLAP_MS).map(({ event }) => eventText(event)).join(''));
    assert.deepEqual(early.filter((word) => replyWords.has(word)), [], `session ${ch}: spoke the reply over the utterance`);
    const said = spoken.map(({ event }) => eventText(event)).join('');
    assert.ok(words(said).includes(subject(reply)), `session ${ch}: never spoke the reply`);
    rows.push({ session: ch, heard: heard.map(({ event }) => eventText(event)).join(''), spoken: said.trim() });
  }
  return rows;
}
